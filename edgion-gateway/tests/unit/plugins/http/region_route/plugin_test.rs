use super::*;
use crate::plugins::http::plugin_store::compile_plugins;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use bytes::Bytes;
use edgion_resources::common::{KeyGet, KeySet};
use edgion_resources::resources::edgion_plugins::{
    EdgionPlugin, EdgionPlugins, HashCalcConfig, RegionDef, RegionRouteConfig, RequestFilterEntry,
};

fn make_region(name: &str, low: u16, high: u16, endpoint: &str) -> RegionDef {
    RegionDef {
        name: name.to_string(),
        hash_range: [low, high],
        backend_endpoint: endpoint.to_string(),
        tls: false,
        failover_to: None,
    }
}

/// A default (empty) config must not panic and must yield no routing logic.
#[test]
fn default_config_has_no_routing() {
    let config = RegionRouteConfig::default();
    assert!(config.route_rules.is_empty());
    assert!(config.regions.is_empty());
    assert!(config.override_ref.is_none());
}

#[tokio::test]
async fn constructor_checks_invalid_base_before_any_request_side_effect() {
    let config = RegionRouteConfig {
        route_rules: vec![RouteRule::RouteByHashRange],
        regions: vec![make_region("bad", 0, 999, "bad:invalid")],
        ..Default::default()
    };
    let plugin = RegionRoute::create(&config, "default");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("RegionRoute");
    assert_eq!(
        plugin.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
    assert_eq!(log.render_for_test(), "err: region_table_invalid");
}

#[test]
fn overlay_updates_use_generation_proofs_and_consumer_modulo() {
    use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, RegionRouteOverride};
    let key = "unit-test-ns/rr-generation-validation";
    let store = get_global_edgion_config_data_store();
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".to_string(),
        init_error: None,
        base: Arc::new(RegionRouteConfig {
            regions: vec![
                make_region("base", 0, 499, "base"),
                make_region("base-only", 500, 999, "base-only"),
            ],
            ..Default::default()
        }),
        override_key: Some(key.to_string()),
    };
    let mut resource = EdgionConfigData::new(
        "rr-generation-validation",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                enable: Some(false),
                active: None,
                regions: Some(vec![make_region("overlay", 0, 999, "overlay")]),
            }),
        },
    );
    resource.metadata.namespace = Some("unit-test-ns".to_string());
    store.insert_raw_for_test(key, Arc::new(resource.clone()));
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("RegionRoute");
    let held = plugin.compute_effective_regions(&mut session, &mut log);
    assert_eq!(held.len(), 1, "overlay replaces the entire base table");
    assert_eq!(held[0].name, "overlay", "variant enable is inert");
    if let ConfigEntry::RegionRouteOverride(overlay) = &mut resource.spec.data {
        overlay.regions.as_mut().unwrap()[0].hash_range = [0, 1500];
    }
    store.insert_raw_for_test(key, Arc::new(resource.clone()));
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Fallback && v == "overlay-fail-to-base")
        .times(1)
        .return_const(());
    assert_eq!(plugin.compute_effective_regions(&mut session, &mut log)[0].name, "base");
    assert_eq!(
        held[0].hash_range,
        [0, 999],
        "held table cannot observe later publication"
    );
    session.checkpoint();
    if let ConfigEntry::RegionRouteOverride(overlay) = &mut resource.spec.data {
        overlay.regions = None;
    }
    store.insert_raw_for_test(key, Arc::new(resource));
    let inherited = plugin.compute_effective_regions(&mut session, &mut log);
    assert!(
        std::ptr::eq(&*inherited, plugin.base.regions.as_slice()),
        "None borrows the complete base"
    );
}

/// When no override_key is set, `compute_effective_regions` returns the base regions.
#[test]
fn base_regions_used_when_no_override_ref() {
    let config = RegionRouteConfig {
        regions: vec![
            make_region("us-east", 0, 499, "10.0.0.1:8080"),
            make_region("us-west", 500, 999, "10.0.0.2:8080"),
        ],
        override_ref: None,
        ..Default::default()
    };
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".to_string(),
        init_error: None,
        base: Arc::new(config),
        override_key: None,
    };

    // No overlay configured — the function returns before touching `session`,
    // so a mock with zero expectations correctly panics if that assumption breaks.
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("RegionRoute");
    let regions = plugin.compute_effective_regions(&mut session, &mut log);
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].name, "us-east");
    assert_eq!(regions[1].name, "us-west");
}

