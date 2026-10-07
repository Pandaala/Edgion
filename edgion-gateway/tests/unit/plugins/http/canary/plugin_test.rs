use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
use crate::{EdgionHttpContext, MatchInfo, Rsc};
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigData, EdgionConfigDataSpec, Selector};
use edgion_resources::resources::edgion_plugins::plugin_configs::DynUpstreamTarget;
use edgion_resources::{HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule};
use std::collections::HashMap;
use std::sync::Arc;

fn target(name: &str) -> DynUpstreamTarget {
    DynUpstreamTarget {
        name: name.to_string(),
        namespace: None,
    }
}

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

/// Minimal `HttpRouteRuleUnit` fixture with schema and compiled backend
/// views kept in sync, as they are in the production route compiler.
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

/// Build an `EdgionHttpContext` with `route_unit` set to a fixture carrying
/// `backend_refs`, for tests that exercise `run_request`'s backend-ref
/// pre-validation (canary / DIU / direct_endpoint all share this shape).
fn ctx_with_backend_refs(backend_refs: Vec<HTTPBackendRef>) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.route_unit = Some(Arc::new(test_route_unit(backend_refs)));
    ctx
}

fn base_config() -> CanaryConfig {
    CanaryConfig {
        enable: true,
        active_profile: "stable".to_string(),
        profiles: HashMap::from([
            ("stable".to_string(), target("app-v1")),
            ("canary".to_string(), target("app-v2")),
        ]),
        active_profile_ref: None,
        on_invalid: UpstreamFallback::Reject,
    }
}

fn canary_with_key(config: CanaryConfig, key: Option<&str>) -> Canary {
    Canary {
        name: "Canary".to_string(),
        config,
        active_profile_ref_key: key.map(|k| k.to_string()),
    }
}

#[test]
fn no_selector_uses_active_profile() {
    let plugin = canary_with_key(base_config(), None);
    let mut log = PluginLog::new("Canary");
    assert_eq!(
        plugin.resolve_active_profile(&mut log).map(|(name, _)| name),
        Some("stable")
    );
}

#[test]
fn selector_missing_falls_back_to_base() {
    let plugin = canary_with_key(base_config(), Some("unit-test-ns/canary-selector-absent"));
    let mut log = PluginLog::new("Canary");
    assert_eq!(
        plugin.resolve_active_profile(&mut log).map(|(name, _)| name),
        Some("stable")
    );
    assert!(
        log.contains("selector miss: absent"),
        "absent-store-entry branch must emit the distinct `absent` log tag for access-log grep"
    );
}

#[test]
fn selector_active_overrides_base() {
    let test_key = "unit-test-ns/canary-selector-override";
    let mut ed = EdgionConfigData::new(
        "canary-selector-override",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::Selector(Selector {
                active: Some("canary".to_string()),
                description: None,
            }),
        },
    );
    ed.metadata.namespace = Some("unit-test-ns".to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

    let plugin = canary_with_key(base_config(), Some(test_key));
    let mut log = PluginLog::new("Canary");
    assert_eq!(
        plugin.resolve_active_profile(&mut log).map(|(name, _)| name),
        Some("canary")
    );
}

