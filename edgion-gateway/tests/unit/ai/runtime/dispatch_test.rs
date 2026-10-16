use std::collections::{BTreeSet, HashMap, HashSet};

use super::*;
use serde_json::Value;

use crate::ai::runtime::attempt::{AttemptScheduler, BackendCredentials};
use crate::ai::{
    AiBackendStore, AiCandidateModel, AiPlanCandidate, ReadyAiAttemptPlan, RuntimeAiBackend, RuntimeAiCredential,
    RuntimeAiCredentialPool, RuntimeAiEndpoint,
};
use crate::backends::policy::backend_traffic::{EffectiveBackendTrafficPolicy, RetryBudget};
use crate::plugins::PluginRuntime;
use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
use crate::{MatchInfo, Rsc};
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_backend::AiCredentialLimits;
use edgion_resources::resources::edgion_backend_traffic_policy::{BudgetDetails, RetryConstraintConfig};
use edgion_resources::resources::edgion_plugins::{AiBackendSelection, OnRedisFailure};
use edgion_resources::{HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule, ParsedRouteTimeouts, RouteTimeout};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

fn request_max_tokens(body: &[u8]) -> Option<u64> {
    request::metadata(body).ok()?.output_bound
}

fn effective_max_output_tokens(body: &[u8], service_max: Option<u32>) -> Result<u64, AiDispatchError> {
    Ok(request::metadata(body)
        .map_err(AiDispatchError::Client)?
        .output_bound
        .or(service_max.map(u64::from))
        .unwrap_or(0))
}

fn candidate_scheduler() -> AttemptScheduler {
    let mut scheduler = AttemptScheduler::new(&[BackendCredentials { weights: &[1] }], 0).unwrap();
    scheduler.select_backend(0).unwrap();
    scheduler.select_credential(1, 0).unwrap();
    scheduler
}

fn candidate_plan() -> ReadyAiAttemptPlan {
    ReadyAiAttemptPlan {
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
        canonical_query: Some("trace=1".to_string()),
        logical_model: "logical".to_string(),
        requested_streaming: false,
        inject_usage_tracking: true,
        input_chars: 19,
        selection: AiBackendSelection::Weighted,
        selection_sequence: 0,
        candidates: Vec::new(),
        scheduler: candidate_scheduler(),
        retry_cap_initialized: true,
        backend_snapshot: AiBackendStore::new().snapshot(),
        quota: None,
        quota_status: None,
        terminate_on_over_limit: false,
    }
}

fn candidate_plan_with_retries(retries: u8) -> ReadyAiAttemptPlan {
    let mut plan = candidate_plan();
    plan.scheduler = AttemptScheduler::new(&[BackendCredentials { weights: &[1, 1] }], retries).unwrap();
    plan.scheduler.select_backend(0).unwrap();
    plan.scheduler.select_credential(u16::MAX, 0).unwrap();
    plan
}

fn retry_policy(percent: u8) -> (Arc<EffectiveBackendTrafficPolicy>, Arc<RetryBudget>) {
    let budget = Arc::new(RetryBudget::from_config(&RetryConstraintConfig {
        budget: BudgetDetails {
            percent,
            interval: "1h".to_string(),
        },
        min_retry_rate: None,
    }));
    let policy = Arc::new(EffectiveBackendTrafficPolicy {
        ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
        resolved_target_class:
            edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Ai,
        winning_policy_key: "default/policy".to_string(),
        load_balancer: None,
        degrade_threshold_override: None,
        upstream_authority: None,
        upstream_authority_config: None,
        retry_budget: Some(Arc::clone(&budget)),
        retry_constraint: None,
        circuit_breaker_limit: None,
        connect_timeout: None,
        outlier_detection: None,
        ai_credential_outlier: None,
        logical_circuit_breaker: None,
    });
    (policy, budget)
}

fn selectable_plan(credentials: Vec<RuntimeAiCredential>) -> ReadyAiAttemptPlan {
    let weights: Vec<u32> = credentials.iter().map(|credential| credential.weight).collect();
    let aliases: Vec<String> = credentials.iter().map(|credential| credential.name.clone()).collect();
    let outlier_store = crate::ai::AiCredentialOutlierStore::new();
    let target = edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKey {
        target_kind: edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKind::EdgionBackend,
        namespace: "default".to_string(),
        name: "provider".to_string(),
    };
    outlier_store.configure_backend(target.clone(), None, aliases.clone());
    let outlier_service = outlier_store.current_handle(&target).unwrap();
    let credential_outliers = aliases.iter().map(|alias| outlier_service.capture(alias)).collect();
    let backend = Arc::new(RuntimeAiBackend {
        has_credential_tpm: false,
        declared_credential_count: 1,
        uid: "backend-uid".to_string(),
        provider: AiProvider::OpenAICompatible,
        endpoint: RuntimeAiEndpoint {
            domain: "provider.example".to_string(),
            port: 443,
            use_tls: true,
            sni: "provider.example".to_string(),
            host: "provider.example".to_string(),
            path_prefix: String::new(),
        },
        streaming: true,
        max_output_tokens: Some(256),
        credential_pool: RuntimeAiCredentialPool {
            redis_ref: None,
            on_redis_failure: OnRedisFailure::Deny,
            credentials,
            limiter_core: Arc::new(crate::plugins::common::redis_rate_limiter::RedisRateLimiter::new()),
        },
        models: HashMap::new(),
        wildcard_models: Vec::new(),
        canonical_models: HashSet::new(),
        public_model_ids: BTreeSet::new(),
    });
    let mut plan = candidate_plan();
    plan.scheduler = AttemptScheduler::new(&[BackendCredentials { weights: &weights }], 0).unwrap();
    plan.candidates = vec![AiPlanCandidate {
        policy_ancestor_admitted: true,
        logical: compiled_ai_logical(None),
        slot: 0,
        order: 0,
        weight: 1,
        backend_key: "default/provider".to_string(),
        backend: Some(backend),
        model: Some(AiCandidateModel {
            provider_model: "provider-model".to_string(),
            pricing: None,
        }),
        effective_policy: None,
        backend_tls_policy: crate::backends::policy::BackendTlsPolicyLookup::Absent,
        adaptive: None,
        credential_outliers,
    }];
    plan
}

fn compiled_ai_logical(ref_denied: Option<RefDenied>) -> crate::backends::LogicalBackendHandle {
    let compiled = crate::routes::http::match_unit::compile_http_backend_fixture(
        HTTPBackendRef {
            name: "provider".to_string(),
            namespace: Some("default".to_string()),
            port: None,
            weight: Some(1),
            group: Some("edgion.io".to_string()),
            kind: Some("EdgionBackend".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied,
        },
        "default",
        0,
        Arc::new(PluginRuntime::new()),
    );
    crate::backends::LogicalBackendHandle::new(Arc::from(vec![compiled]), 0).unwrap()
}

fn denied_route_unit() -> Arc<HttpRouteRuleUnit> {
    let schema = HTTPBackendRef {
        name: "provider".to_string(),
        namespace: Some("tenant-b".to_string()),
        port: None,
        weight: Some(1),
        group: Some("edgion.io".to_string()),
        kind: Some("EdgionBackend".to_string()),
        filters: None,
        backend_tls_policy: None,
        ref_denied: Some(RefDenied {
            target_namespace: "tenant-b".to_string(),
            target_name: "provider".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        }),
    };
    let backend = crate::routes::http::match_unit::compile_http_backend_fixture(
        schema.clone(),
        "tenant-a",
        0,
        Arc::new(PluginRuntime::new()),
    );
    Arc::new(HttpRouteRuleUnit {
        parent_match: None,
        session_persistence_ttl_secs: None,
        resource_key: "tenant-a/route".to_string(),
        matched_info: MatchInfo {
            rsc: Rsc::new("HTTPRoute", "tenant-a", "route"),
            rule_id: 0,
            match_id: 0,
            sv: 0,
            m: HTTPRouteMatch {
                path: None,
                headers: None,
                query_params: None,
                method: None,
            },
            labels_meta: None,
            annotations_meta: None,
        },
        rule: Arc::new(HTTPRouteRule {
            resolved_parent_match: None,
            name: None,
            resolved_terminal_route_uid: None,
            resolved_ai_admission: None,
            resolved_terminal_rule_identity: None,
            matches: None,
            filters: None,
            backend_refs: Some(vec![schema]),
            timeouts: None,
            retry: None,
            session_persistence: None,
            parsed_timeouts: None,
            parsed_retry: None,
            parsed_forward_raw_path: None,
            parsed_allow_non_idempotent_retry: None,
            parsed_mirror_tuning: None,
        }),
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
            route_key: "tenant-a/route".to_string(),
            rule_index: 0,
            match_index: 0,
        },
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
        plugin_runtime: Arc::new(PluginRuntime::new()),
        backends: Arc::from(vec![backend]),
        backend_finder: Arc::new(crate::lb::BackendSelector::new(vec![], vec![])),
    })
}

