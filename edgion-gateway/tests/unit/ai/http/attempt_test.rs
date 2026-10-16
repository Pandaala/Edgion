//! Assertion bodies delegated from the existing proxy callback test identities.

use super::*;
use crate::ai::render_openai_error;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

pub(crate) async fn ai_predispatch_error_writes_current_request_ids_on_wire() {
    use tokio::io::AsyncReadExt;
    let proxy = EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("ai-id-test/HTTP".to_string()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(crate::observe::AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: crate::routes::http::proxy_http::PreflightHandler,
        http_route_manager: Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(crate::routes::grpc::GrpcPortRouteManager::default()),
    };
    for disable_default in [false, true] {
        let (mut client, server) = tokio::io::duplex(8192);
        client
            .write_all(b"POST /v1/chat/completions HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        assert!(session.read_request().await.unwrap());
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.trace_id.initialize(&session.req_header().headers);
        let expected = ctx.request_info.trace_id.standard_trace_id().unwrap().to_owned();
        if disable_default {
            let config = serde_json::from_value(serde_json::json!({
                "defaultId":{"enabled":false},
                "ids":[{"header":"x-ai-correlation-id","source":{"type":"traceId"}}]
            }))
            .unwrap();
            let plugin = edgion_resources::EdgionPlugins::new(
                "ai-ids",
                edgion_resources::EdgionPluginsSpec {
                    current_status: None,
                    request_plugins: Some(vec![edgion_resources::RequestFilterEntry::new(
                        edgion_resources::EdgionPlugin::RequestId(config),
                    )]),
                    ..Default::default()
                },
            );
            crate::plugins::http::compile_plugins(&plugin)
                .plugin_runtime
                .run_request_plugins(&mut session, &mut ctx)
                .await;
        }
        super::end_ai_predispatch_error(&proxy, &mut session, &mut ctx, AiProxyError::BackendUnavailable)
            .await
            .unwrap();
        let mut bytes = vec![0; 4096];
        let count = tokio::time::timeout(Duration::from_secs(1), client.read(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let wire = std::str::from_utf8(&bytes[..count]).unwrap();
        let headers = wire.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
        assert!(headers.starts_with("http/1.1 503"));
        assert!(headers.contains("content-type: application/json"));
        if disable_default {
            assert!(!headers.contains("x-trace-id:"));
            assert!(headers.contains(&format!("x-ai-correlation-id: {expected}")));
        } else {
            assert!(headers.contains(&format!("x-trace-id: {expected}")));
        }
        assert!(!headers.contains("traceparent:"));
    }
}

pub(crate) fn ai_unavailable_maps_to_fixed_openai_backend_unavailable() {
    assert_eq!(
        ai_predispatch_error(AiDispatchError::Unavailable),
        Some(AiProxyError::BackendUnavailable)
    );
    assert_eq!(
        ai_predispatch_error(AiDispatchError::Client(AiProxyError::UnsupportedParameter)),
        Some(AiProxyError::UnsupportedParameter)
    );
    assert_eq!(
        ai_predispatch_error(AiDispatchError::Client(AiProxyError::MalformedBody)),
        Some(AiProxyError::MalformedBody)
    );
    assert_eq!(ai_predispatch_error(AiDispatchError::Rejected), None);
    assert_eq!(ai_predispatch_error(AiDispatchError::InvalidConfiguration), None);
    assert_eq!(ai_predispatch_error(AiDispatchError::RequestDeadlineExceeded), None);
    assert_eq!(ai_predispatch_error(AiDispatchError::GrpcDeadlineExceeded), None);

    let (status, body) = render_openai_error(AiProxyError::BackendUnavailable);
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(status, 503);
    assert_eq!(body["error"]["type"], "api_error");
    assert_eq!(body["error"]["code"], "service_unavailable");
    assert_eq!(body["error"].as_object().unwrap().len(), 3);

    let (status, body) = render_openai_error(AiProxyError::MalformedBody);
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(status, 400);
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert_eq!(body["error"]["code"], "invalid_request_body");
}

pub(crate) fn ai_predispatch_deadlines_map_to_standard_gateway_statuses() {
    assert_eq!(
        ai_predispatch_deadline_mapping(&AiDispatchError::RequestDeadlineExceeded),
        Some((
            EdgionErrorCode::RequestTimeoutExceeded,
            "Request timeout exceeded during AI predispatch selection",
        ))
    );
    assert_eq!(
        ai_predispatch_deadline_mapping(&AiDispatchError::GrpcDeadlineExceeded),
        Some((
            EdgionErrorCode::GrpcClientDeadlineExceeded,
            "gRPC client deadline exceeded during AI predispatch selection",
        ))
    );
    assert_eq!(ai_predispatch_deadline_mapping(&AiDispatchError::Unavailable), None);
}

pub(crate) async fn post_io_predispatch_gate_returns_standard_deadline_error() {
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info
        .set_grpc_client_deadline(Instant::now() - Duration::from_secs(1));

    let response = stop_ai_predispatch_if_expired(&mut ctx)
        .await
        .expect("expired gRPC deadline must stop predispatch");

    assert_eq!(response.etype(), &ErrorType::HTTPStatus(504));
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::GrpcClientDeadlineExceeded));
}

pub(crate) fn ai_final_and_dispatch_error_clear_incoming_retry_decision() {
    use crate::ai::runtime::dispatch::AiRetryDecision;
    let retry: Result<_, ()> = Ok(AiRetryDecision::Retry);
    let final_decision: Result<_, ()> = Ok(AiRetryDecision::Final);
    let budget_denied: Result<_, ()> = Ok(AiRetryDecision::BudgetDenied);
    let dispatch_error: Result<AiRetryDecision, _> = Err(());
    assert!(ai_dispatch_should_retry(&retry));
    assert!(!ai_dispatch_should_retry(&final_decision));
    assert!(!ai_dispatch_should_retry(&budget_denied));
    assert!(!ai_dispatch_should_retry(&dispatch_error));
}

pub(crate) fn ai_failure_classification_excludes_control_flow_and_non_upstream_errors() {
    assert_eq!(
        classify_ai_proxy_failure(
            &ErrorType::HTTPStatus(503),
            &ErrorSource::Upstream,
            Some(PeerTimeoutSource::BackendRequest),
        ),
        None
    );
    assert_eq!(
        classify_ai_proxy_failure(&ErrorType::ReadError, &ErrorSource::Downstream, None),
        None
    );
    assert_eq!(
        classify_ai_proxy_failure(&ErrorType::ReadError, &ErrorSource::Upstream, None),
        Some(AiProxyFailureClass::AmbiguousUpstream)
    );
    assert_eq!(
        classify_ai_proxy_failure(&ErrorType::InternalError, &ErrorSource::Upstream, None),
        None
    );
    assert_eq!(
        classify_ai_proxy_failure(
            &ErrorType::ReadTimedout,
            &ErrorSource::Upstream,
            Some(PeerTimeoutSource::BackendRequest),
        ),
        Some(AiProxyFailureClass::BackendRequestTimeout)
    );
}

pub(crate) fn ambiguous_upstream_requires_explicit_replay_opt_in() {
    assert!(!ai_failure_wants_retry(
        AiProxyFailureClass::AmbiguousUpstream,
        true,
        false
    ));
    assert!(ai_failure_wants_retry(
        AiProxyFailureClass::AmbiguousUpstream,
        true,
        true
    ));
    assert!(ai_failure_wants_retry(
        AiProxyFailureClass::BackendRequestTimeout,
        true,
        false
    ));
    assert!(!ai_failure_wants_retry(
        AiProxyFailureClass::BackendRequestTimeout,
        false,
        true
    ));
}

pub(crate) fn proxy_fixture() -> EdgionHttpProxy {
    EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("ai-phase-test/HTTP".to_string()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(crate::observe::AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: crate::routes::http::proxy_http::PreflightHandler,
        http_route_manager: Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(crate::routes::grpc::GrpcPortRouteManager::default()),
    }
}

use crate::ai::runtime::attempt::{AttemptScheduler, BackendCredentials, DispatchFailureKind, NextStep, StopReason};
use crate::ai::{AiAttemptPlan, AiBackendStore, ReadyAiAttemptPlan};
use crate::backends::policy::backend_traffic::{EffectiveBackendTrafficPolicy, RetryBudget};
use crate::plugins::PluginRuntime;
use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
use crate::{MatchInfo, Rsc};
use edgion_resources::resources::edgion_backend_traffic_policy::{BudgetDetails, RetryConstraintConfig};
use edgion_resources::resources::edgion_plugins::AiBackendSelection;
use edgion_resources::resources::http_route::ParsedRouteRetry;
use edgion_resources::{HTTPRouteMatch, HTTPRouteRule};

fn retry_route() -> Arc<HttpRouteRuleUnit> {
    let mut rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({})).unwrap();
    rule.parsed_retry = Some(ParsedRouteRetry {
        attempts: Some(1),
        backoff: None,
        codes: Some(vec![503]),
    });
    Arc::new(HttpRouteRuleUnit {
        parent_match: None,
        session_persistence_ttl_secs: None,
        resource_key: "default/retry".to_string(),
        matched_info: MatchInfo {
            rsc: Rsc::new("HTTPRoute", "default", "retry"),
            rule_id: 0,
            match_id: 0,
            sv: 0,
            m: serde_json::from_value::<HTTPRouteMatch>(serde_json::json!({})).unwrap(),
            labels_meta: None,
            annotations_meta: None,
        },
        rule: Arc::new(rule),
        route: None,
        path_regex: None,
        parent_refs: None,
        compiled_header_regexes: Vec::new(),
        compiled_query_regexes: Vec::new(),
        has_query_matchers: false,
        specificity: MatchSpecificity {
            has_method: false,
            header_count: 0,
            query_count: 0,
            creation_ts_ms: 0,
            route_key: "default/retry".to_string(),
            rule_index: 0,
            match_index: 0,
        },
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
        plugin_runtime: Arc::new(PluginRuntime::new()),
        backends: Arc::from([]),
        backend_finder: Arc::new(crate::lb::BackendSelector::new(vec![], vec![])),
    })
}

fn ready_context(retries: u8) -> EdgionHttpContext {
    let mut scheduler = AttemptScheduler::new(
        &[
            BackendCredentials { weights: &[1, 1] },
            BackendCredentials { weights: &[1] },
        ],
        retries,
    )
    .unwrap();
    scheduler.select_backend(0).unwrap();
    scheduler.select_credential(u16::MAX, 0).unwrap();
    let plan = ReadyAiAttemptPlan {
        api: edgion_resources::resources::edgion_plugins::AiApi::OpenaiChat,
        operation: crate::ai::runtime::attempt_plan::AiOperation::ChatCompletions,
        conversion: None,
        usage_observation: edgion_resources::resources::edgion_plugins::AiUsageObservation::Provider,
        observation_unavailable: false,
        body_generation: None,
        body_is_current: false,
        continuation: false,
        request_max_output_tokens: None,
        request_body: Bytes::from_static(br#"{"model":"logical"}"#),
        canonical_path: "/v1/chat/completions".to_string(),
        canonical_query: None,
        logical_model: "logical".to_string(),
        requested_streaming: false,
        inject_usage_tracking: false,
        input_chars: 19,
        selection: AiBackendSelection::Weighted,
        selection_sequence: 0,
        candidates: Vec::new(),
        scheduler,
        retry_cap_initialized: true,
        backend_snapshot: AiBackendStore::new().snapshot(),
        quota: None,
        quota_status: None,
        terminate_on_over_limit: false,
    };
    let mut ctx = EdgionHttpContext::new();
    ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(plan));
    ctx.route_unit = Some(retry_route());
    ctx
}

fn scheduler(ctx: &EdgionHttpContext) -> &AttemptScheduler {
    let Some(AiAttemptPlan::Ready(plan)) = ctx.ai_state().and_then(|state| state.attempt_plan.as_ref()) else {
        panic!("ready plan must remain installed")
    };
    &plan.scheduler
}

fn transport(relay_allows_retry: bool) -> RetryTransportFacts {
    RetryTransportFacts {
        relay_allows_retry,
        final_response_written: false,
    }
}

fn install_zero_retry_budget(ctx: &mut EdgionHttpContext) {
    let budget = Arc::new(RetryBudget::from_config(&RetryConstraintConfig {
        budget: BudgetDetails {
            percent: 0,
            interval: "1h".to_string(),
        },
        min_retry_rate: None,
    }));
    ctx.ensure_ai_state().effective_policy = Some(Arc::new(EffectiveBackendTrafficPolicy {
        ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
        resolved_target_class:
            edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Ai,
        winning_policy_key: "default/policy".to_string(),
        load_balancer: None,
        degrade_threshold_override: None,
        upstream_authority: None,
        upstream_authority_config: None,
        retry_budget: Some(budget),
        retry_constraint: None,
        circuit_breaker_limit: None,
        connect_timeout: None,
        outlier_detection: None,
        ai_credential_outlier: None,
        logical_circuit_breaker: None,
    }));
}

#[test]
fn raw_status_retry_is_acknowledged_without_a_second_transition() {
    let mut ctx = ready_context(1);
    crate::ai::runtime::dispatch::mark_ai_dispatch_started(&mut ctx).unwrap();
    let mut response = ResponseHeader::build(503, None).unwrap();
    let error = on_raw_response(&mut ctx, &mut response, transport(true)).unwrap_err();
    assert!(ctx.retry_on_status);
    assert_eq!(scheduler(&ctx).dispatch_count(), 1);
    assert_eq!(scheduler(&ctx).failures().len(), 1);
    let next = scheduler(&ctx).next_step();
    let acknowledged = on_transport_failure(&mut ctx, transport(true), error);
    assert!(acknowledged.retry());
    assert!(!ctx.retry_on_status);
    assert_eq!(scheduler(&ctx).next_step(), next);
    assert_eq!(scheduler(&ctx).failures().len(), 1);
    assert_eq!(scheduler(&ctx).dispatch_count(), 1);
}

#[test]
fn raw_status_replay_veto_denies_retry_without_another_dispatch() {
    let mut ctx = ready_context(1);
    crate::ai::runtime::dispatch::mark_ai_dispatch_started(&mut ctx).unwrap();
    let mut response = ResponseHeader::build(503, None).unwrap();
    on_raw_response(&mut ctx, &mut response, transport(false)).unwrap();
    assert!(!ctx.retry_on_status);
    assert_eq!(scheduler(&ctx).next_step(), NextStep::Stop(StopReason::RetryDenied));
    assert_eq!(scheduler(&ctx).dispatch_count(), 1);
    assert_eq!(scheduler(&ctx).failures().len(), 1);
}

#[test]
fn ambiguous_transport_failure_clears_decided_retry_without_opt_in() {
    let mut ctx = ready_context(1);
    crate::ai::runtime::dispatch::mark_ai_dispatch_started(&mut ctx).unwrap();
    let mut error = Error::new_up(ErrorType::ReadError);
    error.set_retry(true);
    let error = on_transport_failure(&mut ctx, transport(true), error);
    assert!(!error.retry());
    assert_eq!(
        scheduler(&ctx).next_step(),
        NextStep::Stop(StopReason::AmbiguousFailure)
    );
    assert_eq!(scheduler(&ctx).failures()[0].kind, DispatchFailureKind::Ambiguous);
}

#[test]
fn configured_backend_timeout_retry_is_distinct_from_ambiguous_failure() {
    let mut ctx = ready_context(1);
    ctx.peer_timeout_source = Some(PeerTimeoutSource::BackendRequest);
    crate::ai::runtime::dispatch::mark_ai_dispatch_started(&mut ctx).unwrap();
    let error = on_transport_failure(&mut ctx, transport(true), Error::new_up(ErrorType::ReadTimedout));
    assert!(error.retry());
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::BackendRequestTimeoutRetry));
    assert_eq!(scheduler(&ctx).failures()[0].kind, DispatchFailureKind::Retryable);
    assert_eq!(scheduler(&ctx).dispatch_count(), 1);
}