/// When override_key is set but not found in the global store, the plugin must
/// fall back to the base regions (fail-to-base, not fail-closed).
#[test]
fn overlay_missing_falls_back_to_base() {
    let config = RegionRouteConfig {
        regions: vec![make_region("eu-west", 0, 999, "10.0.1.1:8080")],
        ..Default::default()
    };
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".to_string(),
        init_error: None,
        base: Arc::new(config),
        // Key that is not present in the global store in this unit test.
        override_key: Some("unit-test-ns/rr-override-absent-key".to_string()),
    };

    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Fallback && v == "overlay-fail-to-base")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("RegionRoute");
    let regions = plugin.compute_effective_regions(&mut session, &mut log);
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].name, "eu-west");
}

/// `lookup_by_hash` returns the first region whose hash_range covers the given value.
#[test]
fn lookup_by_hash_returns_correct_region() {
    let regions = [make_region("r1", 0, 499, "h1:80"), make_region("r2", 500, 999, "h2:80")];
    assert_eq!(
        RegionRoute::lookup_by_hash(&regions, 0).map(|(_, r)| r.name.as_str()),
        Some("r1")
    );
    assert_eq!(
        RegionRoute::lookup_by_hash(&regions, 499).map(|(_, r)| r.name.as_str()),
        Some("r1")
    );
    assert_eq!(
        RegionRoute::lookup_by_hash(&regions, 500).map(|(_, r)| r.name.as_str()),
        Some("r2")
    );
    assert_eq!(
        RegionRoute::lookup_by_hash(&regions, 999).map(|(_, r)| r.name.as_str()),
        Some("r2")
    );
}

#[test]
fn supported_hash_algorithms_keep_their_existing_vectors() {
    let modulo = NonZeroU16::new(1000).unwrap();
    assert_eq!(compute_hash("routing-key", &HashAlgorithm::Crc32, modulo), 554);
    assert_eq!(compute_hash("routing-key", &HashAlgorithm::Sha256, modulo), 5);
}

#[tokio::test]
async fn direct_construction_rejects_invalid_modulo_before_session_side_effects() {
    for modulo in [0, 1] {
        let config = RegionRouteConfig {
            key_get: vec![KeyGet::Header {
                name: "x-route-key".to_string(),
            }],
            hash_calc: Some(HashCalcConfig {
                algorithm: HashAlgorithm::Crc32,
                modulo,
                write_back: vec![KeySet::Ctx {
                    name: "route-hash".to_string(),
                }],
            }),
            route_rules: vec![RouteRule::RouteByHashRange],
            regions: vec![make_region("region-a", 0, 1, "region-a:80")],
            ..RegionRouteConfig::default()
        };
        let plugin = RegionRoute::create(&config, "default");
        // No expectations: an extraction, write-back, or jump would fail the test.
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("RegionRoute");

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "err: hash_modulo_invalid");
    }
}

#[tokio::test]
async fn compile_plugins_seals_invalid_modulo_with_generic_response() {
    let config = RegionRouteConfig {
        hash_calc: Some(HashCalcConfig {
            algorithm: HashAlgorithm::Crc32,
            modulo: 1,
            write_back: Vec::new(),
        }),
        ..RegionRouteConfig::default()
    };
    let mut resource = EdgionPlugins::new("invalid-region-route", Default::default());
    resource.metadata.namespace = Some("default".to_string());
    resource.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::RegionRoute(config))]);

    let compiled = compile_plugins(&resource);

    assert!(!compiled.is_activatable);
    assert!(!compiled.retain_previous_on_invalid_update);
    assert_eq!(
        compiled.preparse_errors,
        vec!["requestPlugins[0] (RegionRoute): hashCalc.modulo must be at least 2"]
    );
    assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);
    let filter = compiled
        .plugin_runtime
        .request_plugins_iter()
        .next()
        .expect("sealed reject filter");
    assert_eq!(filter.name(), "RejectInvalidConfig");

    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new(filter.name());
    let result = filter.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: Some(Bytes::from_static(br#"{"message":"plugin configuration error"}"#)),
        }
    );
    assert_eq!(log.render_for_test(), "err: invalid plugin config\nerr: RegionRoute");
}

