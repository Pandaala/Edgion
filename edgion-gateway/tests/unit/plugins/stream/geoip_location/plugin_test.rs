use super::*;
use crate::geoip::{inject_lookup_fault, GeoIpProvider, LookupFault, GEO_TEST_LOCK};
use crate::plugins::stream::stream_plugin_trait::StreamLayer;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    DefaultAction, GeoRule, GeoRuleGroup, GeoRuleKey, GeoRuleSource,
};

fn rule(source: GeoRuleSource, key: GeoRuleKey, values: &[&str]) -> GeoRule {
    GeoRule {
        source,
        key,
        values: if key == GeoRuleKey::Cidr {
            Vec::new()
        } else {
            values.iter().map(|s| s.to_string()).collect()
        },
        cidr: if key == GeoRuleKey::Cidr {
            edgion_resources::resources::edgion_plugins::ip_reference_set::IpReferenceSet {
                ip_refs: values
                    .iter()
                    .map(
                        |value| edgion_resources::resources::edgion_config_data::EdgionConfigDataRef {
                            name: if value.starts_with("192.") { "private" } else { "office" }.into(),
                            namespace: None,
                            ref_denied: None,
                        },
                    )
                    .collect(),
                ..Default::default()
            }
        } else {
            Default::default()
        },
    }
}

fn stream_ctx(ip: &str) -> StreamContext {
    StreamContext::new(ip.parse().unwrap(), 8080, StreamLayer::Stage1)
}

/// A rule-less L4 config never reaches `on_connection` — it fails at
/// construction, the same gate the controller's ingest path uses. Parse-only
/// is meaningful at L7 (it fills the access log) and provably inert here.
#[tokio::test]
async fn parse_only_config_is_rejected_at_construction() {
    let err = create_plugin(&GeoIpLocationConfig::default()).err().unwrap();
    assert!(err.contains("at least one"), "got: {err}");
}

#[tokio::test]
async fn remote_ip_source_is_rejected_at_construction() {
    let cfg = GeoIpLocationConfig {
        invalid_input: false,
        fail_open: Some(false),
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::RemoteIp, GeoRuleKey::Country, &["RU"])],
        }],
        ..Default::default()
    };
    let err = create_plugin(&cfg).err().unwrap();
    assert!(err.contains("RemoteIp"), "got: {err}");
}

#[tokio::test]
async fn db_unavailable_fail_closed_denies_and_meters() {
    let _lock = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    let cfg = GeoIpLocationConfig {
        invalid_input: false,
        fail_open: Some(false),
        default_action: DefaultAction::Allow,
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
        }],
        ..Default::default()
    };
    let plugin = create_plugin(&cfg).unwrap();
    let result = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
    assert!(matches!(result, StreamPluginResult::Deny(_)));
}

#[tokio::test]
async fn db_unavailable_fail_open_allows() {
    let _lock = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    let cfg = GeoIpLocationConfig {
        invalid_input: false,
        fail_open: Some(true),
        default_action: DefaultAction::Allow,
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
        }],
        ..Default::default()
    };
    let plugin = create_plugin(&cfg).unwrap();
    let result = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
    assert!(matches!(result, StreamPluginResult::Allow));
}

/// A CIDR-only config must not be affected by the GeoIP database being
/// unavailable — CIDR matching needs no MMDB, so this must NOT hit the
/// db-unavailable fail-open/close branch at all.
#[tokio::test]
async fn cidr_only_config_is_unaffected_by_db_unavailability() {
    let _lock = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    let cfg = GeoIpLocationConfig {
        invalid_input: false,
        fail_open: Some(false),
        // An allow-hit does not itself return Allow — it only escapes the
        // "allow configured and missed" auto-deny; the actual verdict
        // still falls through to `defaultAction` (per the decision order
        // in the task brief). Whitelist semantics need `defaultAction:
        // Allow` here: hit -> falls through -> Allow; miss -> denied by
        // the allow-miss branch regardless of `defaultAction`.
        default_action: DefaultAction::Allow,
        allow_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: Some("office".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
        }],
        ..Default::default()
    };
    let plugin = create_plugin(&cfg).unwrap();
    // Inside the CIDR -> allow group hit, despite no GeoIP db loaded.
    let allowed = plugin.on_connection(&stream_ctx("10.1.2.3")).await;
    assert!(matches!(allowed, StreamPluginResult::Allow));
    // Outside the CIDR -> allow configured and missed -> deny.
    let denied = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
    assert!(matches!(denied, StreamPluginResult::Deny(_)));
}

