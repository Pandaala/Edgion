use super::*;
use crate::ai::{
    AiProviderLabel, AiRequestContext, AiUsageShared, GenAiAttempt, GenAiAttemptFailure, GenAiAttemptUsageSource,
    UsageExtractionProcessor,
};
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::StreamingResponseProcessor;
use crate::BackendContext;
use bytes::Bytes;
use edgion_resources::resources::edgion_backend::AiModelPricing;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::Arc;

fn ai_context(provider: AiProviderLabel, model: &str, requested_streaming: bool) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.ensure_ai_state().request = Some(AiRequestContext {
        provider,
        model: model.to_string(),
        pricing: None,
        requested_streaming,
        usage: Arc::new(AiUsageShared::default()),
        quota: None,
        quota_status: None,
    });
    ctx
}

async fn observe_response(ctx: &EdgionHttpContext, body: &[u8], eos: bool, kind: ResponseStreamKind) {
    let shared = ctx.ai_state().unwrap().request.as_ref().unwrap().usage.clone();
    let mut processor = UsageExtractionProcessor::new(shared);
    let mut chunk = Some(Bytes::copy_from_slice(body));
    let mut session = MockPluginSession::new();
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    ai.expect_semantic_guard_active().return_const(false);
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
    session.expect_record_error().return_const(());
    let mut sink = pingora_proxy::ResponseBodySink::new();
    processor.on_chunk(&mut chunk, eos, kind, &mut sink, &mut session).await;
    assert_eq!(chunk.as_deref(), Some(body), "usage observation must not mutate body");
}

fn init_backend_context(ctx: &mut EdgionHttpContext) {
    ctx.backend_context = Some(BackendContext {
        source: None,
        rsc: None,
        service_key: None,
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: None,
        effective_policy_captured: false,
        effective_policy: None,
    });
}

#[test]
fn ceil_character_estimate_is_overflow_safe() {
    assert_eq!(ceil_chars_div_four(0), 0);
    assert_eq!(ceil_chars_div_four(1), 1);
    assert_eq!(ceil_chars_div_four(4), 1);
    assert_eq!(ceil_chars_div_four(5), 2);
    assert_eq!(ceil_chars_div_four(u64::MAX), u64::MAX / 4 + 1);
}

#[tokio::test]
async fn finalize_uses_reported_usage_and_actual_sse_classification() {
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
    observe_response(
        &ctx,
        b"data: {\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":7}}\n\n",
        true,
        ResponseStreamKind::Sse,
    )
    .await;
    ctx.response_stream_kind = ResponseStreamKind::Sse;
    finalize_ai_usage_with_metrics_enabled(&mut ctx, false);

    let summary = ctx.ai_state().unwrap().summary.as_ref().unwrap();
    assert_eq!((summary.input_tokens, summary.output_tokens), (Some(12), Some(7)));
    assert!(!summary.usage_estimated);
    assert!(
        summary.streaming,
        "actual response classification must win over requested=false"
    );
}