/// `apply_failover_redirect` with a valid target returns the target region.
#[test]
fn apply_failover_redirect_follows_one_level() {
    let regions = [
        RegionDef {
            name: "primary".to_string(),
            hash_range: [0, 499],
            backend_endpoint: "h1:80".to_string(),
            tls: false,
            failover_to: Some("backup".to_string()),
        },
        RegionDef {
            name: "backup".to_string(),
            hash_range: [500, 749],
            backend_endpoint: "h2:80".to_string(),
            tls: false,
            failover_to: Some("last".to_string()),
        },
        RegionDef {
            name: "last".to_string(),
            hash_range: [750, 999],
            backend_endpoint: "h3:80".to_string(),
            tls: false,
            failover_to: None,
        },
    ];
    let mut log = PluginLog::new("RegionRoute");
    let (_, r) = RegionRoute::apply_failover_redirect(&regions, 0, &regions[0], &mut log);
    assert_eq!(r.name, "backup");
}

/// `apply_failover_redirect` with a self-referencing `failoverTo` returns the original.
#[test]
fn apply_failover_redirect_self_ref_returns_original() {
    let regions = [RegionDef {
        name: "only".to_string(),
        hash_range: [0, 999],
        backend_endpoint: "h1:80".to_string(),
        tls: false,
        failover_to: Some("only".to_string()),
    }];
    let mut log = PluginLog::new("RegionRoute");
    let (_, r) = RegionRoute::apply_failover_redirect(&regions, 0, &regions[0], &mut log);
    assert_eq!(r.name, "only");
}

/// `apply_failover_redirect` with an unknown target returns the original.
#[test]
fn apply_failover_redirect_unknown_target_returns_original() {
    let regions = [RegionDef {
        name: "a".to_string(),
        hash_range: [0, 999],
        backend_endpoint: "h1:80".to_string(),
        tls: false,
        failover_to: Some("nonexistent".to_string()),
    }];
    let mut log = PluginLog::new("RegionRoute");
    let (_, r) = RegionRoute::apply_failover_redirect(&regions, 0, &regions[0], &mut log);
    assert_eq!(r.name, "a");
}

/// H1: When the referenced EdgionConfigData has `spec.enable=false` (envelope kill-switch),
/// the overlay must NOT be applied — base regions must be returned.
///
/// The overlay contains regions that differ from base; if the kill-switch were ignored,
/// the base regions would be replaced. Asserting base regions verifies H1.
#[test]
fn overlay_envelope_disabled_falls_back_to_base() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigData, EdgionConfigDataSpec, RegionRouteOverride,
    };
    use std::sync::Arc;

    // Unique key — no other test inserts or depends on this key.
    let test_key = "unit-test-ns/rr-overlay-envelope-disabled-h1-test";

    // Build an EdgionConfigData with spec.enable=false and regions that DIFFER from base.
    // If the kill-switch is honoured, these regions must NOT replace base.
    let override_regions = vec![make_region("override-only", 0, 999, "override-ep:9090")];
    let mut ed = EdgionConfigData::new(
        "rr-overlay-envelope-disabled-h1-test",
        EdgionConfigDataSpec {
            current_status: None,
            enable: false, // envelope kill-switch
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                enable: None,
                active: None,
                regions: Some(override_regions),
            }),
        },
    );
    ed.metadata.namespace = Some("unit-test-ns".to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

    // Base: single region distinct from the override.
    let config = RegionRouteConfig {
        regions: vec![make_region("base-region", 0, 999, "base-ep:8080")],
        ..Default::default()
    };
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".to_string(),
        init_error: None,
        base: Arc::new(config),
        override_key: Some(test_key.to_string()),
    };

    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Fallback && v == "overlay-fail-to-base")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("RegionRoute");
    let regions = plugin.compute_effective_regions(&mut session, &mut log);

    assert_eq!(regions.len(), 1, "base has exactly 1 region");
    assert_eq!(
        regions[0].name, "base-region",
        "envelope disabled: must return base regions, not override"
    );
}