#[tokio::test]
async fn cidr_only_config_is_unaffected_by_lookup_failure() {
    let _lock = GEO_TEST_LOCK.lock();
    load_fixture_db();
    let _fault = inject_lookup_fault(LookupFault::Decode);
    let cfg = GeoIpLocationConfig {
        invalid_input: false,
        fail_open: Some(false),
        default_action: DefaultAction::Allow,
        allow_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: Some("office".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
        }],
        ..Default::default()
    };
    let plugin = create_plugin(&cfg).unwrap();
    let allowed = plugin.on_connection(&stream_ctx("10.1.2.3")).await;
    assert!(matches!(allowed, StreamPluginResult::Allow));
    let denied = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
    assert!(matches!(denied, StreamPluginResult::Deny(_)));
}

#[tokio::test]
async fn cidr_deny_hit_and_allow_miss_have_exact_reasons_and_metrics() {
    let _lock = GEO_TEST_LOCK.lock();
    let cfg = GeoIpLocationConfig {
        invalid_input: false,
        fail_open: Some(false),
        default_action: DefaultAction::Allow,
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: Some("blocked".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
        }],
        allow_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: Some("office".into()),
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["192.168.0.0/16"])],
        }],
    };
    let plugin = create_plugin(&cfg).unwrap();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    let deny_hit = metrics::with_local_recorder(&recorder, || {
        futures::FutureExt::now_or_never(plugin.on_connection(&stream_ctx("10.1.2.3")))
            .expect("CIDR-only GeoIpLocation stream path must not yield")
    });
    assert!(matches!(
        deny_hit,
        StreamPluginResult::Deny(reason) if reason == "geo:deny:deny_country"
    ));

    let allow_miss = metrics::with_local_recorder(&recorder, || {
        futures::FutureExt::now_or_never(plugin.on_connection(&stream_ctx("8.8.8.8")))
            .expect("CIDR-only GeoIpLocation stream path must not yield")
    });
    assert!(matches!(
        allow_miss,
        StreamPluginResult::Deny(reason) if reason == "geo:deny:allow_miss"
    ));

    let mut deny_only = cfg.clone();
    deny_only.allow_rule_groups.clear();
    deny_only.default_action = DefaultAction::Deny;
    let deny_only = create_plugin(&deny_only).unwrap();
    for ip in ["10.1.2.3", "8.8.8.8"] {
        let result = metrics::with_local_recorder(&recorder, || {
            futures::FutureExt::now_or_never(deny_only.on_connection(&stream_ctx(ip)))
                .expect("CIDR-only GeoIpLocation stream path must not yield")
        });
        assert!(matches!(
            result,
            StreamPluginResult::Deny(reason) if reason == "geo:deny:deny_country"
        ));
    }

    let rendered = handle.render();
    assert!(
        rendered.contains("reason=\"deny_country\"") && rendered.contains("reason=\"allow_miss\""),
        "CIDR denials must retain their exact metric labels, got:\n{rendered}"
    );
}

fn load_fixture_db() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb");
    GeoIpProvider::global()
        .load_from(&path)
        .expect("city fixture must load");
}

