use super::*;
use crate::EdgionHttpContext;
use serde_json::json;

fn config() -> GraphqlOperationRouterConfig {
    serde_json::from_value(json!({"rules":[{"operationType":"query","operationName":"GetUser","backendRef":{"name":"users"}},{"operationType":"mutation","operationName":"CreateOrder","backendRef":{"name":"orders"}}]})).unwrap()
}
#[test]
fn exact_operation_pair_and_explicit_default_are_compiled_once() {
    let mut c = config();
    let router = GraphqlOperationRouter::try_new(&c, None).unwrap();
    assert_eq!(
        router
            .target(GraphqlOperationType::Query, Some("GetUser"))
            .unwrap()
            .0
            .name,
        "users"
    );
    assert_eq!(
        router
            .target(GraphqlOperationType::Mutation, Some("CreateOrder"))
            .unwrap()
            .0
            .name,
        "orders"
    );
    for name in [None, Some("getuser"), Some("Unknown")] {
        assert!(router.target(GraphqlOperationType::Query, name).is_none());
    }
    assert!(router.target(GraphqlOperationType::Mutation, Some("GetUser")).is_none());
    c.default_backend_ref = Some(DynUpstreamTarget {
        name: "core".into(),
        namespace: None,
    });
    let router = GraphqlOperationRouter::try_new(&c, None).unwrap();
    let (target, default) = router.target(GraphqlOperationType::Query, None).unwrap();
    assert_eq!(target.name, "core");
    assert!(default);
    assert!(!router.target(GraphqlOperationType::Query, Some("GetUser")).unwrap().1);
}
#[test]
fn constructor_rejects_invalid_config_and_unsafe_body_policy() {
    let mut c = config();
    c.rules.push(c.rules[0].clone());
    assert!(GraphqlOperationRouter::create(&c, None).is_none());
    for body in [
        BodyRequirement {
            max_body_size: Some("65k".into()),
            ..BodyRequirement::fail_close()
        },
        BodyRequirement {
            on_read_failure: edgion_resources::resources::edgion_plugins::BodyReadFailureMode::FailOpen,
            ..BodyRequirement::fail_close()
        },
    ] {
        assert!(GraphqlOperationRouter::create(&config(), Some(body)).is_none());
    }
    assert!(!GraphqlOperationRouter::create(&config(), None)
        .unwrap()
        .requires_materialized_request_body());
}
#[test]
fn strict_jump_cannot_be_silently_replaced_and_ordinary_jumps_keep_existing_behavior() {
    let jump = || {
        UpstreamJumpTarget::Internal(InternalJumpPreset {
            backend_ref_name: "users".into(),
            backend_ref_namespace: Some("apps".into()),
        })
    };
    let mut ctx = EdgionHttpContext::default();
    assert_eq!(ctx.try_set_strict_jump(jump()), SetJumpOutcome::Set);
    assert_eq!(ctx.try_set_jump(jump()), SetJumpOutcome::Rejected);
    assert!(ctx.has_strict_jump_conflict());
    let mut ctx = EdgionHttpContext::default();
    assert_eq!(ctx.try_set_jump(jump()), SetJumpOutcome::Set);
    assert_eq!(ctx.try_set_jump(jump()), SetJumpOutcome::Rejected);
    assert!(!ctx.has_strict_jump_conflict());
    assert_eq!(ctx.try_set_strict_jump(jump()), SetJumpOutcome::Rejected);
}

use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
use crate::{MatchInfo, Rsc};
use edgion_resources::{HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule};
use std::sync::Arc;
fn test_backend_ref(name: &str) -> HTTPBackendRef {
    HTTPBackendRef {
        name: name.to_string(),
        namespace: None,
        port: None,
        weight: None,
        group: None,
        kind: None,
        filters: None,
        backend_tls_policy: None,
        ref_denied: None,
    }
}