/// H2: When the overlay is applied but the merged effective regions are invalid
/// (duplicate region names), the plugin must fall back to base regions.
#[test]
fn overlay_invalid_regions_falls_back_to_base() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigData, EdgionConfigDataSpec, RegionRouteOverride,
    };
    use std::sync::Arc;

    let test_key = "unit-test-ns/rr-overlay-invalid-dup-names-h2-test";

    // Build an EdgionConfigData whose overlay has duplicate region names (invalid).
    let dup_regions = vec![
        make_region("dup-name", 0, 499, "ep-a:8080"),
        make_region("dup-name", 500, 999, "ep-b:8080"), // duplicate — must trigger overlay_invalid
    ];
    let mut ed = EdgionConfigData::new(
        "rr-overlay-invalid-dup-names-h2-test",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true, // envelope enabled — the validation must catch the bad regions
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                enable: None,
                active: None,
                regions: Some(dup_regions),
            }),
        },
    );
    ed.metadata.namespace = Some("unit-test-ns".to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

    let config = RegionRouteConfig {
        regions: vec![make_region("valid-base", 0, 999, "base:8080")],
        ..Default::default()
    };
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".to_string(),
        init_error: None,
        base: Arc::new(config),
        override_key: Some(test_key.to_string()),
    };

    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Fallback && v == "overlay-fail-to-base")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("RegionRoute");
    let regions = plugin.compute_effective_regions(&mut session, &mut log);

    assert_eq!(regions.len(), 1, "base has exactly 1 region");
    assert_eq!(
        regions[0].name, "valid-base",
        "invalid overlay: must fall back to base regions"
    );
}

/// `route_to` on a successful cross-region jump (`SetJumpOutcome::Set`) must
/// record a `Success` dye outcome with the target region's name — the value
/// the deleted `apply_dye_headers` call used to pass.
#[tokio::test]
async fn route_to_records_success_outcome_with_region_name() {
    let config = RegionRouteConfig {
        // Distinct from the target region so the local-passthrough branch
        // (which does not call set_upstream_jump) is not taken.
        my_region: "local".to_string(),
        ..Default::default()
    };
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".to_string(),
        init_error: None,
        base: Arc::new(config),
        override_key: None,
    };
    let region = make_region("us-east", 0, 999, "10.0.0.5:9090");

    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[]);
    session
        .expect_set_upstream_jump()
        .withf(|target| match target {
            UpstreamJumpTarget::External(jump) => jump.upstream_read_timeout_override.is_none(),
            _ => false,
        })
        .return_const(SetJumpOutcome::Set);
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Success && v == "us-east")
        .times(1)
        .return_const(());

    let mut log = PluginLog::new("RegionRoute");
    let result = plugin.route_to(&mut session, &region, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

/// `route_to` on a local-region pass-through (my_region matches the target,
/// no jump is set) must still record a `Success` dye outcome with the target
/// region's name — the value the deleted `apply_dye_headers` call used to pass
/// at this same branch (old comment: "The request WAS routed to a region, so
/// it is dyed like a jump.").
#[tokio::test]
async fn route_to_local_region_pass_records_success_outcome_with_region_name() {
    let config = RegionRouteConfig {
        // Same as the target region so the local-passthrough branch is taken
        // (no set_upstream_jump call).
        my_region: "us-east".to_string(),
        ..Default::default()
    };
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".to_string(),
        init_error: None,
        base: Arc::new(config),
        override_key: None,
    };
    let region = make_region("us-east", 0, 999, "10.0.0.5:9090");

    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|o, v| *o == DyeOutcome::Success && v == "us-east")
        .times(1)
        .return_const(());
    // No `expect_header_value()` / `expect_set_upstream_jump()`: the local
    // pass-through branch returns before either is reached.

    let mut log = PluginLog::new("RegionRoute");
    let result = plugin.route_to(&mut session, &region, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

/// A jump rejected by an earlier plugin (`SetJumpOutcome::Rejected`) must NOT
/// record any dye outcome — region_route's capability table excludes Reject
/// for this plugin (pre-existing silent-GoodNext behavior, out of scope here).
#[tokio::test]
async fn route_to_rejected_jump_records_no_outcome() {
    let config = RegionRouteConfig {
        my_region: "local".to_string(),
        ..Default::default()
    };
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".to_string(),
        init_error: None,
        base: Arc::new(config),
        override_key: None,
    };
    let region = make_region("us-east", 0, 999, "10.0.0.5:9090");

    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[]);
    session
        .expect_set_upstream_jump()
        .return_const(SetJumpOutcome::Rejected);
    // No `expect_record_outcome()` at all: any call panics the mock.

    let mut log = PluginLog::new("RegionRoute");
    let result = plugin.route_to(&mut session, &region, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

// Detached fixture convenience is confined to this mirrored test module.
impl RegionRoute {
    pub(crate) fn create(config: &RegionRouteConfig, namespace: &str) -> Box<dyn RequestFilter> {
        let owner: edgion_resources::EdgionPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "fixture", "namespace": namespace},
            "spec": {"requestPlugins": [{"type": "RegionRoute", "config": config}]}
        }))
        .unwrap();
        Self::create_bound(HttpConfigDataEntry::from_plugins(&owner, 0).unwrap()).unwrap()
    }
}

