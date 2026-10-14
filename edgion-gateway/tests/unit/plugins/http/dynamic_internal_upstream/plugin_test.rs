use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
use crate::{EdgionHttpContext, MatchInfo, Rsc};
use edgion_resources::common::KeyGet;
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    DynUpstreamExtract, DynUpstreamRule, DynUpstreamTarget,
};
use edgion_resources::{HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule};
use std::sync::Arc;

/// A standard Service fixture with its required numeric section. Names,
/// namespaces, weights and denials remain independently controlled by each test.
fn test_backend_ref(name: &str) -> HTTPBackendRef {
    HTTPBackendRef {
        name: name.to_string(),
        namespace: None,
        port: Some(80),
        weight: None,
        group: None,
        kind: None,
        filters: None,
        backend_tls_policy: None,
        ref_denied: None,
    }
}

fn test_backend_ref_in(name: &str, namespace: &str) -> HTTPBackendRef {
    HTTPBackendRef {
        namespace: Some(namespace.to_string()),
        ..test_backend_ref(name)
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
    let runtime = crate::routes::http::match_unit::compile_http_fixture(&rule, "test-ns").unwrap();
    let backend_finder = HttpRouteRuleUnit::build_backend_selector(&runtime.backends);

    HttpRouteRuleUnit {
        session_persistence_ttl_secs: None,
        resource_key: "test-ns/test-route".to_string(),
        matched_info: MatchInfo {
            rsc: Rsc::new("HTTPRoute", "test-ns", "test-route"),
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
            route_key: "test-ns/test-route".to_string(),
            rule_index: 0,
            match_index: 0,
        },
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
        plugin_runtime: runtime.plugin_runtime,
        backends: runtime.backends,
        backend_finder,
    }
}

fn ctx_with_backend_refs(backend_refs: Vec<HTTPBackendRef>) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.route_unit = Some(Arc::new(test_route_unit(backend_refs)));
    ctx
}

/// Direct mode (`rules: None`): the routing key extracted via `from` IS the
/// backend_ref name.
fn direct_mode_config() -> DynamicInternalUpstreamConfig {
    DynamicInternalUpstreamConfig {
        from: KeyGet::Header {
            name: "X-Backend-Target".to_string(),
        },
        extract: None,
        rules: None,
        on_missing: UpstreamFallback::Fallback,
        on_no_match: UpstreamFallback::Fallback,
        on_invalid: UpstreamFallback::Reject,
        compiled_regex: None,
    }
}

#[test]
fn dynamic_internal_upstream_name_is_static_across_instances() {
    let first = DynamicInternalUpstream::create(&direct_mode_config());
    let second = DynamicInternalUpstream::create(&DynamicInternalUpstreamConfig {
        on_invalid: UpstreamFallback::Fallback,
        ..direct_mode_config()
    });

    assert_eq!(first.name(), "DynamicInternalUpstream");
    assert_eq!(second.name(), "DynamicInternalUpstream");
}

#[tokio::test]
async fn regex_capture_selects_namespaced_rule_and_records_namespaced_success() {
    let config = DynamicInternalUpstreamConfig {
        extract: Some(DynUpstreamExtract {
            regex: r"^tier=(.+)$".to_string(),
            group: 1,
        }),
        rules: Some(vec![DynUpstreamRule {
            value: "premium".to_string(),
            backend_ref: DynUpstreamTarget {
                name: "premium-api".to_string(),
                namespace: Some("shared-services".to_string()),
            },
        }]),
        ..direct_mode_config()
    };
    let plugin = DynamicInternalUpstream::create(&config);

    let mut session = MockPluginSession::new();
    session
        .expect_ctx()
        .return_const(ctx_with_backend_refs(vec![test_backend_ref_in(
            "premium-api",
            "shared-services",
        )]));
    session.expect_key_get().return_const(Some("tier=premium".to_string()));
    session
        .expect_set_upstream_jump()
        .withf(|target| {
            matches!(
                target,
                UpstreamJumpTarget::Internal(jump)
                    if jump.backend_ref_name == "premium-api"
                        && jump.backend_ref_namespace.as_deref() == Some("shared-services")
            )
        })
        .times(1)
        .return_const(SetJumpOutcome::Set);
    session
        .expect_record_outcome()
        .withf(|outcome, value| *outcome == DyeOutcome::Success && value == "shared-services/premium-api")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicInternalUpstream");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert!(log.contains("upstream selected"));
}