#[test]
fn raw_status_budget_denial_returns_503_and_preserves_dispatch_count() {
    let mut ctx = ready_context(1);
    install_zero_retry_budget(&mut ctx);
    crate::ai::runtime::dispatch::mark_ai_dispatch_started(&mut ctx).unwrap();
    let mut response = ResponseHeader::build(503, None).unwrap();
    let error = on_raw_response(&mut ctx, &mut response, transport(true)).unwrap_err();
    assert_eq!(error.etype(), &ErrorType::HTTPStatus(503));
    assert!(!ctx.retry_on_status);
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::RetryBudgetExhausted));
    assert_eq!(scheduler(&ctx).next_step(), NextStep::Stop(StopReason::RetryDenied));
    assert_eq!(scheduler(&ctx).dispatch_count(), 1);
}

#[test]
fn absent_ai_connect_cleanup_keeps_ordinary_context_allocation_free() {
    let mut ctx = EdgionHttpContext::new();
    let mut error = Error::new_up(ErrorType::ConnectError);
    let (_, stats) =
        crate::test_allocation_counter::count_allocation_stats(|| on_connect_failure(&mut ctx, false, &mut error));
    assert_eq!(stats.calls, 0);
    assert!(ctx.ai_state().is_none());
    assert!(!error.retry());
}