#[test]
fn owning_region_invalid_target_keeps_base_but_owner_failure_has_no_constructor() {
    for reference in [
        serde_json::json!({"name":"bad/name"}),
        serde_json::json!({"name":"overlay","namespace":""}),
    ] {
        let config = RegionRouteConfig {
            override_ref: Some(serde_json::from_value(reference).unwrap()),
            ..Default::default()
        };
        let owner: EdgionPlugins=serde_json::from_value(serde_json::json!({"metadata":{"name":"region"},"spec":{"requestPlugins":[{"type":"RegionRoute","config":config}]}})).unwrap();
        let entry = HttpConfigDataEntry::from_plugins(&owner, 0).unwrap();
        assert!(entry.field().unwrap().runtime_key().is_err());
        assert!(RegionRoute::create_bound(entry).is_some());
        assert!(RegionRoute::create_inline(&config).is_none());
        let mut owner = owner;
        owner.metadata.namespace = Some(String::new());
        assert!(RegionRoute::create_bound(HttpConfigDataEntry::from_plugins(&owner, 0).unwrap()).is_none());
    }
}

#[test]
fn wrong_overlay_type_preserves_the_complete_base_and_reports_fallback() {
    let key = "unit-test-ns/rr-wrong-type-complete-base";
    let resource: EdgionConfigData = serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionConfigData",
        "metadata": {"name": "rr-wrong-type-complete-base", "namespace": "unit-test-ns"},
        "spec": {"data": {"type": "Misc", "config": {}}}
    }))
    .unwrap();
    get_global_edgion_config_data_store().insert_raw_for_test(key, Arc::new(resource));
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".into(),
        init_error: None,
        base: Arc::new(RegionRouteConfig {
            regions: vec![
                make_region("first", 0, 499, "first"),
                make_region("second", 500, 999, "second"),
            ],
            ..Default::default()
        }),
        override_key: Some(key.into()),
    };
    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| *outcome == DyeOutcome::Fallback && value == "overlay-fail-to-base")
        .times(1)
        .return_const(());
    let mut log = PluginLog::new("RegionRoute");
    let regions = plugin.compute_effective_regions(&mut session, &mut log);
    assert!(std::ptr::eq(&*regions, plugin.base.regions.as_slice()));
    assert!(log.render_for_test().contains("overlay_type_mismatch"));
}

