use super::*;
use crate::plugins::stream::stream_plugin_trait::StreamLayer;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    DefaultAction, GeoIpLocationConfig, GeoRule, GeoRuleGroup, GeoRuleKey, GeoRuleSource,
};
use edgion_resources::resources::edgion_plugins::{OnRedisFailure, RateLimitAlgorithm};
use edgion_resources::resources::edgion_stream_plugins::{
    ConnectionIpRestrictionRules, ConnectionRateLimitConfig, GlobalConnectionIpRestrictionConfig, PerListenerLimit,
};
use edgion_resources::resources::plugin_policy::{PluginPolicyConfig, PolicyActionKind};
use std::collections::HashMap;
use std::net::IpAddr;

fn policy_runtime(entries: &[StreamPluginEntry], key: &str, action: PolicyActionKind) -> StreamPluginRuntime {
    let mut policy = PluginPolicyConfig::default();
    policy
        .deny
        .push(edgion_resources::resources::plugin_policy::PluginPolicyEntryConfig {
            name: "stream/ConnectionRateLimit".to_string(),
            action: Some(action),
            block_status: None,
        });
    let policy = policy.compile().unwrap();
    StreamPluginRuntime::from_cached_entries(entries, "default", key, Some(&policy), &mut vec![None; entries.len()])
}

fn cfg_local_only(rate: u64) -> ConnectionRateLimitConfig {
    ConnectionRateLimitConfig {
        redis_ref: None,
        algorithm: RateLimitAlgorithm::SlidingWindow,
        key_prefix: "edgion:crl:".to_string(),
        per_listener: Some(PerListenerLimit {
            rate,
            interval: "1s".to_string(),
            redis_distributed: false,
            on_redis_failure: OnRedisFailure::Deny,
            interval_duration: None,
        }),
        per_source_ip: None,
        redis_timeout: "100ms".to_string(),
    }
}

fn valid_geoip_config() -> GeoIpLocationConfig {
    GeoIpLocationConfig {
        fail_open: Some(false),
        default_action: DefaultAction::Allow,
        deny_rule_groups: vec![GeoRuleGroup {
            name: None,
            rules: vec![GeoRule {
                source: GeoRuleSource::DirectPeerIp,
                key: GeoRuleKey::Cidr,
                values: vec!["10.0.0.0/8".to_string()],
                cidr_matcher: None,
            }],
        }],
        ..Default::default()
    }
}

fn entry(plugin: EdgionStreamPlugin) -> StreamPluginEntry {
    StreamPluginEntry { enable: true, plugin }
}

#[tokio::test]
async fn blocked_bypassed_disabled_and_invalid_chains_cannot_activate_new_budgets() {
    for mode in ["block", "bypass", "disabled", "invalid"] {
        let key = format!("default/local-gate-inert-{mode}");
        let mut cfg = cfg_local_only(1);
        cfg.per_listener.as_mut().unwrap().interval = "1h".to_string();
        let old = StreamPluginRuntime::from_stream_plugins(
            &[entry(EdgionStreamPlugin::ConnectionRateLimit(cfg.clone()))],
            "default",
            &key,
        );
        let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 18090, StreamLayer::Stage1);
        assert!(matches!(old.run(&ctx).await, StreamPluginResult::Allow));
        cfg.per_listener.as_mut().unwrap().rate = 100;
        let mut candidate = entry(EdgionStreamPlugin::ConnectionRateLimit(cfg.clone()));
        if mode == "disabled" {
            candidate.enable = false;
        }
        let mut entries = vec![candidate];
        if mode == "invalid" {
            cfg.per_listener.as_mut().unwrap().rate = 0;
            entries.push(entry(EdgionStreamPlugin::ConnectionRateLimit(cfg)));
        }
        let next = match mode {
            "block" => policy_runtime(&entries, &key, PolicyActionKind::Block),
            "bypass" => policy_runtime(&entries, &key, PolicyActionKind::Bypass),
            _ => StreamPluginRuntime::from_stream_plugins(&entries, "default", &key),
        };
        let outcome = next.run(&ctx).await;
        if matches!(mode, "block" | "invalid") {
            assert!(matches!(outcome, StreamPluginResult::Deny(_)));
        } else {
            assert!(matches!(outcome, StreamPluginResult::Allow));
        }
        assert!(
            matches!(old.run(&ctx).await, StreamPluginResult::Deny(_)),
            "{mode} activated a budget"
        );
    }
}