#[tokio::test]
async fn finalize_prices_ordinary_cached_and_output_tokens_without_float() {
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
    ctx.ensure_ai_state().request.as_mut().unwrap().pricing = Some(
        AiModelPricing {
            input_per_million_usd: "10".to_string(),
            output_per_million_usd: "20".to_string(),
            cache_read_input_per_million_usd: Some("1".to_string()),
            cache_write_input_per_million_usd: Some("2".to_string()),
        }
        .parsed()
        .unwrap(),
    );
    observe_response(
            &ctx,
            br#"{"usage":{"prompt_tokens":100,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":20},"cache_creation_input_tokens":10}}"#,
            true,
            ResponseStreamKind::None,
        )
        .await;
    finalize_ai_usage_with_metrics_enabled(&mut ctx, false);

    let summary = ctx.ai_state().unwrap().summary.as_ref().unwrap();
    assert_eq!(summary.cache_read_input_tokens, Some(20));
    assert_eq!(summary.cache_write_input_tokens, Some(10));
    assert_eq!(summary.estimated_cost_usd.as_deref(), Some("0.00084"));
    let json = crate::observe::AccessLogEntry::from_context(&ctx).to_json();
    assert!(json.contains(r#""estimated_cost_usd":"0.00084""#));
}

#[test]
fn finalize_preserves_quota_admission_bypass_status() {
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "model-a", false);
    let ai = ctx.ensure_ai_state().request.as_mut().unwrap();
    ai.quota_status = Some("fail_open");
    ai.usage.observe_reported_usage(2, 3, 0, 0);

    finalize_ai_usage_with_metrics_enabled(&mut ctx, false);

    assert_eq!(
        ctx.ai_state().unwrap().summary.as_ref().unwrap().quota_status,
        Some("fail_open")
    );
}

#[tokio::test]
async fn finalize_estimates_partial_aborted_stream_and_emits_metrics_once() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut ctx = ai_context(AiProviderLabel::OpenAiCompatible, "other", true);
    ctx.ai_state()
        .unwrap()
        .request
        .as_ref()
        .unwrap()
        .usage
        .set_input_chars(5);
    observe_response(&ctx, b"data: hello", false, ResponseStreamKind::Sse).await;
    ctx.response_stream_kind = ResponseStreamKind::Sse;
    metrics::with_local_recorder(&recorder, || {
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
    });

    let summary = ctx.ai_state().unwrap().summary.as_ref().unwrap();
    assert_eq!((summary.input_tokens, summary.output_tokens), (Some(2), Some(2)));
    assert!(summary.usage_estimated);
    assert!(summary.streaming);

    let rendered = handle.render();
    let samples: Vec<&str> = rendered
        .lines()
        .filter(|line| line.starts_with(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL))
        .collect();
    assert_eq!(
        samples.len(),
        2,
        "aborted request must emit one input/output pair:\n{rendered}"
    );
    assert!(samples.iter().all(|line| line.ends_with(" 2")));
}

#[test]
fn ttft_uses_only_current_attempt_and_is_absent_without_its_body() {
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
    init_backend_context(&mut ctx);
    ctx.push_upstream(None, None);
    ctx.get_current_upstream_mut().unwrap().bt = Some(10);
    ctx.push_upstream(None, None);
    finalize_ai_usage_with_metrics_enabled(&mut ctx, false);
    assert_eq!(ctx.ai_state().unwrap().summary.as_ref().unwrap().ttft_ms, None);

    let mut with_body = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
    init_backend_context(&mut with_body);
    with_body.push_upstream(None, None);
    with_body.get_current_upstream_mut().unwrap().bt = Some(10);
    with_body.push_upstream(None, None);
    with_body.get_current_upstream_mut().unwrap().bt = Some(23);
    finalize_ai_usage_with_metrics_enabled(&mut with_body, false);
    assert_eq!(
        with_body.ai_state().unwrap().summary.as_ref().unwrap().ttft_ms,
        Some(23)
    );
}

#[tokio::test]
async fn finalize_is_idempotent_and_emits_metrics_once() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
    observe_response(
        &ctx,
        br#"{"usage":{"prompt_tokens":12,"completion_tokens":7}}"#,
        true,
        ResponseStreamKind::None,
    )
    .await;

    metrics::with_local_recorder(&recorder, || {
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
        let first = ctx.ai_state().unwrap().summary.clone();
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
        assert_eq!(ctx.ai_state().unwrap().summary, first);
    });
    let rendered = handle.render();
    let samples: Vec<&str> = rendered
        .lines()
        .filter(|line| line.starts_with(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL))
        .collect();
    assert_eq!(
        samples.len(),
        2,
        "one request must create only input/output series:\n{rendered}"
    );
    assert!(samples.iter().any(|line| line.ends_with(" 12")));
    assert!(samples.iter().any(|line| line.ends_with(" 7")));
}