#[test]
fn extracted_route_values_preserve_source_order_and_ownership() {
    let plugin = RegionRoute {
        writeback_log_targets: String::new(),
        name: "RegionRoute".to_string(),
        init_error: None,
        base: Arc::new(RegionRouteConfig::default()),
        override_key: None,
    };
    for source in [
        KeyGet::Header {
            name: "x-region".to_string(),
        },
        KeyGet::Query {
            name: "region".to_string(),
        },
        KeyGet::Cookie {
            name: "region".to_string(),
        },
        KeyGet::Ctx {
            name: "region".to_string(),
        },
        KeyGet::Path,
        KeyGet::RemoteIpAndPath,
    ] {
        let sources = [
            KeyGet::Header {
                name: "x-empty".to_string(),
            },
            source,
            KeyGet::Method,
        ];
        let mut headers = http::HeaderMap::new();
        headers.insert("x-empty", http::HeaderValue::from_static(""));
        headers.insert("x-region", http::HeaderValue::from_static("selected"));
        headers.insert(http::header::COOKIE, http::HeaderValue::from_static("region=selected"));
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(headers);
        session.expect_get_query_param_ref().return_const(Some("selected"));
        session.expect_get_ctx_var_ref().return_const(Some("selected"));
        session.expect_get_path().return_const("selected".to_string());
        session.expect_get_method().times(0);
        let composite = matches!(sources[1], KeyGet::RemoteIpAndPath);
        if composite {
            session
                .expect_key_get()
                .withf(|key| matches!(key, KeyGet::RemoteIpAndPath))
                .times(1)
                .return_const(Some("selected".to_string()));
        } else {
            session.expect_key_get().times(0);
        }
        let (value, selected) = plugin.extract_first(&session, &sources).unwrap();
        assert_eq!(value, "selected");
        assert_eq!(matches!(value, Cow::Owned(_)), composite);
        assert!(std::ptr::eq(selected, &sources[1]));
    }
}

#[tokio::test]
async fn borrowed_direct_and_configured_map_routes_preserve_selection() {
    use edgion_resources::resources::edgion_plugins::plugin_configs::RouteByKeyConfMatchMeta;
    use std::collections::HashMap;
    for mode in ["direct", "override", "bypass"] {
        let selected = if mode == "override" { "other" } else { "local" };
        let config = RegionRouteConfig {
            my_region: selected.to_string(),
            key_get: vec![KeyGet::Header {
                name: "x-key".to_string(),
            }],
            route_rules: vec![if mode == "direct" {
                RouteRule::RouteByDirectRegion {
                    key_from: vec![KeyGet::Header {
                        name: "x-region".to_string(),
                    }],
                }
            } else {
                RouteRule::RouteByKeyConfMatch {
                    match_map: HashMap::from([("key".to_string(), "local".to_string())]),
                }
            }],
            route_by_key_conf_match: Some(RouteByKeyConfMatchMeta {
                match_map: HashMap::from([("key".to_string(), "other".to_string())]),
                bypass_cluster_keys: if mode == "bypass" {
                    vec!["key".to_string()]
                } else {
                    vec![]
                },
            }),
            regions: vec![
                make_region("local", 0, 499, "local:80"),
                make_region("other", 500, 999, "other:80"),
            ],
            ..Default::default()
        };
        let plugin = RegionRoute::create(&config, "default");
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("x-region", "local"), ("x-key", "key")]);
        session.expect_key_get().times(0);
        session
            .expect_record_outcome()
            .withf(move |outcome, name| *outcome == DyeOutcome::Success && name == selected)
            .times(1)
            .return_const(());
        assert_eq!(
            plugin
                .run_request(&mut session, &mut PluginLog::new("RegionRoute"))
                .await,
            PluginRunningResult::GoodNext
        );
    }
}

