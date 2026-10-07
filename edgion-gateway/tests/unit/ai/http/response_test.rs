use super::{provider_reset_hint, MAX_PROVIDER_RESET_HINT_BYTES};
use crate::ai::AiProviderLabel;
use http::{HeaderMap, HeaderValue};
use std::time::{Duration, SystemTime};

#[test]
fn openai_reset_headers_accept_delta_and_choose_the_later_bound() {
    let mut headers = HeaderMap::new();
    headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("250ms"));
    headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_static("2s"));
    assert_eq!(
        provider_reset_hint(&headers, AiProviderLabel::OpenAi, SystemTime::UNIX_EPOCH),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        provider_reset_hint(&headers, AiProviderLabel::OpenAiCompatible, SystemTime::UNIX_EPOCH),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        provider_reset_hint(&headers, AiProviderLabel::Anthropic, SystemTime::UNIX_EPOCH),
        None,
        "provider-specific headers must not bleed across protocols"
    );
}

#[test]
fn openai_reset_headers_accept_absolute_epoch_and_reject_unsafe_values() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut headers = HeaderMap::new();
    headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("1700000030"));
    assert_eq!(
        provider_reset_hint(&headers, AiProviderLabel::OpenAi, now),
        Some(Duration::from_secs(30))
    );

    headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("invalid"));
    headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_static("1699999999"));
    assert_eq!(provider_reset_hint(&headers, AiProviderLabel::OpenAi, now), None);

    let oversized = "1".repeat(MAX_PROVIDER_RESET_HINT_BYTES + 1);
    headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_str(&oversized).unwrap());
    assert_eq!(provider_reset_hint(&headers, AiProviderLabel::OpenAi, now), None);
}

#[tokio::test]
async fn encoded_reject_stages_json_reply_before_conversion_and_header_plugins() {
    use crate::ai::quota::{QuotaApplyResult, QuotaFuture, QuotaSink};
    use crate::ai::runtime::attempt::{AttemptScheduler, BackendCredentials};
    use crate::ai::{AiAttemptPlan, AiBackendStore, AiRequestContext, AiUsageShared, ReadyAiAttemptPlan};
    use edgion_resources::resources::edgion_plugins::AiBackendSelection;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;
    struct Sink;
    impl QuotaSink for Sink {
        fn apply_cumulative(&self, _: u64, _: u64, _: bool) -> QuotaFuture<'_> {
            Box::pin(async { Ok(QuotaApplyResult { over_limit: false }) })
        }
    }
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"POST /v1/chat/completions HTTP/1.1\r\nHost: fixture\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    let mut session = pingora_proxy::Session::new_h1(Box::new(server));
    assert!(session.read_request().await.unwrap());
    let mut ctx = crate::EdgionHttpContext::new();
    let shared = Arc::new(AiUsageShared::default());
    ctx.ensure_ai_state().request = Some(AiRequestContext {
        provider: AiProviderLabel::OpenAiCompatible,
        model: "fixture-model".to_string(),
        pricing: None,
        requested_streaming: true,
        usage: shared.clone(),
        quota_status: None,
        quota: Some(crate::ai::quota::AiQuotaRequest::start(
            Arc::new(Sink),
            1,
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(1),
        )),
    });
    let mut scheduler = AttemptScheduler::new(&[BackendCredentials { weights: &[1] }], 0).unwrap();
    scheduler.select_backend(0).unwrap();
    scheduler.select_credential(1, 0).unwrap();
    ctx.ensure_ai_state().attempt_plan = Some(AiAttemptPlan::Ready(ReadyAiAttemptPlan {
        api: edgion_resources::resources::edgion_plugins::AiApi::OpenaiChat,
        operation: crate::ai::runtime::attempt_plan::AiOperation::ChatCompletions,
        conversion: None,
        usage_observation: edgion_resources::resources::edgion_plugins::AiUsageObservation::Provider,
        body_generation: None,
        body_is_current: false,
        request_max_output_tokens: None,
        request_body: bytes::Bytes::new(),
        canonical_path: "/v1/chat/completions".to_string(),
        canonical_query: None,
        logical_model: "fixture-model".to_string(),
        requested_streaming: true,
        inject_usage_tracking: true,
        input_chars: 0,
        selection: AiBackendSelection::Weighted,
        selection_sequence: 0,
        candidates: vec![],
        scheduler,
        retry_cap_initialized: true,
        backend_snapshot: AiBackendStore::new().snapshot(),
        quota: None,
        quota_status: None,
        terminate_on_over_limit: true,
    }));
    let mut response = pingora_http::ResponseHeader::build(200, None).unwrap();
    response.append_header("content-encoding", "identity").unwrap();
    response.append_header("content-encoding", "gzip").unwrap();
    response.insert_header("content-length", "9999").unwrap();
    assert!(super::enforce_observation_encoding(
        &mut session,
        &mut ctx,
        &mut response
    ));
    assert!(
        ctx.response_denial().is_none(),
        "a staged reply must not be overwritten by an abort marker"
    );
    let (reply, body) = ctx.take_pending_response_local_reply().unwrap();
    assert_eq!(reply.status.as_u16(), 503);
    assert_eq!(reply.headers["content-type"], "application/json");
    assert!(!reply.headers.contains_key("content-encoding"));
    assert_eq!(
        reply.headers["content-length"]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        body.len()
    );
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["code"], "token_quota_unavailable");
    assert!(shared.gateway_terminated());
    assert!(shared.observation_unavailable());
    // Passive operation retains raw eligibility even if later operator hooks
    // remove the encoding header. The shared unknown state is sticky.
    response.remove_header("content-encoding");
    assert!(!super::enforce_observation_encoding(
        &mut session,
        &mut ctx,
        &mut response
    ));
    assert!(shared.observation_unavailable());
}

#[tokio::test]
async fn native_off_keeps_encoded_response_without_usage_diagnostics() {
    use crate::ai::{AiRequestContext, AiUsageShared};
    use edgion_resources::resources::edgion_plugins::AiUsageObservation;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"POST /v1/chat/completions HTTP/1.1\r\nHost: fixture\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    let mut session = pingora_proxy::Session::new_h1(Box::new(server));
    assert!(session.read_request().await.unwrap());
    let mut ctx = crate::EdgionHttpContext::new();
    let usage = Arc::new(AiUsageShared::with_observation(AiUsageObservation::Off));
    ctx.ensure_ai_state().request = Some(AiRequestContext {
        provider: AiProviderLabel::OpenAiCompatible,
        model: "fixture-model".into(),
        pricing: None,
        requested_streaming: true,
        usage: Arc::clone(&usage),
        quota_status: None,
        quota: None,
    });
    let mut response = pingora_http::ResponseHeader::build(200, None).unwrap();
    response.insert_header("content-encoding", "gzip").unwrap();
    assert!(!super::enforce_observation_encoding(
        &mut session,
        &mut ctx,
        &mut response
    ));
    assert_eq!(response.status.as_u16(), 200);
    assert_eq!(response.headers["content-encoding"], "gzip");
    assert!(!usage.observation_unavailable());
    assert_eq!(
        usage.accounting_snapshot(0, true).source,
        crate::ai::accounting::usage::GenAiAttemptUsageSource::NotObserved
    );
}
