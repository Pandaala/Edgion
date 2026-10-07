use std::sync::Arc;

use super::*;
use crate::ai::{AiAttemptPlan, AiPlanError, AiProviderLabel, AiRequestContext, AiUsageShared};

fn context(model: &str) -> AiRequestContext {
    AiRequestContext {
        provider: AiProviderLabel::OpenAi,
        model: model.to_string(),
        pricing: None,
        requested_streaming: false,
        usage: Arc::new(AiUsageShared::default()),
        quota: None,
        quota_status: None,
    }
}

#[test]
fn first_ai_context_writer_wins() {
    let mut ctx = EdgionHttpContext::new();
    assert!(ctx.ai_state().is_none());
    assert!(ctx.try_set_ai_request_context(context("model-a")));
    assert!(!ctx.try_set_ai_request_context(context("model-b")));
    assert_eq!(
        ctx.ai_state()
            .and_then(|state| state.request.as_ref())
            .map(|ai| ai.model.as_str()),
        Some("model-a")
    );
}

#[test]
fn first_ai_attempt_plan_writer_wins() {
    let mut ctx = EdgionHttpContext::new();
    assert!(ctx.try_set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend)));
    assert!(!ctx.try_set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidRouteConfiguration,)));
    assert!(matches!(
        ctx.ai_state().and_then(|state| state.attempt_plan.as_ref()),
        Some(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend))
    ));
}

#[test]
fn ordinary_context_keeps_ai_state_unallocated() {
    let ctx = EdgionHttpContext::new();
    assert!(ctx.ai_state().is_none());
}

#[test]
fn absent_ai_baseline_restore_keeps_ordinary_context_allocation_free() {
    let mut ctx = EdgionHttpContext::new();
    let mut request = pingora_http::RequestHeader::build("POST", b"/v1/messages", None).unwrap();
    let (restored, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.restore_deferred_credential_mutations(None);
        ctx.restore_ai_request_baseline(&mut request)
    });
    assert!(!restored);
    assert!(ctx.ai_state().is_none());
    assert!(ctx.deferred_credential_mutations().is_none());
    assert_eq!(allocations.calls, 0);
}

#[test]
fn ai_baseline_captures_once_and_clears_attempt_local_deferred_headers() {
    let mut ctx = EdgionHttpContext::new();
    let mut request = pingora_http::RequestHeader::build("POST", b"/v1/messages", None).unwrap();
    request.insert_header("anthropic-beta", "original").unwrap();
    ctx.capture_ai_request_baseline(&request);
    request.insert_header("anthropic-beta", "modified").unwrap();
    ctx.upsert_deferred_credential_mutation(
        http::HeaderName::from_static("anthropic-version"),
        Arc::new(http::HeaderValue::from_static("attempt-local")),
        None,
    );
    ctx.capture_ai_request_baseline(&request);
    assert!(ctx.restore_ai_request_baseline(&mut request));
    ctx.apply_deferred_credentials(&mut request).unwrap();
    assert_eq!(request.headers["anthropic-beta"], "original");
    assert!(!request.headers.contains_key("anthropic-version"));
}

#[test]
fn ordinary_context_size_stays_within_budget() {
    assert!(
        std::mem::size_of::<EdgionHttpContext>() <= 1280,
        "EdgionHttpContext grew beyond the ordinary-request hot-path size budget"
    );
}

#[test]
#[cfg(target_pointer_width = "64")]
fn ordinary_context_construction_is_allocation_free() {
    let (ctx, allocations) = crate::test_allocation_counter::count_allocation_stats(EdgionHttpContext::new);

    assert!(ctx.gateway_info.is_none());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn client_api_capture_is_first_writer_and_survives_a_rejected_plan() {
    use edgion_resources::resources::edgion_plugins::AiApi;
    let mut ctx = EdgionHttpContext::new();
    assert!(ctx.ai_state().is_none());
    assert!(ctx.capture_ai_client_api(AiApi::AnthropicMessages));
    assert!(ctx.capture_ai_client_api(AiApi::AnthropicMessages));
    assert!(!ctx.capture_ai_client_api(AiApi::OpenaiChat));
    assert!(ctx.try_set_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend)));
    let (status, body) =
        crate::ai::protocols::error::render_ai_error(&ctx, crate::ai::AiProxyError::BackendUnavailable);
    assert_eq!(status, 503);
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "overloaded_error");
    assert!(ctx.ai_state().unwrap().request.is_none());
}