#[tokio::test]
async fn borrowed_hash_input_preserves_passthrough_and_writeback() {
    for passthrough in [false, true] {
        let config = RegionRouteConfig {
            my_region: "local".to_string(),
            key_get: vec![KeyGet::Header {
                name: "x-key".to_string(),
            }],
            hash_key_get: Some(vec![KeyGet::Header {
                name: "x-hash".to_string(),
            }]),
            hash_calc: Some(HashCalcConfig {
                algorithm: HashAlgorithm::Crc32,
                modulo: 1000,
                write_back: vec![KeySet::Ctx {
                    name: "route-hash".to_string(),
                }],
            }),
            route_rules: vec![RouteRule::RouteByHashRange],
            regions: vec![make_region("local", 0, 999, "local:80")],
            ..Default::default()
        };
        let plugin = RegionRoute::create(&config, "default");
        let mut session = MockPluginSession::new();
        setup_header_mocks(
            &mut session,
            &[
                ("x-key", "routing-key"),
                ("x-hash", if passthrough { "5" } else { "invalid" }),
            ],
        );
        session.expect_key_get().times(0);
        if passthrough {
            session.expect_key_set().times(0);
        } else {
            session
                .expect_key_set()
                .withf(|target, value| {
                    matches!(target, KeySet::Ctx { name } if name == "route-hash") && value.as_deref() == Some("554")
                })
                .times(1)
                .returning(|_, _| Ok(()));
        }
        session
            .expect_record_outcome()
            .withf(|outcome, name| *outcome == DyeOutcome::Success && name == "local")
            .times(1)
            .return_const(());
        assert_eq!(
            plugin
                .run_request(&mut session, &mut PluginLog::new("RegionRoute"))
                .await,
            PluginRunningResult::GoodNext
        );
    }
}

fn writeback_config(targets: Vec<KeySet>) -> RegionRouteConfig {
    RegionRouteConfig {
        key_get: vec![KeyGet::Header { name: "x-key".into() }],
        hash_key_get: Some(vec![KeyGet::Header { name: "x-hash".into() }]),
        hash_calc: Some(HashCalcConfig {
            algorithm: HashAlgorithm::Crc32,
            modulo: 1000,
            write_back: targets,
        }),
        route_rules: vec![RouteRule::RouteByHashRange],
        // "routing-key" hashes to 554, outside this range.
        regions: vec![make_region("unused", 0, 10, "unused:80")],
        ..Default::default()
    }
}

#[test]
fn prepared_writeback_targets_preserve_labels_and_move_allocation_to_construction() {
    let targets = vec![
        KeySet::Header { name: "x-a".into() },
        KeySet::ResponseHeader { name: "x-b".into() },
        KeySet::Cookie { name: "c".into() },
        KeySet::Ctx { name: "d".into() },
        KeySet::Header { name: "x-a".into() },
    ];
    let config = writeback_config(targets);
    assert_eq!(
        RegionRoute::prepare_writeback_log_targets(&config),
        "hdr:x-a,res_hdr:x-b,cookie:c,ctx:d,hdr:x-a"
    );
    fn prior_label_assembly(config: &RegionRouteConfig) -> String {
        let mut targets = String::new();
        if let Some(calc) = &config.hash_calc {
            for target in &calc.write_back {
                if !targets.is_empty() {
                    targets.push(',');
                }
                targets.push_str(&target.as_log_str());
            }
        }
        targets
    }
    for count in [0, 1, 8, 64] {
        let config = writeback_config(
            (0..count)
                .map(|i| KeySet::Ctx {
                    name: format!("target-{i}-{}", "x".repeat(32)),
                })
                .collect(),
        );
        let (prior, old_hot) = crate::test_allocation_counter::count_allocation_stats(|| prior_label_assembly(&config));
        let (prepared, cold) = crate::test_allocation_counter::count_allocation_stats(|| {
            RegionRoute::prepare_writeback_log_targets(&config)
        });
        let (borrowed, hot) =
            crate::test_allocation_counter::count_allocation_stats(|| std::hint::black_box(prepared.as_str()));
        assert_eq!(hot.calls, 0);
        assert_eq!(borrowed, prepared);
        assert_eq!(prior, prepared);
        assert!(prepared.capacity() <= prior.capacity());
        if count == 0 {
            assert_eq!(cold.calls, 0);
            assert_eq!(prepared.capacity(), 0);
        } else {
            assert!(cold.calls > count);
        }
        println!(
            "targets={count} removed_label_calls={} removed_requested_bytes={} retained_capacity={} inline={}",
            old_hot.calls,
            old_hot.requested_bytes,
            prepared.capacity(),
            std::mem::size_of::<String>()
        );
    }
    let (empty, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        RegionRoute::prepare_writeback_log_targets(&RegionRouteConfig::default())
    });
    assert!(empty.is_empty());
    assert_eq!(stats.calls, 0);
}