#[tokio::test]
async fn repeated_listener_route_attachment_consumes_twice_and_other_entries_are_independent() {
    let mut cfg = cfg_local_only(2);
    cfg.per_listener.as_mut().unwrap().interval = "1h".to_string();
    let key = "default/local-gate-layered-runtime";
    let one = entry(EdgionStreamPlugin::ConnectionRateLimit(cfg.clone()));
    let runtime = StreamPluginRuntime::from_stream_plugins(std::slice::from_ref(&one), "default", key);
    let listener = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 18091, StreamLayer::Stage1);
    let route = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 18091, StreamLayer::Tcp);
    assert!(matches!(runtime.run(&listener).await, StreamPluginResult::Allow));
    assert!(matches!(runtime.run(&route).await, StreamPluginResult::Allow));
    assert!(matches!(runtime.run(&listener).await, StreamPluginResult::Deny(_)));

    let other = StreamPluginRuntime::from_stream_plugins(
        std::slice::from_ref(&one),
        "default",
        "default/local-gate-other-runtime",
    );
    assert!(matches!(other.run(&listener).await, StreamPluginResult::Allow));
    // Disabled index 0 is inert; index 1 has its own untouched budget.
    let mut disabled = one.clone();
    disabled.enable = false;
    let second_entry = StreamPluginRuntime::from_stream_plugins(&[disabled, one], "default", key);
    assert!(matches!(second_entry.run(&listener).await, StreamPluginResult::Allow));
    assert!(matches!(second_entry.run(&listener).await, StreamPluginResult::Allow));
    assert!(matches!(second_entry.run(&listener).await, StreamPluginResult::Deny(_)));
}

async fn assert_invalid_gate_precedes_rate_limit(
    invalid_plugin: EdgionStreamPlugin,
    expected_reason: &str,
    resource_key: &str,
) {
    let mut one_token = cfg_local_only(1);
    one_token
        .per_listener
        .as_mut()
        .expect("test config must contain a per-listener limit")
        .interval = "1h".to_string();
    let entries = [
        entry(EdgionStreamPlugin::ConnectionRateLimit(one_token.clone())),
        entry(invalid_plugin),
    ];
    let runtime = StreamPluginRuntime::from_stream_plugins(&entries, "default", resource_key);
    let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 18080, StreamLayer::Stage1);
    assert!(matches!(
        runtime.run(&ctx).await,
        StreamPluginResult::Deny(reason) if reason == expected_reason
    ));

    // Reuse the same resource/index identity. If the earlier valid plugin had
    // run before the invalid-config gate, its only token would already be gone.
    let valid_only = StreamPluginRuntime::from_stream_plugins(
        &[entry(EdgionStreamPlugin::ConnectionRateLimit(one_token))],
        "default",
        resource_key,
    );
    assert!(matches!(valid_only.run(&ctx).await, StreamPluginResult::Allow));
    assert!(matches!(valid_only.run(&ctx).await, StreamPluginResult::Deny(_)));
}

#[tokio::test]
async fn block_policy_denies_connection_bypass_policy_skips() {
    let blocked = StreamPluginEntry {
        enable: true,
        plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(10)),
    };
    let runtime = policy_runtime(&[blocked], "default/test-esp", PolicyActionKind::Block);
    let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 8080, StreamLayer::Stage1);
    match runtime.run(&ctx).await {
        StreamPluginResult::Deny(reason) => assert!(reason.contains("plugin disabled by policy")),
        other => panic!("expected Deny, got {other:?}"),
    }

    let bypassed = StreamPluginEntry {
        enable: true,
        plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(10)),
    };
    let runtime = policy_runtime(&[bypassed], "default/test-esp", PolicyActionKind::Bypass);
    assert!(runtime.is_empty(), "bypassed plugin must not be instantiated");
}