fn request_deadline_route_unit(timeout: Duration) -> Arc<HttpRouteRuleUnit> {
    let mut unit = (*denied_route_unit()).clone();
    let mut rule = (*unit.rule).clone();
    rule.parsed_timeouts = Some(ParsedRouteTimeouts {
        request_timeout: RouteTimeout::Duration(timeout),
        backend_request_timeout: RouteTimeout::Unset,
    });
    unit.rule = Arc::new(rule);
    Arc::new(unit)
}

async fn test_session() -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST /v1/chat/completions HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\n\r\n")
        .await
        .expect("write request");
    drop(client);
    session
        .read_request()
        .await
        .expect("read_request should succeed")
        .then_some(())
        .expect("request must be present");
    session
}

#[tokio::test]
async fn header_modifier_ai_backend_matches_native_first_equivalent_operations() {
    for (first, duplicate) in [("first", "ignored"), ("updated", "first")] {
        let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
            "backendRefs": [{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "provider",
                "filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                    "set": [
                        {"name": "X-Set", "value": first},
                        {"name": "x-set", "value": duplicate},
                        {"name": "X-Set", "value": "exact-duplicate"}
                    ],
                    "add": [
                        {"name": "X-Add", "value": first},
                        {"name": "x-add", "value": duplicate},
                        {"name": "X-Add", "value": "exact-duplicate"}
                    ],
                    "remove": ["X-Remove", "x-remove"]
                }}]
            }]
        }))
        .unwrap();
        let runtime = crate::routes::http::match_unit::compile_http_fixture(&rule, "default").unwrap();
        let backend = &runtime.backends[0];
        let mut results = Vec::new();
        for ai in [false, true] {
            let mut session = test_session().await;
            for name in ["x-set", "x-add", "x-remove"] {
                session.req_header_mut().append_header(name, "client-a").unwrap();
                session.req_header_mut().append_header(name, "client-b").unwrap();
            }
            let mut ctx = EdgionHttpContext::new();
            if ai {
                apply_backend_request_headers(&mut session, &mut ctx, backend).unwrap();
            } else {
                backend
                    .plugin_runtime
                    .run_backend_request_plugins(&mut session, &mut ctx)
                    .await;
            }
            let headers = &session.req_header().headers;
            let values = |name| {
                headers
                    .get_all(name)
                    .iter()
                    .map(|value| value.to_str().unwrap())
                    .collect::<Vec<_>>()
            };
            assert_eq!(values("x-set"), [first]);
            assert_eq!(values("x-add"), ["client-a", "client-b", first]);
            assert!(!headers.contains_key("x-remove"));
            results.push(headers.clone());
        }
        assert_eq!(results[0], results[1]);
    }
}

#[tokio::test]
async fn header_modifier_ai_backend_rejects_invalid_duplicate_and_recovers() {
    let rule_for = |duplicate: &str| {
        serde_json::from_value::<HTTPRouteRule>(serde_json::json!({
            "backendRefs": [{
                "group": "edgion.io", "kind": "EdgionBackend", "name": "provider",
                "filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
                    "set": [
                        {"name": "X-Generation", "value": "first"},
                        {"name": "x-generation", "value": duplicate}
                    ]
                }}]
            }]
        }))
        .unwrap()
    };
    let captured = crate::routes::http::match_unit::compile_http_fixture(&rule_for("ignored"), "default").unwrap();
    for duplicate in ["ignored", "invalid\r\nvalue", "recovered"] {
        let current = crate::routes::http::match_unit::compile_http_fixture(&rule_for(duplicate), "default").unwrap();
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::new();
        let result = apply_backend_request_headers(&mut session, &mut ctx, &current.backends[0]);
        if duplicate.contains('\r') {
            assert_eq!(result, Err(AiDispatchError::InvalidConfiguration));
            assert!(!session.req_header().headers.contains_key("x-generation"));
        } else {
            assert_eq!(result, Ok(()));
            assert_eq!(session.req_header().headers["x-generation"], "first");
        }
        // Previously captured requests keep their original valid generation.
        apply_backend_request_headers(&mut session, &mut ctx, &captured.backends[0]).unwrap();
        assert_eq!(session.req_header().headers["x-generation"], "first");
    }
}

#[tokio::test]
async fn header_modifier_invalid_ai_backend_is_rejected_before_header_mutation() {
    let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({
        "backendRefs": [{
            "group": "edgion.io",
            "kind": "EdgionBackend",
            "name": "provider",
            "filters": [
                {"type": "RequestHeaderModifier", "requestHeaderModifier": {
                    "set": [{"name": "x-first", "value": "must-not-install"}]
                }},
                {"type": "ResponseHeaderModifier", "responseHeaderModifier": {
                    "set": [{"name": "bad name", "value": "private-value"}]
                }}
            ]
        }]
    }))
    .unwrap();
    let runtime = crate::routes::http::match_unit::compile_http_fixture(&rule, "default").unwrap();
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::new();
    assert!(matches!(
        apply_backend_request_headers(&mut session, &mut ctx, &runtime.backends[0]),
        Err(AiDispatchError::InvalidConfiguration)
    ));
    assert!(session.req_header().headers.get("x-first").is_none());
}

#[tokio::test]
async fn dispatch_rejects_denied_backend_before_request_mutation() {
    let credential = RuntimeAiCredential {
        name: "primary".to_string(),
        weight: 1,
        credential: "must-not-install".to_string(),
        limits: None,
    };
    let mut ctx = EdgionHttpContext::new();
    ctx.route_unit = Some(denied_route_unit());
    let mut plan = selectable_plan(vec![credential]);
    plan.candidates[0].logical = compiled_ai_logical(Some(RefDenied {
        target_namespace: "default".to_string(),
        target_name: "provider".to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    }));
    plan.body_generation = ctx.request_body.generation();
    ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(plan));
    let mut session = test_session().await;

    assert!(matches!(
        prepare_first_attempt(&mut session, &mut ctx, 0, None, 0),
        Err(AiDispatchError::InvalidConfiguration)
    ));
    assert!(ctx.selected_backend.is_none());
    assert!(ctx.ai_state().unwrap().request_baseline.is_none());
    assert!(ctx.request_body.read_all().await.unwrap().is_none());
    assert!(session.req_header().headers.get("authorization").is_none());
}

#[tokio::test]
async fn dispatch_uses_candidate_handle_after_route_snapshot_changes() {
    let credential = RuntimeAiCredential {
        name: "primary".to_string(),
        weight: 1,
        credential: "secret".to_string(),
        limits: None,
    };
    let mut ctx = EdgionHttpContext::new();
    use pingora_core::protocols::http::RequestBodyBuffer;
    let (mut storage, view) = crate::request_body::BodyStorage::new(64 * 1024, 1024 * 1024);
    storage.finish().await.unwrap();
    ctx.request_body.install(view);
    // This route generation contains a denied slot. The captured plan owns
    // an earlier permitted logical handle and must not re-read this route.
    ctx.route_unit = Some(denied_route_unit());
    let mut plan = selectable_plan(vec![credential]);
    plan.body_generation = ctx.request_body.generation();
    plan.body_generation = ctx.request_body.generation();
    ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(plan));
    let mut session = test_session().await;

    let prepared = prepare_first_attempt(&mut session, &mut ctx, 0, None, 0).unwrap();

    assert_eq!(prepared.selected.logical().identity().namespace, "default");
    assert_eq!(prepared.selected.logical().identity().name, "provider");
    assert!(matches!(
        ctx.selected_backend.as_ref().and_then(SelectedHttpBackend::routed),
        Some(SelectedLogicalBackend::Ai(_))
    ));
}