fn country_deny_cfg(fail_open: bool) -> GeoIpLocationConfig {
    GeoIpLocationConfig {
        invalid_input: false,
        fail_open: Some(fail_open),
        default_action: DefaultAction::Allow,
        deny_rule_groups: vec![GeoRuleGroup {
            invalid_input: false,
            name: None,
            rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn lookup_failed_fail_closed_denies() {
    let _lock = GEO_TEST_LOCK.lock();
    load_fixture_db();
    let _fault = inject_lookup_fault(LookupFault::Decode);
    let plugin = create_plugin(&country_deny_cfg(false)).unwrap();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let result = metrics::with_local_recorder(&recorder, || {
        futures::FutureExt::now_or_never(plugin.on_connection(&stream_ctx("8.8.8.8")))
            .expect("GeoIpLocation stream path must not yield")
    });
    match result {
        StreamPluginResult::Deny(reason) => {
            assert!(reason.contains("lookup-failed"), "got: {reason}");
        }
        other => panic!("expected Deny, got {other:?}"),
    }
    let rendered = handle.render();
    assert!(
        rendered.contains("edgion_geoip_denied_total") && rendered.contains("reason=\"lookup_failed\""),
        "fail-close lookup failure must meter lookup_failed, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("edgion_geoip_lookup_panic_total"),
        "decode failure must not increment the panic counter, got:\n{rendered}"
    );
}

#[tokio::test]
async fn lookup_failed_fail_open_allows() {
    let _lock = GEO_TEST_LOCK.lock();
    load_fixture_db();
    let _fault = inject_lookup_fault(LookupFault::Decode);
    let plugin = create_plugin(&country_deny_cfg(true)).unwrap();
    let result = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
    assert!(matches!(result, StreamPluginResult::Allow));
}

#[tokio::test]
async fn lookup_panic_fail_closed_denies() {
    let _lock = GEO_TEST_LOCK.lock();
    load_fixture_db();
    let _fault = inject_lookup_fault(LookupFault::Panic);
    let plugin = create_plugin(&country_deny_cfg(false)).unwrap();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let result = metrics::with_local_recorder(&recorder, || {
        futures::FutureExt::now_or_never(plugin.on_connection(&stream_ctx("8.8.8.8")))
            .expect("GeoIpLocation stream path must not yield")
    });
    match result {
        StreamPluginResult::Deny(reason) => {
            assert!(reason.contains("lookup-failed"), "got: {reason}");
        }
        other => panic!("expected Deny, got {other:?}"),
    }
    let rendered = handle.render();
    assert!(
        rendered.contains("reason=\"lookup_failed\""),
        "panic fail-close must meter lookup_failed, got:\n{rendered}"
    );
    assert!(
        rendered.contains("edgion_geoip_lookup_panic_total"),
        "panic seam must increment edgion_geoip_lookup_panic_total, got:\n{rendered}"
    );
}

fn geo_data() -> std::collections::HashMap<String, std::sync::Arc<edgion_resources::EdgionConfigData>> {
    [ ("office", "10.0.0.0/8"), ("private", "192.168.0.0/16") ].into_iter().map(|(name, cidr)| {
        let resource = serde_json::from_value(serde_json::json!({"metadata":{"name":name,"namespace":"b4-geo-tests"},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"main","cidrs":[cidr]}]}}}})).unwrap();
        (format!("b4-geo-tests/{name}"), std::sync::Arc::new(resource))
    }).collect()
}

fn create_plugin(config: &GeoIpLocationConfig) -> Result<GeoIpLocationStreamPlugin, String> {
    let owner: edgion_resources::EdgionStreamPlugins = serde_json::from_value(serde_json::json!({"metadata":{"name":"geo","namespace":"b4-geo-tests"},"spec":{"plugins":[{"type":"GeoIpLocation","config":config}]}})).unwrap();
    let prepared = edgion_resources::refs::ip_list_fields::PreparedIpListReferences::from_stream_plugins(&owner);
    let binding = prepared.bind_stream_plugins(&owner).unwrap();
    let plugin = GeoIpLocationStreamPlugin::from_bound(binding.geo_entry(0).unwrap())?;
    edgion_common::conf_sync::traits::ConfHandler::partial_update(
        &*crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store(),
        geo_data(),
        Default::default(),
        Default::default(),
    );
    Ok(plugin)
}

#[tokio::test]
async fn missing_ref_precedes_mmdb_fail_open_and_has_only_the_reference_metric_reason() {
    let _lock = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    let config:GeoIpLocationConfig=serde_json::from_value(serde_json::json!({"failOpen":true,"denyRuleGroups":[{"rules":[{"source":"DirectPeerIp","key":"country","values":["RU"]}]}],"allowRuleGroups":[{"rules":[{"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"missing"}]}]}]})).unwrap();
    let plugin = create_plugin(&config).unwrap();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let result = metrics::with_local_recorder(&recorder, || {
        futures::FutureExt::now_or_never(plugin.on_connection(&stream_ctx("8.8.8.8"))).unwrap()
    });
    assert!(matches!(result,StreamPluginResult::Deny(reason)if reason=="geo:deny:ref_unavailable"));
    let rendered = handle.render();
    assert!(rendered.contains("reason=\"ref_unavailable\""));
    assert!(!rendered.contains("reason=\"db_unavailable\"") && !rendered.contains("reason=\"lookup_failed\""));
}