/// A successful selection (resolved backend_ref found in the route's
/// backend_refs, `SetJumpOutcome::Set`) must record `Success` with the
/// resolved `"[namespace/]name"` value — the value the deleted
/// `apply_dye_headers` call used to pass.
#[tokio::test]
async fn diu_records_success_outcome_with_resolved_name() {
    let plugin = DynamicInternalUpstream::create(&direct_mode_config());

    let mut session = MockPluginSession::new();
    session
        .expect_ctx()
        .return_const(ctx_with_backend_refs(vec![test_backend_ref("app-v1")]));
    session.expect_key_get().return_const(Some("app-v1".to_string()));
    session
        .expect_set_upstream_jump()
        .withf(|target| {
            matches!(
                target,
                UpstreamJumpTarget::Internal(jump)
                    if jump.backend_ref_name == "app-v1" && jump.backend_ref_namespace.is_none()
            )
        })
        .times(1)
        .return_const(SetJumpOutcome::Set);
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Success && v == "app-v1")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicInternalUpstream");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(log.contains("upstream selected"));
}

#[tokio::test]
async fn rejected_jump_records_only_locked_log_and_no_success_dye() {
    let config = DynamicInternalUpstreamConfig {
        rules: Some(vec![DynUpstreamRule {
            value: "premium".to_string(),
            backend_ref: DynUpstreamTarget {
                name: "premium-api".to_string(),
                namespace: Some("shared-services".to_string()),
            },
        }]),
        ..direct_mode_config()
    };
    let plugin = DynamicInternalUpstream::create(&config);

    let mut session = MockPluginSession::new();
    session
        .expect_ctx()
        .return_const(ctx_with_backend_refs(vec![test_backend_ref_in(
            "premium-api",
            "shared-services",
        )]));
    session.expect_key_get().return_const(Some("premium".to_string()));
    session
        .expect_set_upstream_jump()
        .withf(|target| {
            matches!(
                target,
                UpstreamJumpTarget::Internal(jump)
                    if jump.backend_ref_name == "premium-api"
                        && jump.backend_ref_namespace.as_deref() == Some("shared-services")
            )
        })
        .times(1)
        .return_const(SetJumpOutcome::Rejected);
    // No record_outcome expectation: any Success Dye publication fails the mock.

    let mut log = PluginLog::new("DynamicInternalUpstream");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(
        serde_json::to_value(&log).unwrap()["log"],
        serde_json::json!(["jump locked, rejected"])
    );
}

/// Missing routing key with `on_missing: Reject` must record `Reject` with
/// cause `on-missing` and return the 400 local reply.
#[tokio::test]
async fn diu_records_reject_outcome_on_missing_key() {
    let config = DynamicInternalUpstreamConfig {
        on_missing: UpstreamFallback::Reject,
        ..direct_mode_config()
    };
    let plugin = DynamicInternalUpstream::create(&config);

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_key_get().return_const(None::<String>);
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Reject && v == "on-missing")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicInternalUpstream");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 400,
            body: Some(Bytes::from_static(b"Missing routing key for backend selection")),
        }
    );
}

/// `on_invalid: Fallback` on an unresolvable backend ref must record
/// `Fallback` with cause `on-invalid` and pass the request through.
#[tokio::test]
async fn diu_records_fallback_outcome_on_invalid_backend() {
    let config = DynamicInternalUpstreamConfig {
        on_invalid: UpstreamFallback::Fallback,
        ..direct_mode_config()
    };
    let plugin = DynamicInternalUpstream::create(&config);

    let mut session = MockPluginSession::new();
    // Route's only backend_ref is "other-svc" — never matches the resolved
    // "app-v1" target (direct mode), so the `!found` branch fires.
    session
        .expect_ctx()
        .return_const(ctx_with_backend_refs(vec![test_backend_ref("other-svc")]));
    session.expect_key_get().return_const(Some("app-v1".to_string()));
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Fallback && v == "on-invalid")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicInternalUpstream");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