#[test]
fn selector_kill_switch_falls_back_to_base() {
    let test_key = "unit-test-ns/canary-selector-killswitch";
    let mut ed = EdgionConfigData::new(
        "canary-selector-killswitch",
        EdgionConfigDataSpec {
            current_status: None,
            enable: false, // envelope kill-switch
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::Selector(Selector {
                active: Some("canary".to_string()),
                description: None,
            }),
        },
    );
    ed.metadata.namespace = Some("unit-test-ns".to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

    let plugin = canary_with_key(base_config(), Some(test_key));
    let mut log = PluginLog::new("Canary");
    assert_eq!(
        plugin.resolve_active_profile(&mut log).map(|(name, _)| name),
        Some("stable")
    );
    assert!(
        log.contains("selector miss: disabled"),
        "envelope-kill-switch branch must emit the distinct `disabled` log tag for access-log grep"
    );
}

#[test]
fn selector_unknown_name_falls_back_to_base() {
    let test_key = "unit-test-ns/canary-selector-unknown";
    let mut ed = EdgionConfigData::new(
        "canary-selector-unknown",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::Selector(Selector {
                active: Some("ghost".to_string()),
                description: None,
            }),
        },
    );
    ed.metadata.namespace = Some("unit-test-ns".to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

    let plugin = canary_with_key(base_config(), Some(test_key));
    let mut log = PluginLog::new("Canary");
    assert_eq!(
        plugin.resolve_active_profile(&mut log).map(|(name, _)| name),
        Some("stable")
    );
    assert!(
        log.contains("selector miss: unknown_profile"),
        "unknown-active-name branch must emit the distinct `unknown_profile` log tag for access-log grep"
    );
}

#[test]
fn selector_none_active_is_noop_uses_base() {
    let test_key = "unit-test-ns/canary-selector-none-active";
    let mut ed = EdgionConfigData::new(
        "canary-selector-none-active",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::Selector(Selector {
                active: None,
                description: None,
            }),
        },
    );
    ed.metadata.namespace = Some("unit-test-ns".to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

    let plugin = canary_with_key(base_config(), Some(test_key));
    let mut log = PluginLog::new("Canary");
    assert_eq!(
        plugin.resolve_active_profile(&mut log).map(|(name, _)| name),
        Some("stable")
    );
    assert!(
        !log.contains("selector miss"),
        "`active: None` is a deliberate no-op and must NOT emit any selector-miss tag"
    );
}

/// Regression: an `EdgionConfigData` present at the referenced key but carrying a
/// non-`Selector` variant (e.g. a `KeyList` misconfigured under the Canary ref) must
/// fail-to-base AND emit the distinct `type_mismatch` tag so operators can grep
/// this specific misconfiguration in the access log.
#[test]
fn selector_wrong_type_falls_back_and_logs_type_mismatch() {
    use edgion_resources::resources::edgion_config_data::{KeyListData, KeyMatchMode};

    let test_key = "unit-test-ns/canary-selector-wrongtype";
    let mut ed = EdgionConfigData::new(
        "canary-selector-wrongtype",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::KeyList(KeyListData {
                match_mode: KeyMatchMode::Exact,
                // Wrong variant is the whole point — items list can be empty.
                items: vec![],
            }),
        },
    );
    ed.metadata.namespace = Some("unit-test-ns".to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

    let plugin = canary_with_key(base_config(), Some(test_key));
    let mut log = PluginLog::new("Canary");
    assert_eq!(
        plugin.resolve_active_profile(&mut log).map(|(name, _)| name),
        Some("stable")
    );
    assert!(
        log.contains("selector miss: type_mismatch"),
        "wrong-ConfigEntry-variant branch must emit the distinct `type_mismatch` log tag for access-log grep"
    );
}

/// A denied (`ref_denied: Some`) `active_profile_ref` must not be resolved
/// as a store key — `resolve_active_profile_ref_key` (the exact function
/// `Canary::create()` calls to build `active_profile_ref_key`) must apply
/// the `.filter(|r| r.ref_denied.is_none())` gate `RequestAccessPolicy::create`
/// applies too, otherwise a cross-namespace ref the controller explicitly
/// denied would still take effect at runtime. Unlike a test that hand-builds
/// a separate `Canary` struct, this calls the very function `create()` uses,
/// so deleting the `ref_denied` filter from `create()`/this function would
/// fail this test.
#[test]
fn denied_active_profile_ref_resolves_to_no_store_key() {
    use edgion_resources::resources::common::RefDenied;

    let denied_ref = Some(EdgionConfigDataRef {
        name: "canary-selector-denied".to_string(),
        namespace: Some("other-ns".to_string()),
        ref_denied: Some(RefDenied {
            target_namespace: "other-ns".to_string(),
            target_name: "canary-selector-denied".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        }),
    });

    assert_eq!(
        resolve_active_profile_ref_key(&denied_ref, "unit-test-ns"),
        None,
        "a denied activeProfileRef must never resolve to a live store key"
    );
}

/// Sanity counterpart to the test above: a non-denied ref DOES resolve to a
/// store key, proving the gate depends specifically on `ref_denied` (and
/// isn't just an always-`None` stub that would make the denial test above
/// vacuously pass).
#[test]
fn allowed_active_profile_ref_resolves_to_store_key() {
    let allowed_ref = Some(EdgionConfigDataRef {
        name: "canary-selector-allowed".to_string(),
        namespace: Some("other-ns".to_string()),
        ref_denied: None,
    });

    assert_eq!(
        resolve_active_profile_ref_key(&allowed_ref, "unit-test-ns"),
        Some("other-ns/canary-selector-allowed".to_string())
    );
}

// ── record_outcome wiring ────────────────────────────────────────────────

/// A successful selection (resolved profile's backend found in the route's
/// backend_refs, `SetJumpOutcome::Set`) must record `Success` with the
/// resolved profile name — the value the deleted `apply_dye_headers` call
/// used to pass.
#[tokio::test]
async fn canary_records_success_outcome_with_profile_name() {
    let plugin = canary_with_key(base_config(), None);

    let mut session = MockPluginSession::new();
    session
        .expect_ctx()
        .return_const(ctx_with_backend_refs(vec![test_backend_ref("app-v1")]));
    session.expect_set_upstream_jump().return_const(SetJumpOutcome::Set);
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Success && v == "stable")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("Canary");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

/// `on_invalid: Reject` (default) on an unresolvable backend ref must record
/// `Reject` with cause `on-invalid` and return the 403 local reply.
#[tokio::test]
async fn canary_records_reject_outcome_on_invalid_backend() {
    let config = CanaryConfig {
        on_invalid: UpstreamFallback::Reject,
        ..base_config()
    };
    let plugin = canary_with_key(config, None);

    let mut session = MockPluginSession::new();
    // Route's only backend_ref is "other-svc" — never matches the resolved
    // "app-v1" target, so the `!found` branch fires.
    session
        .expect_ctx()
        .return_const(ctx_with_backend_refs(vec![test_backend_ref("other-svc")]));
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Reject && v == "on-invalid")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("Canary");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"Target backend not allowed")),
        }
    );
}