#[tokio::test]
async fn block_policy_dominates_regardless_of_entry_position() {
    // A restricted entry after a normal plugin must still gate first.
    let normal = StreamPluginEntry {
        enable: true,
        plugin: EdgionStreamPlugin::GeoIpLocation(valid_geoip_config()),
    };
    let blocked = StreamPluginEntry {
        enable: true,
        plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(10)),
    };
    let runtime = policy_runtime(&[normal, blocked], "default/test-esp", PolicyActionKind::Block);
    let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 8081, StreamLayer::Stage1);
    match runtime.run(&ctx).await {
        StreamPluginResult::Deny(reason) => {
            assert!(reason.contains("plugin disabled by policy: stream/ConnectionRateLimit"))
        }
        other => panic!("expected Deny, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_ip_restriction_is_a_front_gate_without_state_consumption() {
    assert_invalid_gate_precedes_rate_limit(
        EdgionStreamPlugin::IpRestriction(ConnectionIpRestrictionRules::default()),
        "invalid stream/IpRestriction configuration",
        "default/invalid-ip-restriction-front-gate",
    )
    .await;
}

#[tokio::test]
async fn legacy_http_ip_field_is_a_front_gate_without_state_consumption() {
    let rules: ConnectionIpRestrictionRules = serde_json::from_str(
        r#"{
                "allow": [{"name": "office", "cidrs": ["127.0.0.0/8"]}],
                "message": "legacy response text"
            }"#,
    )
    .expect("legacy flattened field and valid stream rules must deserialize");
    assert_invalid_gate_precedes_rate_limit(
        EdgionStreamPlugin::IpRestriction(rules),
        "invalid stream/IpRestriction configuration",
        "default/legacy-http-ip-field-front-gate",
    )
    .await;
}

#[tokio::test]
async fn invalid_connection_rate_limit_is_a_front_gate_without_state_consumption() {
    let mut invalid = cfg_local_only(1);
    invalid.per_listener = None;
    assert_invalid_gate_precedes_rate_limit(
        EdgionStreamPlugin::ConnectionRateLimit(invalid),
        "invalid stream/ConnectionRateLimit configuration",
        "default/invalid-connection-rate-limit-front-gate",
    )
    .await;
}

#[tokio::test]
async fn invalid_disabled_inner_gcir_is_a_front_gate_without_state_consumption() {
    let invalid = GlobalConnectionIpRestrictionConfig {
        enable: false,
        active_profile: "missing".to_string(),
        profiles: HashMap::new(),
        description: None,
        active_profile_ref: None,
    };
    assert_invalid_gate_precedes_rate_limit(
        EdgionStreamPlugin::GlobalConnectionIpRestriction(invalid),
        "invalid stream/GlobalConnectionIpRestriction configuration",
        "default/invalid-gcir-front-gate",
    )
    .await;
}

#[tokio::test]
async fn disabled_and_policy_bypassed_invalid_entries_remain_inert() {
    let disabled = StreamPluginEntry {
        enable: false,
        plugin: EdgionStreamPlugin::IpRestriction(ConnectionIpRestrictionRules::default()),
    };
    let mut invalid_rate = cfg_local_only(1);
    invalid_rate.per_listener = None;
    let bypassed = StreamPluginEntry {
        enable: true,
        plugin: EdgionStreamPlugin::ConnectionRateLimit(invalid_rate),
    };

    let runtime = policy_runtime(
        &[disabled, bypassed],
        "default/inert-invalid-entries",
        PolicyActionKind::Bypass,
    );
    assert!(runtime.is_empty());
}

