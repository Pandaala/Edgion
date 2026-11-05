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
        ctx.restore_ai_request_baseline(&mut request).unwrap()
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
    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(request.uri.query().map(str::to_owned))
            .unwrap();
    }
    ctx.capture_ai_request_baseline(&request).unwrap();
    request.insert_header("anthropic-beta", "modified").unwrap();
    ctx.upsert_deferred_credential_mutation(
        http::HeaderName::from_static("anthropic-version"),
        Arc::new(http::HeaderValue::from_static("attempt-local")),
        None,
    );
    if !ctx.request_query.is_captured() {
        ctx.request_query
            .capture(request.uri.query().map(str::to_owned))
            .unwrap();
    }
    ctx.capture_ai_request_baseline(&request).unwrap();
    assert!(ctx.restore_ai_request_baseline(&mut request).unwrap());
    ctx.apply_deferred_credentials(&mut request).unwrap();
    assert_eq!(request.headers["anthropic-beta"], "original");
    assert!(!request.headers.contains_key("anthropic-version"));
}

#[test]
fn ordinary_context_size_stays_within_budget() {
    // Retained interim regression guard; ownership decisions are documented in
    // skills/04-review/cpu-memory/intentional-design/http-request-context-layout-budget.md.
    let budget = 1800;
    let actual = std::mem::size_of::<EdgionHttpContext>();
    assert!(
        actual < budget,
        "EdgionHttpContext size {actual} must remain below the hot-path budget {budget}"
    );
}

#[test]
fn context_size_under_budget() {
    // The delegated chain is one nullable box: 8 bytes inline, no allocation
    // and `None` for every ordinary request.
    assert_eq!(
        std::mem::size_of::<Option<Box<crate::routes::delegation::DelegatedMatch>>>(),
        std::mem::size_of::<usize>()
    );
    let ctx = EdgionHttpContext::new();
    assert!(ctx.delegated.is_none());
    let actual = std::mem::size_of::<EdgionHttpContext>();
    assert!(actual < 1800, "EdgionHttpContext size {actual} must remain below 1800");
}

#[test]
#[cfg(target_pointer_width = "64")]
fn ordinary_context_construction_is_allocation_free() {
    // Snapshot initialization is startup work; measure the production request constructor.
    let config = crate::config::default_gateway_config();
    let (ctx, allocations) =
        crate::test_allocation_counter::count_allocation_stats(|| EdgionHttpContext::with_gateway_config(config));

    assert!(ctx.wasm.is_none());
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

#[test]
#[cfg(target_pointer_width = "64")]
fn inline_request_body_policy_stays_within_audited_layout() {
    let actual = std::mem::size_of::<crate::request_body::ResolvedRequestBodyConfig>();
    // Keep the logical body cap and independently resolved storage cap inline.
    assert!(
        actual <= 48,
        "ResolvedRequestBodyConfig size {actual} exceeds the audited budget 48"
    );
}

#[test]
fn ai_baseline_projects_dirty_query_once_and_reuses_decoded_state_on_restore() {
    let mut ctx = EdgionHttpContext::new();
    let mut request =
        pingora_http::RequestHeader::build("POST", b"/entry?token=secret&name=hello%20world", None).unwrap();
    ctx.request_query
        .capture(request.uri.query().map(str::to_owned))
        .unwrap();
    ctx.request_query.remove_current_names(&["token"]).unwrap();
    let decoded = ctx.request_query.current_param("name").unwrap().as_ptr();
    ctx.capture_ai_request_baseline(&request).unwrap();
    assert!(request.uri.query().unwrap().contains("token="));
    assert!(ctx.request_query.projection_is_dirty());
    for value in ["provider=one", "provider=two"] {
        ctx.request_query.replace_raw(Some(value.into())).unwrap();
        ctx.capture_ai_request_baseline(&request).unwrap();
        ctx.request_query.mark_synced();
        assert!(ctx.restore_ai_request_baseline(&mut request).unwrap());
        assert_eq!(request.uri.query(), Some("name=hello%20world"));
        assert_eq!(ctx.query(), Some("name=hello%20world"));
        assert_eq!(ctx.request_query.current_param("name").unwrap().as_ptr(), decoded);
        assert_eq!(ctx.entry_query_param("token"), Some("secret"));
        assert!(ctx.request_query.projection_is_dirty());
    }
}