#[tokio::test]
async fn prepare_first_attempt_rechecks_request_deadline_before_plan_mutation() {
    let credential = RuntimeAiCredential {
        name: "primary".to_string(),
        weight: 1,
        credential: "secret".to_string(),
        limits: None,
    };
    let mut ctx = EdgionHttpContext::new();
    ctx.route_unit = Some(request_deadline_route_unit(Duration::from_millis(1)));
    ctx.start_time = Instant::now() - Duration::from_secs(1);
    let mut plan = selectable_plan(vec![credential]);
    plan.body_generation = ctx.request_body.generation();
    plan.body_generation = ctx.request_body.generation();
    ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(plan));
    let mut session = test_session().await;

    assert!(matches!(
        prepare_first_attempt(&mut session, &mut ctx, 0, None, 0),
        Err(AiDispatchError::RequestDeadlineExceeded)
    ));
    let Some(AiAttemptPlan::Ready(plan)) = ctx.ai_state().unwrap().attempt_plan.as_ref() else {
        panic!("deadline gate must preserve the ready plan");
    };
    assert_eq!(plan.scheduler.dispatch_count(), 0);
    assert!(matches!(plan.scheduler.next_step(), NextStep::PickBackend(_)));
}

#[tokio::test]
async fn prepare_first_attempt_rechecks_grpc_deadline_before_plan_mutation() {
    let credential = RuntimeAiCredential {
        name: "primary".to_string(),
        weight: 1,
        credential: "secret".to_string(),
        limits: None,
    };
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info
        .set_grpc_client_deadline(Instant::now() - Duration::from_secs(1));
    let mut plan = selectable_plan(vec![credential]);
    plan.body_generation = ctx.request_body.generation();
    plan.body_generation = ctx.request_body.generation();
    ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(plan));
    let mut session = test_session().await;

    assert!(matches!(
        prepare_first_attempt(&mut session, &mut ctx, 0, None, 0),
        Err(AiDispatchError::GrpcDeadlineExceeded)
    ));
    let Some(AiAttemptPlan::Ready(plan)) = ctx.ai_state().unwrap().attempt_plan.as_ref() else {
        panic!("deadline gate must preserve the ready plan");
    };
    assert_eq!(plan.scheduler.dispatch_count(), 0);
    assert!(matches!(plan.scheduler.next_step(), NextStep::PickBackend(_)));
}

#[test]
fn openai_attempt_rewrites_only_provider_model() {
    let body = request::rewrite_model(
        &Bytes::from_static(br#"{"model":"logical","messages":[]}"#),
        "provider-a",
    )
    .unwrap()
    .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["model"], "provider-a");
    assert_eq!(value["messages"], serde_json::json!([]));
}

#[test]
fn anthropic_attempt_translation_is_bounded_and_native() {
    let value = serde_json::json!({
        "model": "logical",
        "messages": [{"role": "user", "content": "Hello"}]
    });
    let translated = prepare_anthropic_request(value, "claude-provider", Some(256), true, 64 * 1024).unwrap();
    let body: Value = serde_json::from_slice(&translated.body).unwrap();
    assert_eq!(body["model"], "claude-provider");
    assert_eq!(body["max_tokens"], 256);
}