#[tokio::test]
async fn invalid_geoip_config_installs_an_always_deny_gate() {
    let invalid = StreamPluginEntry {
        enable: true,
        plugin: EdgionStreamPlugin::GeoIpLocation(GeoIpLocationConfig::default()),
    };
    let runtime = StreamPluginRuntime::from_stream_plugins(&[invalid], "default", "default/test-esp");
    assert!(!runtime.is_empty());

    let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 8080, StreamLayer::Stage1);
    assert!(matches!(
        runtime.run(&ctx).await,
        StreamPluginResult::Deny(reason)
            if reason == "invalid stream/GeoIpLocation configuration"
    ));
}

#[tokio::test]
async fn invalid_geoip_config_cannot_be_undone_and_policy_block_stays_first() {
    let geo_entries = || {
        [
            StreamPluginEntry {
                enable: true,
                plugin: EdgionStreamPlugin::GeoIpLocation(GeoIpLocationConfig::default()),
            },
            StreamPluginEntry {
                enable: true,
                plugin: EdgionStreamPlugin::GeoIpLocation(valid_geoip_config()),
            },
        ]
    };
    let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 8080, StreamLayer::Stage1);

    let runtime = StreamPluginRuntime::from_stream_plugins(&geo_entries(), "default", "default/test-esp");
    assert!(matches!(
        runtime.run(&ctx).await,
        StreamPluginResult::Deny(reason)
            if reason == "invalid stream/GeoIpLocation configuration"
    ));

    let blocked = StreamPluginEntry {
        enable: true,
        plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(10)),
    };
    let mut entries = geo_entries().to_vec();
    entries.push(blocked);
    let runtime = policy_runtime(&entries, "default/test-esp", PolicyActionKind::Block);

    assert!(matches!(
        runtime.run(&ctx).await,
        StreamPluginResult::Deny(reason)
            if reason == "plugin disabled by policy: stream/ConnectionRateLimit"
    ));
}

#[tokio::test]
async fn standalone_entries_are_allowed() {
    let entry = StreamPluginEntry {
        enable: true,
        plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(1_000_000)),
    };
    let runtime = StreamPluginRuntime::from_stream_plugins(&[entry], "default", "default/test-esp");
    assert!(!runtime.is_empty());
    let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 8082, StreamLayer::Stage1);
    assert!(matches!(runtime.run(&ctx).await, StreamPluginResult::Allow));
}

#[tokio::test]
async fn unsupported_direct_stream_entries_deny_before_disable_and_policy_checks() {
    let policy = PluginPolicyConfig {
        allow: Some(Vec::new()),
        denied_action: PolicyActionKind::Bypass,
        ..Default::default()
    }
    .compile()
    .unwrap();
    let ctx = StreamContext::new("127.0.0.1".parse().unwrap(), 443, StreamLayer::Stage1);
    for enabled in [true, false] {
        let entry: StreamPluginEntry = serde_json::from_value(serde_json::json!({
            "type":"FuturePlugin", "enable":enabled, "config":{}
        }))
        .unwrap();
        let mut cache = vec![None];
        let runtime =
            StreamPluginRuntime::from_cached_entries(&[entry], "default", "default/unknown", Some(&policy), &mut cache);
        assert!(matches!(runtime.run(&ctx).await, StreamPluginResult::Deny(_)));
        assert!(cache[0].is_none());
    }
}

use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
impl StreamPluginRuntime {
    pub(crate) fn from_stream_plugins(entries: &[StreamPluginEntry], namespace: &str, resource_key: &str) -> Self {
        Self::from_cached_entries(entries, namespace, resource_key, None, &mut vec![None; entries.len()])
    }
    pub(crate) fn from_cached_entries(
        entries: &[StreamPluginEntry],
        namespace: &str,
        resource_key: &str,
        policy: Option<&CompiledPluginPolicy>,
        cache: &mut [Option<StreamEntryOutcome>],
    ) -> Self {
        let owner: edgion_resources::EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "fixture", "namespace": namespace}, "spec": {"plugins": entries}
        }))
        .unwrap();
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        let binding = prepared.bind_stream_plugins(&owner).unwrap();
        Self::from_owner_cached(&binding, resource_key, policy, cache)
    }
}