/// A matching backend ref denied by the controller must take the same
/// `on_invalid` path as any other unusable target, before a jump is
/// published. The raw schema scan this replaced ignored `ref_denied`
/// entirely and published the jump with a `Success` Dye outcome.
#[tokio::test]
async fn diu_rejects_denied_backend_before_setting_jump() {
    let plugin = DynamicInternalUpstream::create(&direct_mode_config());
    let mut denied = test_backend_ref("app-v1");
    denied.ref_denied = Some(RefDenied {
        target_namespace: "test-ns".to_string(),
        target_name: "app-v1".to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    });

    let mut session = MockPluginSession::new();
    session
        .expect_ctx()
        .return_const(ctx_with_backend_refs(vec![denied, test_backend_ref("app-v2")]));
    session.expect_key_get().return_const(Some("app-v1".to_string()));
    session.expect_set_upstream_jump().times(0);
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Reject && v == "on-invalid")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicInternalUpstream");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"Target backend not allowed")),
        }
    );
    assert!(
        log.contains("backend ref denied"),
        "a denied ref must be diagnosed apart from a missing one"
    );
}

/// The same denied target under `on_invalid: Fallback` passes the request
/// through without a jump, so weighted selection owns the outcome — it must
/// never be published as a `Success` selection of the denied name.
#[tokio::test]
async fn diu_falls_back_on_denied_backend_without_setting_jump() {
    let config = DynamicInternalUpstreamConfig {
        on_invalid: UpstreamFallback::Fallback,
        ..direct_mode_config()
    };
    let plugin = DynamicInternalUpstream::create(&config);
    let mut denied = test_backend_ref("app-v1");
    denied.ref_denied = Some(RefDenied {
        target_namespace: "test-ns".to_string(),
        target_name: "app-v1".to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    });

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(ctx_with_backend_refs(vec![denied]));
    session.expect_key_get().return_const(Some("app-v1".to_string()));
    session.expect_set_upstream_jump().times(0);
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Fallback && v == "on-invalid")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicInternalUpstream");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

/// Validation through the compiled backend view must keep the documented
/// "hidden backend" pattern working: an explicit named target with
/// `weight: 0` is still selectable even though weighted selection can never
/// pick it.
#[tokio::test]
async fn diu_selects_explicit_weight_zero_backend() {
    let plugin = DynamicInternalUpstream::create(&direct_mode_config());
    let hidden = HTTPBackendRef {
        weight: Some(0),
        ..test_backend_ref("app-debug")
    };

    let mut session = MockPluginSession::new();
    session
        .expect_ctx()
        .return_const(ctx_with_backend_refs(vec![test_backend_ref("app-v1"), hidden]));
    session.expect_key_get().return_const(Some("app-debug".to_string()));
    session
        .expect_set_upstream_jump()
        .withf(|target| {
            matches!(
                target,
                UpstreamJumpTarget::Internal(jump) if jump.backend_ref_name == "app-debug"
            )
        })
        .times(1)
        .return_const(SetJumpOutcome::Set);
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Success && v == "app-debug")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("DynamicInternalUpstream");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert!(log.contains("upstream selected"));
}

#[tokio::test]
async fn diu_rejects_missing_service_port_before_setting_jump() {
    let plugin = DynamicInternalUpstream::create(&direct_mode_config());
    let mut schema = test_backend_ref("app-v1");
    schema.port = None;
    let ctx = ctx_with_backend_refs(vec![schema]);
    let compiled = &ctx.route_unit.as_ref().unwrap().backends[0];
    assert!(!compiled.is_service());
    assert!(matches!(
        compiled.target(),
        crate::backends::runtime::CompiledBackendTarget::Invalid { .. }
    ));
    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(ctx);
    session.expect_key_get().return_const(Some("app-v1".to_owned()));
    session.expect_set_upstream_jump().times(0);
    session
        .expect_record_outcome()
        .withf(|outcome, value| *outcome == DyeOutcome::Reject && value == "on-invalid")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("DynamicInternalUpstream");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"Target backend not allowed"))
        }
    );
}