#[test]
fn retry_metrics_sum_all_attempts_and_emit_once() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
    for (number, input_tokens, output_tokens, failure) in
        [(1, 3, 4, Some(GenAiAttemptFailure::RetryableStatus)), (2, 5, 7, None)]
    {
        ctx.ensure_ai_state().attempt_runtime.record_test_attempt(
            GenAiAttempt {
                number,
                backend: "service-a".to_string(),
                provider: "openai",
                credential: "primary".to_string(),
                status: Some(200),
                failure,
                retry_after_seconds: None,
                ttft_ms: None,
                usage_source: GenAiAttemptUsageSource::Reported,
                input_tokens: Some(input_tokens),
                output_tokens: Some(output_tokens),
                estimated_cost_usd: None,
            },
            AiProviderLabel::OpenAi,
        );
    }

    metrics::with_local_recorder(&recorder, || {
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
    });

    let rendered = handle.render();
    let samples: Vec<&str> = rendered
        .lines()
        .filter(|line| line.starts_with(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL))
        .collect();
    assert_eq!(
        samples.len(),
        2,
        "attempts must share fixed-label input/output series:\n{rendered}"
    );
    assert!(samples.iter().any(|line| line.ends_with(" 8")));
    assert!(samples.iter().any(|line| line.ends_with(" 11")));
    assert!(samples.iter().all(|line| line.contains("provider=\"openai\"")));
    assert!(samples.iter().all(|line| line.contains("usage_source=\"reported\"")));
}

#[test]
fn disabled_or_non_ai_finalization_never_registers_metric_family() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let mut ai = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
        finalize_ai_usage_with_metrics_enabled(&mut ai, false);
        let mut ordinary = EdgionHttpContext::new();
        finalize_ai_usage_with_metrics_enabled(&mut ordinary, true);
        assert!(ordinary.ai_state().is_none());
    });
    assert!(!handle
        .render()
        .contains(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL));
}

#[tokio::test]
async fn access_log_genai_shape_contains_no_request_response_or_key_text() {
    let prompt = "PROMPT-FIXTURE-DO-NOT-LOG";
    let completion = "COMPLETION-FIXTURE-DO-NOT-LOG";
    let api_key = "API-KEY-FIXTURE-DO-NOT-LOG";
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "gpt-declared", false);
    ctx.ai_state()
        .unwrap()
        .request
        .as_ref()
        .unwrap()
        .usage
        .set_input_chars(prompt.chars().count() as u64);
    let event = format!("data: {completion}");
    observe_response(&ctx, event.as_bytes(), false, ResponseStreamKind::Sse).await;
    ctx.ensure_ai_state().attempt_runtime.record_test_attempt(
        GenAiAttempt {
            number: 1,
            backend: "default/service-a".to_string(),
            provider: "openai",
            credential: "primary".to_string(),
            status: Some(200),
            failure: None,
            retry_after_seconds: Some(3),
            ttft_ms: Some(11),
            usage_source: GenAiAttemptUsageSource::Estimated,
            input_tokens: Some(4),
            output_tokens: Some(5),
            estimated_cost_usd: Some("0.00001".to_string()),
        },
        AiProviderLabel::OpenAi,
    );
    finalize_ai_usage_with_metrics_enabled(&mut ctx, false);

    let json = crate::observe::AccessLogEntry::from_context(&ctx).to_json();
    assert!(!json.contains(prompt));
    assert!(!json.contains(completion));
    assert!(!json.contains(api_key));
    assert!(!json.contains("authorization"));
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let genai = value["genai"].as_object().unwrap();
    assert_eq!(genai["provider"], "openai");
    assert_eq!(genai["model"], "gpt-declared");
    assert_eq!(genai["streaming"], false);
    assert!(genai.get("ttft_ms").is_none());
    let keys: std::collections::BTreeSet<&str> = genai.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "input_tokens",
            "model",
            "output_tokens",
            "provider",
            "streaming",
            "usage_estimated",
            "usage_source",
            "attempts",
        ]
        .into_iter()
        .collect()
    );
    let attempts = genai["attempts"].as_array().expect("attempt array");
    assert_eq!(attempts.len(), 1);
    let attempt = attempts[0].as_object().expect("attempt object");
    assert_eq!(attempt["backend"], "default/service-a");
    assert_eq!(attempt["credential"], "primary");
    assert_eq!(
        attempt
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "credential",
            "estimated_cost_usd",
            "input_tokens",
            "number",
            "output_tokens",
            "provider",
            "retry_after_seconds",
            "backend",
            "status",
            "ttft_ms",
            "usage_source",
        ]
        .into_iter()
        .collect()
    );

    let ordinary = EdgionHttpContext::new();
    let ordinary_json = crate::observe::AccessLogEntry::from_context(&ordinary).to_json();
    assert!(!ordinary_json.contains("\"genai\""));
}