#[test]
fn request_max_tokens_prefers_max_completion_tokens() {
    assert_eq!(
        request_max_tokens(br#"{"model":"logical","max_completion_tokens":128}"#),
        Some(128)
    );
    assert_eq!(request_max_tokens(br#"{"model":"logical","max_tokens":64}"#), Some(64));
    // Conflicting numeric spellings are rejected by the shared metadata owner.
    assert_eq!(
        request_max_tokens(br#"{"model":"logical","max_tokens":64,"max_completion_tokens":128}"#),
        None
    );
    assert_eq!(request_max_tokens(br#"{"model":"logical"}"#), None);
    assert_eq!(request_max_tokens(b"not json"), None);
}

#[test]
fn conflicting_request_output_bounds_are_client_errors() {
    assert_eq!(
        effective_max_output_tokens(
            br#"{"model":"logical","max_tokens":64,"max_completion_tokens":128}"#,
            None
        ),
        Err(AiDispatchError::Client(AiProxyError::MalformedBody))
    );
    assert_eq!(
        effective_max_output_tokens(
            br#"{"model":"logical","max_tokens":64,"max_completion_tokens":64}"#,
            None
        ),
        Ok(64)
    );
}

#[test]
fn cost_weighted_accepts_max_completion_tokens_only_body_without_service_max() {
    let priced = |slot: u8, output_price: u64| RouteCandidate {
        slot,
        order: slot,
        weight: 1,
        eligibility: CandidateEligibility {
            valid_ref: true,
            model_compatible: true,
            runtime_available: true,
            untried: true,
        },
        adaptive: crate::ai::routing::candidate::AdaptiveSnapshot::default(),
        cost: Some(CostSnapshot {
            input_price_micro_usd_per_million: 1_000_000,
            output_price_micro_usd_per_million: output_price,
            // No `defaults.maxOutputTokens`: the request bound is the only input.
            service_max_output_tokens: None,
        }),
    };
    let candidates = [priced(0, 8_000_000), priced(1, 1_000_000)];
    let picker = |body: &[u8]| {
        pick(
            PickerInput {
                candidates: &candidates,
                entropy: 0,
                selection_sequence: 0,
                input_tokens: 16,
                request_max_output_tokens: request_max_tokens(body),
            },
            AiBackendSelection::CostWeighted,
        )
    };
    // Before the shared parser this body produced MissingCostInput -> HTML 500.
    assert!(picker(br#"{"model":"logical","max_completion_tokens":1024}"#).is_ok());
    assert_eq!(
        picker(br#"{"model":"logical"}"#),
        Err(crate::ai::routing::candidate::PickerError::MissingCostInput)
    );
}

#[test]
fn dispatch_mark_is_idempotent() {
    let mut ctx = EdgionHttpContext::new();
    ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(candidate_plan()));
    mark_ai_dispatch_started(&mut ctx).unwrap();
    mark_ai_dispatch_started(&mut ctx).unwrap();
    let Some(AiAttemptPlan::Ready(plan)) = ctx.ai_state().unwrap().attempt_plan.as_ref() else {
        panic!("ready plan must remain installed");
    };
    assert_eq!(plan.scheduler.dispatch_count(), 1);
}

#[test]
fn dispatch_retry_budget_denies_and_counts_each_dispatch_once() {
    let mut denied = EdgionHttpContext::new();
    denied.restore_ai_attempt_plan(AiAttemptPlan::Ready(candidate_plan_with_retries(1)));
    denied.ensure_ai_state().effective_policy = Some(retry_policy(0).0);
    mark_ai_dispatch_started(&mut denied).unwrap();
    assert_eq!(
        handle_ai_transport_failure(&mut denied, true, true, false),
        Ok(AiRetryDecision::BudgetDenied)
    );
    let Some(AiAttemptPlan::Ready(plan)) = denied.ai_state().unwrap().attempt_plan.as_ref() else {
        panic!("ready plan must remain installed");
    };
    assert_eq!(
        plan.scheduler.next_step(),
        NextStep::Stop(crate::ai::runtime::attempt::StopReason::RetryDenied)
    );

    let mut sampled_once = EdgionHttpContext::new();
    sampled_once.restore_ai_attempt_plan(AiAttemptPlan::Ready(candidate_plan()));
    let (policy, budget) = retry_policy(51);
    sampled_once.ensure_ai_state().effective_policy = Some(policy);
    mark_ai_dispatch_started(&mut sampled_once).unwrap();
    mark_ai_dispatch_started(&mut sampled_once).unwrap();
    assert!(budget.try_retry(), "one sampled dispatch permits the first retry");
    assert!(
        !budget.try_retry(),
        "an idempotent dispatch mark must not enlarge the denominator"
    );
}

#[test]
fn rejected_plan_never_crosses_dispatch_boundary() {
    let mut ctx = EdgionHttpContext::new();
    ctx.restore_ai_attempt_plan(AiAttemptPlan::Rejected(crate::ai::AiPlanError::InvalidBackend));
    assert_eq!(mark_ai_dispatch_started(&mut ctx), Err(AiDispatchError::Rejected));
}

#[test]
fn anthropic_request_errors_map_to_client_facing_proxy_errors() {
    assert_eq!(
        anthropic_request_error(AnthropicRequestError::UnsupportedParameter),
        AiDispatchError::Client(AiProxyError::UnsupportedParameter)
    );
    assert_eq!(
        anthropic_request_error(AnthropicRequestError::InvalidRequestBody),
        AiDispatchError::Client(AiProxyError::MalformedBody)
    );
    assert_eq!(
        anthropic_request_error(AnthropicRequestError::MissingRequiredParameter),
        AiDispatchError::Client(AiProxyError::MissingRequiredParameter)
    );
    assert_eq!(
        anthropic_request_error(AnthropicRequestError::OutputTooLarge),
        AiDispatchError::Client(AiProxyError::RequestTooLarge)
    );
    assert_eq!(
        anthropic_request_error(AnthropicRequestError::SerializationFailed),
        AiDispatchError::Client(AiProxyError::MalformedBody)
    );
}

#[test]
fn limited_credentials_participate_in_weighted_selection() {
    let unlimited = RuntimeAiCredential {
        name: "unlimited".to_string(),
        weight: 1,
        credential: "secret-unlimited".to_string(),
        limits: None,
    };
    let limited = RuntimeAiCredential {
        name: "limited".to_string(),
        weight: 100,
        credential: "secret-limited".to_string(),
        limits: Some(AiCredentialLimits {
            requests_per_minute: Some(1),
            tokens_per_minute: None,
        }),
    };
    let mut plan = selectable_plan(vec![unlimited, limited]);
    let selected = select_target(&mut plan, 2).unwrap();
    assert_eq!(selected.credential, 1);
}

#[test]
fn limited_only_pool_is_dispatch_candidate() {
    let limited = RuntimeAiCredential {
        name: "limited".to_string(),
        weight: 1,
        credential: "secret-limited".to_string(),
        limits: Some(AiCredentialLimits {
            requests_per_minute: Some(1),
            tokens_per_minute: None,
        }),
    };
    let mut plan = selectable_plan(vec![limited]);
    assert_eq!(select_target(&mut plan, 0).unwrap().credential, 0);
}

#[test]
fn target_selection_reports_unavailable_when_every_candidate_is_exhausted() {
    let credential = || RuntimeAiCredential {
        name: "primary".to_string(),
        weight: 1,
        credential: "secret-primary".to_string(),
        limits: None,
    };
    let mut no_credentials = selectable_plan(vec![credential()]);
    no_credentials.candidates[0].credential_outliers = vec![None];
    assert!(matches!(
        select_target(&mut no_credentials, 0),
        Err(AiDispatchError::Unavailable)
    ));
    assert_eq!(
        no_credentials.scheduler.next_step(),
        NextStep::Stop(crate::ai::runtime::attempt::StopReason::Unavailable)
    );
    assert_eq!(no_credentials.scheduler.dispatch_count(), 0);

    let mut incompatible_model = selectable_plan(vec![credential()]);
    incompatible_model.candidates[0].model = None;
    assert!(matches!(
        select_target(&mut incompatible_model, 0),
        Err(AiDispatchError::Unavailable)
    ));
}

#[test]
fn explicit_retry_cap_is_strict_but_global_fallback_is_bounded() {
    assert_eq!(resolve_ai_retry_cap(Some(7), 99), Ok(7));
    assert_eq!(
        resolve_ai_retry_cap(Some(8), 0),
        Err(AiDispatchError::InvalidConfiguration)
    );
    assert_eq!(resolve_ai_retry_cap(None, 99), Ok(7));
    assert_eq!(resolve_ai_retry_cap(None, 3), Ok(3));
}

#[test]
fn retry_cap_initialization_is_once_only_and_preserves_progress() {
    let credential = RuntimeAiCredential {
        name: "primary".to_string(),
        weight: 1,
        credential: "secret-primary".to_string(),
        limits: None,
    };
    let mut plan = selectable_plan(vec![credential]);
    plan.retry_cap_initialized = false;
    initialize_retry_cap(&mut plan, 3).unwrap();
    assert_eq!(plan.scheduler.max_dispatches(), 4);
    plan.scheduler.select_backend(0).unwrap();
    plan.scheduler.select_credential(1, 0).unwrap();
    plan.scheduler.start_dispatch().unwrap();

    initialize_retry_cap(&mut plan, 0).unwrap();
    assert_eq!(plan.scheduler.max_dispatches(), 4);
    assert_eq!(plan.scheduler.dispatch_count(), 1);
}

#[test]
fn endpoint_predispatch_failure_releases_and_skips_the_whole_service() {
    let first_weights = [1, 1];
    let second_weights = [1];
    let mut plan = candidate_plan();
    plan.scheduler = AttemptScheduler::new(
        &[
            BackendCredentials {
                weights: &first_weights,
            },
            BackendCredentials {
                weights: &second_weights,
            },
        ],
        0,
    )
    .unwrap();
    plan.scheduler.select_backend(0).unwrap();
    plan.scheduler.select_credential(u16::MAX, 0).unwrap();
    let mut ctx = EdgionHttpContext::new();
    plan.body_generation = ctx.request_body.generation();
    ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(plan));

    assert!(AiPeerPredispatchFailure::Endpoint.finish(&mut ctx));
    let Some(AiAttemptPlan::Ready(ready)) = ctx.ensure_ai_state().attempt_plan.as_mut() else {
        panic!("ready plan must remain installed");
    };
    assert!(matches!(ready.scheduler.next_step(), NextStep::PickBackend(_)));
    assert_eq!(
        ready.scheduler.select_backend(0),
        Err(crate::ai::runtime::attempt::SchedulerError::BackendAlreadyExcluded)
    );
    assert!(ready.scheduler.select_backend(1).is_ok());
    assert_eq!(ready.scheduler.dispatch_count(), 0);
}

#[test]
fn ambiguous_failure_replays_only_when_explicitly_requested() {
    let mut denied_ctx = EdgionHttpContext::new();
    denied_ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(candidate_plan_with_retries(1)));
    mark_ai_dispatch_started(&mut denied_ctx).unwrap();
    assert_eq!(
        handle_ai_transport_failure(&mut denied_ctx, false, true, true),
        Ok(AiRetryDecision::Final)
    );
    let Some(AiAttemptPlan::Ready(denied)) = denied_ctx.ai_state().unwrap().attempt_plan.as_ref() else {
        panic!("ready plan must remain installed");
    };
    assert_eq!(
        denied.scheduler.next_step(),
        NextStep::Stop(crate::ai::runtime::attempt::StopReason::AmbiguousFailure)
    );
    assert_eq!(denied.scheduler.failures()[0].kind, DispatchFailureKind::Ambiguous);

    let mut allowed_ctx = EdgionHttpContext::new();
    allowed_ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(candidate_plan_with_retries(1)));
    mark_ai_dispatch_started(&mut allowed_ctx).unwrap();
    assert_eq!(
        handle_ai_transport_failure(&mut allowed_ctx, true, true, true),
        Ok(AiRetryDecision::Retry)
    );
    let Some(AiAttemptPlan::Ready(allowed)) = allowed_ctx.ai_state().unwrap().attempt_plan.as_ref() else {
        panic!("ready plan must remain installed");
    };
    assert!(matches!(allowed.scheduler.next_step(), NextStep::PickCredential(_)));
    assert_eq!(allowed.scheduler.failures()[0].kind, DispatchFailureKind::Ambiguous);
}

#[tokio::test]
async fn composite_ai_processor_stops_its_inner_chain_on_terminate() {
    // AiAttemptResponseProcessor wraps its own Vec of sub-processors and
    // is itself a chain: an inner terminate must stop it exactly like the
    // outer driver stops the top-level chain, or a sub-processor after
    // the terminating one would silently keep running.
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use parking_lot::Mutex;

    struct Terminator;
    #[async_trait]
    impl StreamingResponseProcessor for Terminator {
        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: crate::ctx::ResponseStreamKind,
            sink: &mut ResponseBodySink,
            _session: &mut dyn crate::plugins::runtime::traits::ResponseStreamSession,
        ) {
            sink.terminate();
        }
    }

    struct MarkRan {
        ran: Arc<Mutex<bool>>,
    }
    #[async_trait]
    impl StreamingResponseProcessor for MarkRan {
        async fn on_chunk(
            &mut self,
            _chunk: &mut Option<Bytes>,
            _end_of_stream: bool,
            _stream_kind: crate::ctx::ResponseStreamKind,
            _sink: &mut ResponseBodySink,
            _session: &mut dyn crate::plugins::runtime::traits::ResponseStreamSession,
        ) {
            *self.ran.lock() = true;
        }
    }

    let ran = Arc::new(Mutex::new(false));
    let mut composite = AiAttemptResponseProcessor {
        processors: vec![Box::new(Terminator), Box::new(MarkRan { ran: ran.clone() })],
        usage: Arc::new(AiUsageShared::default()),
        eos: Arc::new(AtomicBool::new(false)),
    };
    let mut chunk = Some(Bytes::from_static(b"x"));
    let mut sink = ResponseBodySink::new();
    let mut session = MockPluginSession::new();

    composite
        .on_chunk(
            &mut chunk,
            false,
            crate::ctx::ResponseStreamKind::None,
            &mut sink,
            &mut session,
        )
        .await;

    assert!(sink.is_terminated());
    assert!(!*ran.lock(), "the sub-processor after an inner terminate must not run");
}

#[tokio::test]
async fn raw_encoding_survives_anthropic_header_replacement_and_fixed_error_body() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    let usage = Arc::new(AiUsageShared::default());
    let converter = AnthropicResponseProcessor::new_with_quota(
        "fixture-model".to_string(),
        false,
        false,
        0,
        usage.clone(),
        None,
        false,
    );
    let mut composite = AiAttemptResponseProcessor {
        processors: vec![Box::new(converter)],
        usage: usage.clone(),
        eos: Arc::new(AtomicBool::new(false)),
    };
    let mut header = ResponseHeader::build(200, None).unwrap();
    header.insert_header("content-type", "application/json").unwrap();
    header.append_header("content-encoding", "identity").unwrap();
    header.append_header("content-encoding", "gzip").unwrap();
    composite.on_response_header(&mut header);
    assert_eq!(header.status.as_u16(), 503);
    assert!(!header.headers.contains_key("content-encoding"));
    let mut body = Some(Bytes::from_static(b"encoded upstream bytes"));
    let mut sink = ResponseBodySink::new();
    let mut session = MockPluginSession::new();
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    session.expect_record_error().return_const(());
    ai.expect_semantic_guard_active().return_const(false);
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
    composite
        .on_chunk(
            &mut body,
            true,
            crate::ctx::ResponseStreamKind::None,
            &mut sink,
            &mut session,
        )
        .await;
    let snapshot = usage.accounting_snapshot(2, true);
    assert_eq!(snapshot.source, crate::ai::GenAiAttemptUsageSource::Unknown);
    assert_eq!(snapshot.usage, None);
    assert!(composite.eos.load(std::sync::atomic::Ordering::Acquire));
}

#[tokio::test]
async fn unary_admission_arbitrates_only_active_owners_before_dispatch() {
    use pingora_core::protocols::http::RequestBodyBuffer;
    let _ = crate::ai::config::initialize(&crate::ai::config::AiConfig::default());
    let runtime = crate::ai::config::runtime().unwrap();
    for guard_first in [true, false] {
        for active in [false, true] {
            let mut plan = selectable_plan(vec![RuntimeAiCredential {
                name: "primary".into(),
                weight: 1,
                credential: "fixture".into(),
                limits: None,
            }]);
            Arc::get_mut(plan.candidates[0].backend.as_mut().unwrap())
                .unwrap()
                .provider = AiProvider::Anthropic;
            plan.conversion = Some(AiConversion::ChatToAnthropic);
            plan.request_body =
                Bytes::from_static(br#"{"model":"logical","messages":[{"role":"user","content":"hello"}]}"#);
            let mut ctx = EdgionHttpContext::new();
            let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
            storage.finish().await.unwrap();
            ctx.request_body.install(view);
            let claim = crate::ctx::ResponseHeadHoldClaim {
                owner: crate::ctx::ResponseHeadHoldOwner::Guardrail,
                limits: pingora_proxy::ResponseHeadHoldLimits::new(1, 1, 1, 1, 1, 1, Duration::from_secs(1)),
                active,
            };
            if guard_first {
                ctx.ensure_ai_state().response_head_hold_claim = Some(claim);
            }
            plan.body_generation = ctx.request_body.generation();
            assert!(ctx.try_set_ai_attempt_plan(AiAttemptPlan::Ready(plan)));
            if !guard_first {
                ctx.ensure_ai_state().response_head_hold_claim = Some(claim);
            }
            assert!(ctx.ai_state().unwrap().unary_possible);
            let mut session = test_session().await;
            let result = prepare_first_attempt(&mut session, &mut ctx, 0, None, 0);
            if active {
                assert!(matches!(
                    result,
                    Err(AiDispatchError::Client(AiProxyError::BackendUnavailable))
                ));
                assert!(ctx.selected_backend.is_none());
            } else {
                let prepared = result.unwrap();
                assert!(prepared.unary_owner.is_some());
                drop(prepared);
            }
            let AiAttemptPlan::Ready(plan) = ctx.ai_state().unwrap().attempt_plan.as_ref().unwrap() else {
                panic!("ready plan retained");
            };
            assert_eq!(plan.scheduler.dispatch_count(), 0);
            assert_eq!(ctx.ai_state().unwrap().response_head_hold_claim.unwrap().active, active);
        }
    }
    let reservations: Vec<_> = (0..runtime.limits.slots)
        .map(|_| runtime.budget.acquire().unwrap())
        .collect();
    let mut plan = selectable_plan(vec![RuntimeAiCredential {
        name: "primary".into(),
        weight: 1,
        credential: "fixture".into(),
        limits: None,
    }]);
    Arc::get_mut(plan.candidates[0].backend.as_mut().unwrap())
        .unwrap()
        .provider = AiProvider::Anthropic;
    plan.conversion = Some(AiConversion::ChatToAnthropic);
    let mut ctx = EdgionHttpContext::new();
    ctx.try_set_ai_attempt_plan(AiAttemptPlan::Ready(plan));
    let mut session = test_session().await;
    assert!(matches!(
        prepare_first_attempt(&mut session, &mut ctx, 0, None, 0),
        Err(AiDispatchError::Client(AiProxyError::BackendUnavailable))
    ));
    assert!(ctx.selected_backend.is_none());
    drop(reservations);
    let mut ctx = EdgionHttpContext::new();
    let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
    storage.finish().await.unwrap();
    ctx.request_body.install(view);
    let mut plan = selectable_plan(vec![RuntimeAiCredential {
        name: "primary".into(),
        weight: 1,
        credential: "fixture".into(),
        limits: None,
    }]);
    plan.body_generation = ctx.request_body.generation();
    ctx.try_set_ai_attempt_plan(AiAttemptPlan::Ready(plan));
    ctx.ensure_ai_state().unary_possible = true;
    let prepared = prepare_first_attempt(&mut session, &mut ctx, 0, None, 0).unwrap();
    assert!(
        prepared.unary_owner.is_none(),
        "native selection in a mixed pending plan does not admit unary"
    );
}

#[tokio::test]
async fn native_off_preserves_bytes_generation_encoding_and_rejects_later_mutation() {
    use pingora_core::protocols::http::RequestBodyBuffer;
    let body = Bytes::from_static(br#" { "model" : "logical", "extensions" : 123456789012345678901234567890 } "#);
    let mut ctx = EdgionHttpContext::new();
    let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
    storage.write(&body).await.unwrap();
    storage.finish().await.unwrap();
    ctx.request_body.install(view);
    let generation = ctx.request_body.generation();
    let mut plan = selectable_plan(vec![RuntimeAiCredential {
        name: "primary".into(),
        weight: 1,
        credential: "fixture".into(),
        limits: None,
    }]);
    plan.request_body = body.clone();
    plan.body_generation = generation;
    plan.body_is_current = true;
    plan.inject_usage_tracking = false;
    plan.usage_observation = AiUsageObservation::Off;
    plan.candidates[0].model.as_mut().unwrap().provider_model = "logical".to_string();
    ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(plan));
    let mut session = test_session().await;
    session
        .req_header_mut()
        .insert_header("accept-encoding", "gzip, br")
        .unwrap();
    let mut prepared = prepare_first_attempt(&mut session, &mut ctx, 0, None, 0).unwrap();
    assert_eq!(ctx.request_body.generation(), generation);
    assert_eq!(ctx.request_body.read_all().await.unwrap().unwrap(), body);
    assert_eq!(session.req_header().headers["accept-encoding"], "gzip, br");
    assert_eq!(
        session.req_header().uri.path_and_query().unwrap().as_str(),
        "/v1/chat/completions?trace=1"
    );
    assert!(prepared.unary_owner.is_none());
    assert_eq!(
        prepared
            .ai_context
            .as_ref()
            .unwrap()
            .usage
            .accounting_snapshot(10, true)
            .source,
        crate::ai::GenAiAttemptUsageSource::NotObserved
    );
    let mut response = ResponseHeader::build(200, None).unwrap();
    response.insert_header("content-encoding", "gzip").unwrap();
    let processor = prepared.response_processor.as_mut().unwrap();
    processor.on_response_header(&mut response);
    let raw = Bytes::from_static(b"opaque compressed response");
    let mut response_chunk = Some(raw.clone());
    let mut sink = ResponseBodySink::new();
    let mut mock = crate::plugins::runtime::traits::session::MockPluginSession::new();
    processor
        .on_chunk(
            &mut response_chunk,
            true,
            crate::ctx::ResponseStreamKind::None,
            &mut sink,
            &mut mock,
        )
        .await;
    assert_eq!(response_chunk, Some(raw));
    assert!(prepared.eos.load(std::sync::atomic::Ordering::Acquire));
    assert!(!prepared.ai_context.as_ref().unwrap().usage.observation_unavailable());
    ctx.overwrite_request_body(Bytes::from_static(br#"{"model":"unauthorized"}"#))
        .unwrap();
    assert!(matches!(
        prepare_first_attempt(&mut session, &mut ctx, 0, None, 0),
        Err(AiDispatchError::Client(AiProxyError::BackendUnavailable))
    ));
}

#[tokio::test]
async fn dispatcher_owned_retry_rewrite_restores_the_original_extension_bytes() {
    use pingora_core::protocols::http::RequestBodyBuffer;
    let body = Bytes::from_static(br#" { "model" : "logical", "extensions" : 123456789012345678901234567890 } "#);
    let mut ctx = EdgionHttpContext::new();
    let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
    storage.write(&body).await.unwrap();
    storage.finish().await.unwrap();
    ctx.request_body.install(view);
    let mut plan = selectable_plan(vec![RuntimeAiCredential {
        name: "primary".into(),
        weight: 1,
        credential: "fixture".into(),
        limits: None,
    }]);
    plan.request_body = body.clone();
    plan.body_generation = ctx.request_body.generation();
    plan.body_is_current = true;
    plan.usage_observation = AiUsageObservation::Off;
    plan.scheduler = AttemptScheduler::new(&[BackendCredentials { weights: &[1] }], 1).unwrap();
    let mut session = test_session().await;
    let first = prepare_ready_attempt(&mut session, &mut ctx, &mut plan, 0).unwrap();
    drop(first);
    assert_eq!(
        ctx.request_body.read_all().await.unwrap().unwrap(),
        Bytes::from(
            String::from_utf8(body.to_vec())
                .unwrap()
                .replace("logical", "provider-model")
        )
    );
    plan.scheduler.start_dispatch().unwrap();
    plan.scheduler
        .finish_dispatch(crate::ai::runtime::attempt::DispatchOutcome::RetryableFailure)
        .unwrap();
    plan.candidates[0].model.as_mut().unwrap().provider_model = "logical".into();
    let second = prepare_ready_attempt(&mut session, &mut ctx, &mut plan, 0).unwrap();
    assert!(second.unary_owner.is_none());
    assert_eq!(ctx.request_body.read_all().await.unwrap().unwrap(), body);
    assert!(plan.body_is_current);
    assert_eq!(plan.body_generation, ctx.request_body.generation());
}

#[tokio::test]
async fn native_deferred_protocol_headers_survive_backend_skip_and_retry() {
    use crate::ai::runtime::attempt::DispatchOutcome;
    use crate::ai::runtime::attempt_runtime::AiAttemptAdmission;
    use crate::ai::runtime::circuit_breaker::LogicalBackendCircuitBreaker;
    use pingora_core::protocols::http::RequestBodyBuffer;
    for operation in ["set", "add", "remove"] {
        for transition in ["skip_backend", "skip_credential", "retry"] {
            let modifier = if operation == "remove" {
                serde_json::json!({"remove":["anthropic-version", "anthropic-beta"]})
            } else {
                let values = serde_json::json!([
                    {"name":"anthropic-version", "value":"2023-06-01"},
                    {"name":"anthropic-beta", "value":"candidate-a-beta"}
                ]);
                serde_json::json!({(operation): values})
            };
            let rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({"backendRefs":[
                {"group":"edgion.io", "kind":"EdgionBackend", "name":"provider-a", "filters":[{"type":"RequestHeaderModifier", "requestHeaderModifier":modifier}]},
                {"group":"edgion.io", "kind":"EdgionBackend", "name":"provider-b"}
            ]})).unwrap();
            let owner: edgion_resources::HTTPRoute = serde_json::from_value(serde_json::json!({"apiVersion":"gateway.networking.k8s.io/v1", "kind":"HTTPRoute", "metadata":{"namespace":"default","name":"test-route"}, "spec":{"rules":[rule]}})).unwrap();
            let compiled = HttpRouteRuleUnit::compile_http_rule(&owner, 0).unwrap();
            let mut plan = selectable_plan(vec![RuntimeAiCredential {
                name: "primary".into(),
                weight: 1,
                credential: "managed-test-key".into(),
                limits: None,
            }]);
            Arc::get_mut(plan.candidates[0].backend.as_mut().unwrap())
                .unwrap()
                .provider = AiProvider::Anthropic;
            plan.candidates[0].model.as_mut().unwrap().provider_model = "logical".into();
            let mut second = plan.candidates[0].clone();
            second.credential_outliers = selectable_plan(vec![RuntimeAiCredential {
                name: "primary".into(),
                weight: 1,
                credential: "managed-test-key".into(),
                limits: None,
            }])
            .candidates
            .remove(0)
            .credential_outliers;
            plan.candidates[0].logical =
                crate::backends::LogicalBackendHandle::new(compiled.backends.clone(), 0).unwrap();
            second.logical = crate::backends::LogicalBackendHandle::new(compiled.backends.clone(), 1).unwrap();
            second.slot = 1;
            second.order = 1;
            second.backend_key = "default/provider-b".into();
            plan.candidates.push(second);
            plan.api = AiApi::AnthropicMessages;
            plan.operation = AiOperation::Messages;
            plan.canonical_path = "/v1/messages".into();
            plan.usage_observation = AiUsageObservation::Off;
            plan.selection = AiBackendSelection::Priority;
            plan.scheduler = AttemptScheduler::new(
                &[
                    BackendCredentials { weights: &[1] },
                    BackendCredentials { weights: &[1] },
                ],
                1,
            )
            .unwrap();
            let body = Bytes::from_static(br#"{"model":"logical","max_tokens":10,"messages":[]}"#);
            plan.request_body = body.clone();
            let mut ctx = EdgionHttpContext::new();
            let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
            storage.write(&body).await.unwrap();
            storage.finish().await.unwrap();
            ctx.request_body.install(view);
            plan.body_generation = ctx.request_body.generation();
            plan.body_is_current = true;
            let deadline = Instant::now().checked_add(Duration::from_secs(60)).unwrap();
            for (name, value) in [
                ("anthropic-version", "2099-01-01"),
                ("anthropic-beta", "original-beta"),
                ("x-api-key", "caller-test-key"),
                ("anthropic-workspace-id", "caller-workspace"),
            ] {
                let mut value = http::HeaderValue::from_static(value);
                value.set_sensitive(true);
                ctx.upsert_deferred_credential_mutation(
                    http::HeaderName::from_static(name),
                    Arc::new(value),
                    Some(deadline),
                );
            }
            let mut session = test_session().await;
            assert!(ctx.try_set_ai_attempt_plan(AiAttemptPlan::Ready(plan)));
            let first = prepare_first_attempt(&mut session, &mut ctx, 0, Some(1), 1).unwrap();
            assert_eq!(first.selected.logical().identity().name, "provider-a");
            if transition == "retry" {
                let mut saved = ctx.take_ai_attempt_plan().unwrap();
                let AiAttemptPlan::Ready(ready) = &mut saved else {
                    panic!("ready plan");
                };
                ready.scheduler.start_dispatch().unwrap();
                ready
                    .scheduler
                    .finish_dispatch(DispatchOutcome::RetryableFailure)
                    .unwrap();
                ctx.restore_ai_attempt_plan(saved);
            } else {
                let breaker = LogicalBackendCircuitBreaker::new(1);
                let _occupied = breaker.try_acquire().unwrap();
                if transition == "skip_credential" {
                    first
                        .selected
                        .credential_outlier
                        .as_ref()
                        .unwrap()
                        .apply_cooldown(Duration::from_secs(60));
                }
                let mut input = first.admission_input().unwrap();
                input.circuit_breaker = (transition == "skip_backend").then_some(breaker);
                let admission = ctx.ensure_ai_state().attempt_runtime.admit(input).await;
                if transition == "skip_backend" {
                    assert!(matches!(admission, AiAttemptAdmission::SkipBackend));
                    assert!(handle_ai_predispatch_service_failure(&mut ctx));
                } else {
                    assert!(matches!(admission, AiAttemptAdmission::Skip));
                    assert!(handle_ai_predispatch_failure(&mut ctx, true));
                }
            }
            let second = prepare_first_attempt(&mut session, &mut ctx, 0, Some(1), 1).unwrap();
            assert_eq!(second.selected.logical().identity().name, "provider-b");
            let mut upstream = session.req_header().clone();
            ctx.apply_deferred_credentials(&mut upstream).unwrap();
            assert_eq!(
                upstream.headers["anthropic-version"], "2099-01-01",
                "{operation}/{transition}"
            );
            assert_eq!(
                upstream.headers["anthropic-beta"], "original-beta",
                "{operation}/{transition}"
            );
            assert!(upstream.headers["anthropic-version"].is_sensitive());
            assert!(upstream.headers["anthropic-beta"].is_sensitive());
            assert!(ctx.deferred_credentials_all_unexpired(deadline - Duration::from_millis(1)));
            assert!(!ctx.deferred_credentials_all_unexpired(deadline));
            assert_eq!(upstream.headers["x-api-key"], "managed-test-key");
            assert!(!upstream.headers.contains_key("anthropic-workspace-id"));
        }
    }
}

#[test]
fn responses_continuation_retry_cap_cannot_be_reinitialized_from_global_policy() {
    let credential = RuntimeAiCredential {
        name: "primary".into(),
        weight: 1,
        credential: "fixed-test-key".into(),
        limits: None,
    };
    let mut plan = selectable_plan(vec![credential]);
    plan.api = AiApi::OpenaiResponses;
    plan.operation = AiOperation::Responses;
    plan.continuation = true;
    plan.retry_cap_initialized = false;
    initialize_retry_cap(&mut plan, 7).unwrap();
    assert_eq!(plan.scheduler.max_dispatches(), 1);
    initialize_retry_cap(&mut plan, 7).unwrap();
    assert_eq!(plan.scheduler.max_dispatches(), 1);
}

#[tokio::test]
async fn native_responses_dispatch_strips_client_and_backend_principal_selectors() {
    use pingora_core::protocols::http::RequestBodyBuffer;
    let rule:HTTPRouteRule=serde_json::from_value(serde_json::json!({"backendRefs":[{"group":"edgion.io","kind":"EdgionBackend","name":"provider","filters":[{"type":"RequestHeaderModifier","requestHeaderModifier":{"set":[{"name":"OpenAI-Organization","value":"org-backend-selector"},{"name":"openai-project","value":"proj-backend-selector"},{"name":"x-native-extension","value":"preserved"}]}}]}]})).unwrap();
    let compiled = crate::routes::http::match_unit::compile_http_fixture(&rule, "default").unwrap();
    for continuation in [false, true] {
        let mut session = test_session().await;
        session
            .req_header_mut()
            .insert_header("OpenAI-Organization", "org-client-selector")
            .unwrap();
        session
            .req_header_mut()
            .insert_header("OPENAI-PROJECT", "proj-client-selector")
            .unwrap();
        let mut ctx = EdgionHttpContext::new();
        for (header, value) in [
            ("authorization", "Bearer deferred-selector-key"),
            ("openai-organization", "org-deferred-selector"),
            ("openai-project", "proj-deferred-selector"),
        ] {
            ctx.upsert_deferred_credential_mutation(
                http::HeaderName::from_static(header),
                Arc::new(http::HeaderValue::from_static(value)),
                None,
            );
        }
        let body=Bytes::from_static(br#" { "model":"logical", "input":"hello", "stream_options":{"include_obfuscation":false}, "extension":1e99999 } "#);
        let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
        storage.write(&body).await.unwrap();
        storage.finish().await.unwrap();
        ctx.request_body.install(view);
        let mut plan = selectable_plan(vec![RuntimeAiCredential {
            name: "primary".into(),
            weight: 1,
            credential: "fixed-test-key".into(),
            limits: None,
        }]);
        plan.api = AiApi::OpenaiResponses;
        plan.operation = AiOperation::Responses;
        plan.continuation = continuation;
        plan.inject_usage_tracking = false;
        plan.usage_observation = AiUsageObservation::Off;
        plan.request_body = body.clone();
        plan.body_generation = ctx.request_body.generation();
        plan.body_is_current = true;
        plan.canonical_path = "/v1/responses".into();
        plan.candidates[0].logical = crate::backends::LogicalBackendHandle::new(compiled.backends.clone(), 0).unwrap();
        Arc::get_mut(plan.candidates[0].backend.as_mut().unwrap())
            .expect("single test backend owner")
            .provider = AiProvider::OpenAI;
        plan.candidates[0].model.as_mut().unwrap().provider_model = "logical".into();
        let prepared = prepare_ready_attempt(&mut session, &mut ctx, &mut plan, 0).unwrap();
        assert!(prepared.unary_owner.is_none());
        for header in ["openai-organization", "openai-project"] {
            assert!(!session.req_header().headers.contains_key(header));
        }
        assert_eq!(session.req_header().headers["authorization"], "Bearer fixed-test-key");
        let mut upstream = session.req_header().clone();
        ctx.apply_deferred_credentials(&mut upstream).unwrap();
        for header in ["openai-organization", "openai-project"] {
            assert!(!upstream.headers.contains_key(header));
        }
        assert_eq!(upstream.headers["authorization"], "Bearer fixed-test-key");
        assert_eq!(upstream.headers["x-native-extension"], "preserved");
        assert_eq!(session.req_header().headers["x-native-extension"], "preserved");
        assert_eq!(session.req_header().uri.to_string(), "/v1/responses?trace=1");
        assert_eq!(ctx.request_body.read_all().await.unwrap().unwrap(), body);
    }
}

#[tokio::test]
async fn prepared_admission_projects_selected_credential_and_retained_generation() {
    use crate::ai::routing::adaptive::{AdaptiveTelemetryKey, AdaptiveTelemetryStore};
    use crate::ai::runtime::circuit_breaker::LogicalBackendCircuitBreaker;
    use crate::backends::outlier_detection::EjectionAdmission;
    use edgion_resources::resources::edgion_backend::AiModelPricing;
    use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
    use pingora_core::protocols::http::RequestBodyBuffer;

    let mut plan = selectable_plan(vec![
        RuntimeAiCredential {
            name: "unselected".into(),
            weight: 0,
            credential: "first-test-key".into(),
            limits: Some(AiCredentialLimits {
                requests_per_minute: Some(11),
                tokens_per_minute: Some(101),
            }),
        },
        RuntimeAiCredential {
            name: "selected".into(),
            weight: 1,
            credential: "second-test-key".into(),
            limits: Some(AiCredentialLimits {
                requests_per_minute: Some(22),
                tokens_per_minute: Some(202),
            }),
        },
    ]);
    let backend = Arc::get_mut(plan.candidates[0].backend.as_mut().unwrap()).unwrap();
    backend.credential_pool.redis_ref = Some("default/quota-redis".into());
    backend.credential_pool.on_redis_failure = OnRedisFailure::Allow;
    let pricing = AiModelPricing {
        input_per_million_usd: "1.25".into(),
        output_per_million_usd: "2.5".into(),
        cache_read_input_per_million_usd: None,
        cache_write_input_per_million_usd: None,
    }
    .parsed()
    .unwrap();
    plan.candidates[0].model.as_mut().unwrap().pricing = Some(pricing);
    let (mut policy, _) = retry_policy(10);
    let breaker = LogicalBackendCircuitBreaker::new(2);
    Arc::get_mut(&mut policy).unwrap().logical_circuit_breaker = Some(Arc::clone(&breaker));
    plan.candidates[0].effective_policy = Some(policy);
    let store = AdaptiveTelemetryStore::new();
    let key = AdaptiveTelemetryKey {
        terminal_route_uid: "route-uid".into(),
        rule_name: "rule".into(),
        target: PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "default".into(),
            name: "provider".into(),
        },
        logical_model: "logical".into(),
    };
    store.reconcile_keys(&HashSet::from([key.clone()]));
    let adaptive = store.get(&key).unwrap();
    plan.candidates[0].adaptive = Some(Arc::clone(&adaptive));
    let mut ctx = EdgionHttpContext::new();
    let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
    storage.write(&plan.request_body).await.unwrap();
    storage.finish().await.unwrap();
    ctx.request_body.install(view);
    plan.body_generation = ctx.request_body.generation();
    plan.body_is_current = true;
    let mut session = test_session().await;
    let prepared = prepare_ready_attempt(&mut session, &mut ctx, &mut plan, 0).unwrap();
    assert_eq!(prepared.selected.credential_index, 1);
    // A later publication cannot replace the backend or credential policy held
    // by this prepared attempt, even when the new candidate has no limits.
    plan.candidates = selectable_plan(vec![RuntimeAiCredential {
        name: "replacement".into(),
        weight: 1,
        credential: "replacement-test-key".into(),
        limits: None,
    }])
    .candidates;
    let input = prepared.admission_input().unwrap();
    assert_eq!(input.namespace, "default");
    assert_eq!(input.backend_name, "default/provider");
    assert_eq!(input.backend_uid, "backend-uid");
    assert_eq!(input.credential_alias, "selected");
    assert_eq!(input.provider, AiProviderLabel::OpenAiCompatible);
    assert_eq!(input.pricing, Some(pricing));
    assert_eq!(input.redis_ref.as_deref(), Some("default/quota-redis"));
    assert_eq!(input.on_redis_failure, OnRedisFailure::Allow);
    let limits = input.limits.as_ref().unwrap();
    assert_eq!(limits.requests_per_minute, Some(22));
    assert_eq!(limits.tokens_per_minute, Some(202));
    assert!(Arc::ptr_eq(
        &input.limiter_core,
        &prepared.selected.backend.credential_pool.limiter_core
    ));
    assert!(Arc::ptr_eq(&input.usage, &prepared.ai_context.as_ref().unwrap().usage));
    assert!(Arc::ptr_eq(&input.eos, &prepared.eos));
    assert!(Arc::ptr_eq(input.adaptive.as_ref().unwrap(), &adaptive));
    assert!(Arc::ptr_eq(input.circuit_breaker.as_ref().unwrap(), &breaker));
    prepared
        .selected
        .credential_outlier
        .as_ref()
        .unwrap()
        .apply_cooldown(Duration::from_secs(60));
    assert!(matches!(
        input.outlier.as_ref().unwrap().admit(),
        EjectionAdmission::Rejected
    ));
}

#[tokio::test]
async fn prepared_admission_preserves_token_bound_precedence_and_saturation() {
    use pingora_core::protocols::http::RequestBodyBuffer;
    for (request_bound, backend_bound, chars, expected_output, expected_input) in [
        (Some(17), Some(256), 19, 17, 5),
        (Some(0), Some(256), 0, 0, 0),
        (None, Some(256), 4, 256, 1),
        (None, None, u64::MAX, 0, u64::MAX / 4),
    ] {
        let mut plan = selectable_plan(vec![RuntimeAiCredential {
            name: "primary".into(),
            weight: 1,
            credential: "test-key".into(),
            limits: None,
        }]);
        plan.request_max_output_tokens = request_bound;
        plan.input_chars = chars;
        Arc::get_mut(plan.candidates[0].backend.as_mut().unwrap())
            .unwrap()
            .max_output_tokens = backend_bound;
        let mut ctx = EdgionHttpContext::new();
        let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
        storage.write(&plan.request_body).await.unwrap();
        storage.finish().await.unwrap();
        ctx.request_body.install(view);
        plan.body_generation = ctx.request_body.generation();
        plan.body_is_current = true;
        let mut session = test_session().await;
        let prepared = prepare_ready_attempt(&mut session, &mut ctx, &mut plan, 0).unwrap();
        let input = prepared.admission_input().unwrap();
        assert_eq!(input.max_output_tokens, expected_output);
        assert_eq!(input.estimated_input_tokens, expected_input);
        assert!(input.limits.is_none());
        assert!(input.redis_ref.is_none());
        assert_eq!(input.on_redis_failure, OnRedisFailure::Deny);
        assert!(input.adaptive.is_none());
        assert!(input.circuit_breaker.is_none());
    }
}

#[tokio::test]
async fn prepared_admission_rejects_invalid_selection_and_committed_context() {
    use pingora_core::protocols::http::RequestBodyBuffer;
    let mut plan = selectable_plan(vec![RuntimeAiCredential {
        name: "primary".into(),
        weight: 1,
        credential: "test-key".into(),
        limits: Some(AiCredentialLimits {
            requests_per_minute: Some(1),
            tokens_per_minute: Some(10),
        }),
    }]);
    let mut ctx = EdgionHttpContext::new();
    let (mut storage, view) = crate::request_body::BodyStorage::new(65536, 1024 * 1024);
    storage.write(&plan.request_body).await.unwrap();
    storage.finish().await.unwrap();
    ctx.request_body.install(view);
    plan.body_generation = ctx.request_body.generation();
    plan.body_is_current = true;
    let mut session = test_session().await;
    let mut prepared = prepare_ready_attempt(&mut session, &mut ctx, &mut plan, 0).unwrap();
    prepared.selected.credential_index = usize::MAX;
    assert!(matches!(
        prepared.admission_input(),
        Err(AiDispatchError::InvalidConfiguration)
    ));
    prepared.selected.credential_index = 0;
    assert!(prepared.admission_input().unwrap().limits.is_some());
    commit_ai_attempt_context(&mut ctx, &mut prepared).unwrap();
    assert!(matches!(
        prepared.admission_input(),
        Err(AiDispatchError::InvalidConfiguration)
    ));
}

#[test]
fn policy_ancestor_refusal_is_terminal_before_initial_and_retry_credentials() {
    let mut plan = selectable_plan(vec![RuntimeAiCredential {
        name: "primary".into(),
        weight: 1,
        credential: "test-token".into(),
        limits: None,
    }]);
    plan.candidates[0].policy_ancestor_admitted = false;
    assert!(matches!(
        select_target(&mut plan, 0),
        Err(AiDispatchError::InvalidConfiguration)
    ));
    assert_eq!(plan.scheduler.dispatch_count(), 0);
    assert!(matches!(plan.scheduler.next_step(), NextStep::PickBackend(_)));
    // The retry entry point must independently reject before credential selection.
    assert!(matches!(
        select_credential(&mut plan, 0, 0),
        Err(AiDispatchError::InvalidConfiguration)
    ));
    assert_eq!(plan.scheduler.dispatch_count(), 0);
    plan.candidates[0].policy_ancestor_admitted = true;
    assert!(select_target(&mut plan, 0).is_ok());
}
