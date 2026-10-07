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