#[tokio::test]
async fn malformed_json_omits_counts_cost_and_metrics_in_canonical_log() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "known-model", false);
    observe_response(
        &ctx,
        br#"{"usage":{"prompt_tokens":2,"completion_tokens":3}}junk"#,
        true,
        ResponseStreamKind::None,
    )
    .await;
    metrics::with_local_recorder(&recorder, || finalize_ai_usage_with_metrics_enabled(&mut ctx, true));
    let summary = ctx.ai_state().unwrap().summary.as_ref().unwrap();
    assert_eq!(summary.usage_source, GenAiAttemptUsageSource::Unknown);
    assert_eq!(
        (
            summary.input_tokens,
            summary.output_tokens,
            summary.estimated_cost_usd.as_ref()
        ),
        (None, None, None)
    );
    let json = serde_json::to_value(summary).unwrap();
    for key in [
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_write_input_tokens",
        "estimated_cost_usd",
    ] {
        assert!(json.get(key).is_none());
    }
    assert_eq!(json["usage_source"], "unknown");
    assert!(!handle
        .render()
        .lines()
        .any(|line| line.starts_with(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL)));
}

#[test]
fn observation_off_serializes_as_absence_and_never_registers_token_metrics() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "known-model", false);
    ctx.ensure_ai_state().request.as_mut().unwrap().usage = Arc::new(AiUsageShared::observation_disabled());
    metrics::with_local_recorder(&recorder, || {
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
    });
    let json = serde_json::to_value(ctx.ai_state().unwrap().summary.as_ref().unwrap()).unwrap();
    assert_eq!(json["usage_source"], "notObserved");
    for key in [
        "input_tokens",
        "output_tokens",
        "estimated_cost_usd",
        "cache_read_input_tokens",
        "cache_write_input_tokens",
    ] {
        assert!(json.get(key).is_none());
    }
    assert!(!handle
        .render()
        .lines()
        .any(|line| line.starts_with(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL)));
}

#[test]
fn mixed_unknown_attempt_keeps_known_metrics_but_omits_request_total_once() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let mut ctx = ai_context(AiProviderLabel::OpenAi, "known-model", false);
    for (number, source, input_tokens, output_tokens) in [
        (1, GenAiAttemptUsageSource::Reported, Some(2), Some(3)),
        (2, GenAiAttemptUsageSource::Unknown, None, None),
        (3, GenAiAttemptUsageSource::Estimated, Some(4), Some(5)),
    ] {
        ctx.ensure_ai_state().attempt_runtime.record_test_attempt(
            GenAiAttempt {
                number,
                backend: "default/fixture".to_string(),
                provider: "openai",
                credential: "primary".to_string(),
                status: Some(200),
                failure: None,
                retry_after_seconds: None,
                ttft_ms: None,
                usage_source: source,
                input_tokens,
                output_tokens,
                estimated_cost_usd: None,
            },
            AiProviderLabel::OpenAi,
        );
    }
    ctx.ensure_ai_state()
        .request
        .as_ref()
        .unwrap()
        .usage
        .observe_reported_usage(9, 10, 0, 0);
    metrics::with_local_recorder(&recorder, || {
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
        finalize_ai_usage_with_metrics_enabled(&mut ctx, true);
    });
    let summary = ctx.ai_state().unwrap().summary.as_ref().unwrap();
    assert_eq!(summary.usage_source, GenAiAttemptUsageSource::Unknown);
    assert_eq!(
        (
            summary.input_tokens,
            summary.output_tokens,
            summary.estimated_cost_usd.as_ref()
        ),
        (None, None, None)
    );
    assert_eq!(summary.attempts.len(), 3);
    assert_eq!(summary.attempts[1].input_tokens, None);
    let rendered = handle.render();
    let samples: Vec<_> = rendered
        .lines()
        .filter(|line| line.starts_with(crate::observe::metrics::registry::names::GENAI_TOKENS_TOTAL))
        .collect();
    assert_eq!(samples.len(), 4);
    for count in [2, 3, 4, 5] {
        assert!(samples.iter().any(|line| line.ends_with(&format!(" {count}"))));
    }
}