/// A matching backend ref denied by the controller must take the same
/// onInvalid path as any other unusable target, before a jump is published.
#[tokio::test]
async fn canary_rejects_denied_backend_before_setting_jump() {
    let plugin = canary_with_key(base_config(), None);
    let mut denied = test_backend_ref("app-v1");
    denied.ref_denied = Some(RefDenied {
        target_namespace: "test-ns".to_string(),
        target_name: "app-v1".to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    });

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(ctx_with_backend_refs(vec![denied]));
    session.expect_set_upstream_jump().times(0);
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Reject && v == "on-invalid")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("Canary");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"Target backend not allowed")),
        }
    );
    assert!(log.contains("backend ref not found"));
}

/// `on_invalid: Fallback` on an unresolvable backend ref must record
/// `Fallback` with cause `on-invalid` and pass the request through unchanged.
#[tokio::test]
async fn canary_records_fallback_outcome_on_invalid_backend() {
    let config = CanaryConfig {
        on_invalid: UpstreamFallback::Fallback,
        ..base_config()
    };
    let plugin = canary_with_key(config, None);

    let mut session = MockPluginSession::new();
    session
        .expect_ctx()
        .return_const(ctx_with_backend_refs(vec![test_backend_ref("other-svc")]));
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Fallback && v == "on-invalid")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("Canary");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn canary_rejects_missing_service_port_before_setting_jump() {
    let plugin = canary_with_key(base_config(), None);
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
    session.expect_set_upstream_jump().times(0);
    session
        .expect_record_outcome()
        .withf(|outcome, value| *outcome == DyeOutcome::Reject && value == "on-invalid")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("Canary");
    let result = plugin.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 403,
            body: Some(Bytes::from_static(b"Target backend not allowed"))
        }
    );
}