/// Minimal `HttpRouteRuleUnit` fixture with schema and compiled backend
/// views kept in sync, as they are in the production route compiler — the
/// plugin validates through the compiled view, so `ref_denied`, namespace
/// and weight semantics must match production exactly.
fn test_route_unit(backend_refs: Vec<HTTPBackendRef>) -> HttpRouteRuleUnit {
    let rule = Arc::new(HTTPRouteRule {
        name: None,
        resolved_terminal_route_uid: None,
        resolved_ai_admission: None,
        resolved_terminal_rule_identity: None,
        matches: None,
        filters: None,
        backend_refs: Some(backend_refs),
        timeouts: None,
        retry: None,
        session_persistence: None,
        parsed_timeouts: None,
        parsed_retry: None,
        parsed_forward_raw_path: None,
        parsed_allow_non_idempotent_retry: None,
        parsed_mirror_tuning: None,
    });
    let runtime = HttpRouteRuleUnit::compile_http_rule(&rule, "apps").unwrap();
    let backend_finder = HttpRouteRuleUnit::build_backend_selector(&runtime.backends);

    HttpRouteRuleUnit {
        resource_key: "apps/test-route".to_string(),
        matched_info: MatchInfo {
            rsc: Rsc::new("HTTPRoute", "apps", "test-route"),
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
        rule,
        route: None,
        path_regex: None,
        parent_refs: None,
        compiled_header_regexes: vec![],
        compiled_query_regexes: vec![],
        has_query_matchers: false,
        specificity: MatchSpecificity {
            has_method: false,
            header_count: 0,
            query_count: 0,
            creation_ts_ms: 0,
            route_key: "apps/test-route".to_string(),
            rule_index: 0,
            match_index: 0,
        },
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
        plugin_runtime: runtime.plugin_runtime,
        backends: runtime.backends,
        backend_finder,
    }
}

#[tokio::test]
async fn selected_target_is_authorized_before_strict_jump_publication_without_fallback() {
    use crate::plugins::http::common::graphql_request::EVALUATION_TEST_LOCK;
    use bytes::Bytes;
    use edgion_resources::resources::common::RefDenied;
    use http::{HeaderMap, HeaderValue};
    let _guard = EVALUATION_TEST_LOCK.lock().await;
    let mut config = config();
    config.default_backend_ref = Some(DynUpstreamTarget {
        name: "orders".into(),
        namespace: None,
    });
    let router = GraphqlOperationRouter::create(&config, None).unwrap();
    for (backend_name, denied, status) in [("users", false, 200), ("users", true, 403), ("orders", false, 500)] {
        let mut backend = test_backend_ref(backend_name);
        if denied {
            backend.ref_denied = Some(RefDenied {
                target_name: "users".into(),
                target_namespace: "apps".into(),
                reason: Some("test denied".into()),
            });
        }
        let mut ctx = EdgionHttpContext::default();
        ctx.route_unit = Some(Arc::new(test_route_unit(vec![backend])));
        ctx.request_info.in_request_body_stage = true;
        let body = br#"{"query":"query GetUser { user { id } }"}"#;
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx);
        session.expect_get_method().return_const("POST".to_owned());
        session.expect_declares_body().return_const(Some(true));
        session.expect_request_query().return_const(None);
        session
            .expect_request_body_available_len()
            .returning(move |_| Ok(body.len() as u64));
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        session.expect_request_header_map().return_const(headers);
        session
            .expect_read_request_body()
            .return_once(move |_| Ok(Bytes::from_static(body)));
        session.expect_record_error().return_const(());
        if status == 200 {
            session.expect_set_strict_upstream_jump().withf(|target|matches!(target,UpstreamJumpTarget::Internal(jump) if jump.backend_ref_name=="users" && jump.backend_ref_namespace.as_deref()==Some("apps"))).times(1).return_const(SetJumpOutcome::Set);
        } else {
            session.expect_set_strict_upstream_jump().times(0);
        }
        let result = router
            .run_request(&mut session, &mut PluginLog::new("GraphqlOperationRouter"))
            .await;
        if status == 200 {
            assert_eq!(result, PluginRunningResult::GoodNext);
        } else {
            assert!(matches!(result,PluginRunningResult::ErrResponse{status:actual,..} if actual==status));
        }
    }
}