#[tokio::test]
async fn prepared_writeback_logs_keep_generations_modes_failures_and_bypass() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::region_route::plugin::tests",
        "prepared_writeback_logs_keep_generations_modes_failures_and_bypass",
        "EDGION_REGION_WRITEBACK_CHILD",
        "labels",
        || async {
            use edgion_resources::sensitive::SENSITIVE_MODE;
            use std::sync::atomic::Ordering;
            for bound in [false, true] {
                for construction_mode in [0, 1, 2] {
                    SENSITIVE_MODE.store(construction_mode, Ordering::SeqCst);
                    let targets = vec![
                        KeySet::Header { name: "x-a".into() },
                        KeySet::ResponseHeader { name: "x-b".into() },
                        KeySet::Cookie { name: "c".into() },
                        KeySet::Ctx { name: "d".into() },
                        KeySet::Header { name: "x-a".into() },
                    ];
                    let mut config = writeback_config(targets.clone());
                    let old = if bound {
                        RegionRoute::create(&config, "default")
                    } else {
                        RegionRoute::create_inline(&config).unwrap()
                    };
                    config.hash_calc.as_mut().unwrap().write_back = vec![KeySet::Ctx { name: "new".into() }];
                    let new = if bound {
                        RegionRoute::create(&config, "default")
                    } else {
                        RegionRoute::create_inline(&config).unwrap()
                    };
                    drop(config);
                    for mode in [0, 1, 2] {
                        SENSITIVE_MODE.store(mode, Ordering::SeqCst);
                        for (plugin, expected_targets, raw_label) in [
                            (&old, targets.clone(), "hdr:x-a,res_hdr:x-b,cookie:c,ctx:d,hdr:x-a"),
                            (&new, vec![KeySet::Ctx { name: "new".into() }], "ctx:new"),
                        ] {
                            for bypass in [false, true] {
                                let mut session = MockPluginSession::new();
                                setup_header_mocks(
                                    &mut session,
                                    &[
                                        ("x-key", "routing-key"),
                                        ("x-hash", if bypass { "554" } else { "invalid" }),
                                    ],
                                );
                                let mut sequence = mockall::Sequence::new();
                                if bypass {
                                    session.expect_key_set().never();
                                } else {
                                    for (i, expected) in expected_targets.iter().cloned().enumerate() {
                                        session
                                            .expect_key_set()
                                            .withf(move |target, value| {
                                                target.as_log_str() == expected.as_log_str()
                                                    && value.as_deref() == Some("554")
                                            })
                                            .times(1)
                                            .in_sequence(&mut sequence)
                                            .returning(move |_, _| {
                                                if i % 2 == 0 {
                                                    Ok(())
                                                } else {
                                                    Err("fixture failure".into())
                                                }
                                            });
                                    }
                                }
                                let mut log = PluginLog::new("RegionRoute");
                                assert_eq!(
                                    plugin.run_request(&mut session, &mut log).await,
                                    PluginRunningResult::GoodNext
                                );
                                let rendered = log.render_for_test();
                                let lines: Vec<_> = rendered.lines().filter(|line| *line != "--- ").collect();
                                if bypass {
                                    assert_eq!(lines.len(), 1);
                                    assert!(lines[0].starts_with("hash_pass="));
                                } else {
                                    let target = match mode {
                                        0 => raw_label.to_string(),
                                        1 => "***".to_string(),
                                        _ => format!("<E>{raw_label}</E>"),
                                    };
                                    assert_eq!(lines.len(), 3);
                                    assert!(lines[0].starts_with("hash key="));
                                    assert_eq!(lines[1], format!("hash_wb→{target}"));
                                    assert!(lines[2].ends_with("no_range, pass"));
                                }
                            }
                        }
                    }
                }
            }
        },
    )
    .await;
}
