//! Stream plugin runtime for executing stream plugins

use super::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use crate::plugins::stream::{
    ConnectionRateLimitPlugin, GeoIpLocationStreamPlugin, GlobalConnectionIpRestrictionPlugin, StreamIpRestriction,
};
use edgion_resources::resources::edgion_plugins::PluginValidation;
use edgion_resources::resources::edgion_stream_plugins::{EdgionStreamPlugin, StreamPluginEntry};
use std::sync::Arc;

/// Runtime for executing stream plugins
#[derive(Clone)]
pub struct StreamPluginRuntime {
    /// Ordered list of plugins to execute
    plugins: Vec<Arc<dyn StreamPlugin>>,
}

impl std::fmt::Debug for StreamPluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamPluginRuntime")
            .field("plugin_count", &self.plugins.len())
            .finish()
    }
}

impl Default for StreamPluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamPluginRuntime {
    /// Create an empty runtime
    pub fn new() -> Self {
        Self { plugins: Vec::new() }
    }

    /// Create runtime from stream plugin entries.
    ///
    /// `parent_namespace` is the namespace of the owning `EdgionStreamPlugins` resource,
    /// used by `GlobalConnectionIpRestrictionPlugin` to resolve `overrideRef` with no
    /// explicit namespace.
    ///
    /// `resource_key` is that resource's store key; combined with the entry index it
    /// gives `ConnectionRateLimit` the identity it needs to namespace its Redis
    /// buckets and its local gate (see `plugins::common::entry_scope`).
    pub fn from_stream_plugins(entries: &[StreamPluginEntry], parent_namespace: &str, resource_key: &str) -> Self {
        let record_invalid_config =
            |invalid_config_reason: &mut Option<String>, plugin: &'static str, reason: &'static str| {
                tracing::error!(
                    component = "stream_plugin_runtime",
                    plugin,
                    resource_key,
                    error_class = "invalid configuration",
                    "Invalid stream plugin configuration; all connections will be denied",
                );
                if invalid_config_reason.is_none() {
                    *invalid_config_reason = Some(reason.to_string());
                }
            };
        let mut plugins: Vec<Arc<dyn StreamPlugin>> = Vec::new();
        // First Block-stamped reason encountered, if any. Inserted at the
        // front of `plugins` after the loop so it dominates the chain
        // regardless of the stamped entry's original position (mirrors the
        // HTTP "gate first" semantics).
        let mut policy_block_reason: Option<String> = None;
        let mut invalid_config_reason: Option<String> = None;

        for (index, entry) in entries.iter().enumerate() {
            if !entry.is_enabled() {
                continue;
            }

            // Plugin policy: stamped entries are never instantiated. Block
            // denies the connection; no HTTP status exists in stream plugin
            // lifecycles, so the stamped status is ignored per spec §5.
            // Bypass skips silently
            // at the chain level; visibility comes from the controller's
            // PluginPolicyRestricted condition and a compile-time log line.
            match &entry.policy_action {
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Block { .. }) => {
                    if policy_block_reason.is_none() {
                        policy_block_reason = Some(format!("plugin disabled by policy: stream/{}", entry.type_name()));
                    }
                    continue;
                }
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass) => {
                    tracing::info!(plugin = %entry.type_name(), "stream plugin bypassed by plugin policy");
                    continue;
                }
                None => {}
            }

            let plugin: Arc<dyn StreamPlugin> = match &entry.plugin {
                EdgionStreamPlugin::IpRestriction(rules) => {
                    if !rules.legacy_http_fields().is_empty() || rules.validation_error().is_some() {
                        record_invalid_config(
                            &mut invalid_config_reason,
                            "IpRestriction",
                            "invalid stream/IpRestriction configuration",
                        );
                        continue;
                    }
                    Arc::new(StreamIpRestriction::new(rules, parent_namespace))
                }
                EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) => {
                    match GlobalConnectionIpRestrictionPlugin::new(gcir, parent_namespace) {
                        Ok(p) => Arc::new(p),
                        Err(_) => {
                            record_invalid_config(
                                &mut invalid_config_reason,
                                "GlobalConnectionIpRestriction",
                                "invalid stream/GlobalConnectionIpRestriction configuration",
                            );
                            continue;
                        }
                    }
                }
                EdgionStreamPlugin::ConnectionRateLimit(cfg) => {
                    if cfg.validation_error().is_some() {
                        record_invalid_config(
                            &mut invalid_config_reason,
                            "ConnectionRateLimit",
                            "invalid stream/ConnectionRateLimit configuration",
                        );
                        continue;
                    }
                    Arc::new(ConnectionRateLimitPlugin::new(
                        cfg,
                        // Stream entries carry no alias, so the scope always uses the index form.
                        &crate::plugins::common::entry_scope(resource_key, None, index),
                    ))
                }
                EdgionStreamPlugin::GeoIpLocation(cfg) => match GeoIpLocationStreamPlugin::new(cfg) {
                    Ok(p) => Arc::new(p),
                    Err(_) => {
                        record_invalid_config(
                            &mut invalid_config_reason,
                            "GeoIpLocation",
                            "invalid stream/GeoIpLocation configuration",
                        );
                        continue;
                    }
                },
            };

            plugins.push(plugin);
        }

        if let Some(reason) = policy_block_reason.or(invalid_config_reason) {
            plugins.insert(0, Arc::new(FailClosedStreamPlugin { reason }));
        }

        Self { plugins }
    }

    /// Execute all plugins in order
    /// Returns Deny on first plugin that denies, otherwise Allow
    pub async fn run(&self, ctx: &StreamContext) -> StreamPluginResult {
        for plugin in &self.plugins {
            match plugin.on_connection(ctx).await {
                StreamPluginResult::Allow => continue,
                StreamPluginResult::Deny(reason) => {
                    return StreamPluginResult::Deny(reason);
                }
            }
        }

        StreamPluginResult::Allow
    }

    /// Check if runtime has any plugins
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

/// Always-deny gate for policy blocks and invalid published configurations.
struct FailClosedStreamPlugin {
    reason: String,
}

#[async_trait::async_trait]
impl StreamPlugin for FailClosedStreamPlugin {
    fn name(&self) -> &str {
        "FailClosed"
    }

    async fn on_connection(&self, _ctx: &StreamContext) -> StreamPluginResult {
        StreamPluginResult::Deny(self.reason.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::stream::stream_plugin_trait::StreamLayer;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        DefaultAction, GeoIpLocationConfig, GeoRule, GeoRuleGroup, GeoRuleKey, GeoRuleSource,
    };
    use edgion_resources::resources::edgion_plugins::{OnRedisFailure, RateLimitAlgorithm};
    use edgion_resources::resources::edgion_stream_plugins::{
        ConnectionIpRestrictionRules, ConnectionRateLimitConfig, GlobalConnectionIpRestrictionConfig, PerListenerLimit,
    };
    use edgion_resources::resources::plugin_policy::PluginPolicyAction;
    use std::collections::HashMap;
    use std::net::IpAddr;

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
        StreamPluginEntry {
            enable: true,
            policy_action: None,
            plugin,
        }
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
    async fn block_stamp_denies_connection_bypass_stamp_skips() {
        let blocked = StreamPluginEntry {
            enable: true,
            policy_action: Some(PluginPolicyAction::Block { status: 503 }),
            plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(10)),
        };
        let runtime = StreamPluginRuntime::from_stream_plugins(&[blocked], "default", "default/test-esp");
        let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 8080, StreamLayer::Stage1);
        match runtime.run(&ctx).await {
            StreamPluginResult::Deny(reason) => assert!(reason.contains("plugin disabled by policy")),
            other => panic!("expected Deny, got {other:?}"),
        }

        let bypassed = StreamPluginEntry {
            enable: true,
            policy_action: Some(PluginPolicyAction::Bypass),
            plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(10)),
        };
        let runtime = StreamPluginRuntime::from_stream_plugins(&[bypassed], "default", "default/test-esp");
        assert!(runtime.is_empty(), "bypassed plugin must not be instantiated");
    }

    #[tokio::test]
    async fn block_stamp_dominates_regardless_of_entry_position() {
        // A Block-stamped entry after a normal plugin must still gate first:
        // the deny must fire before the earlier, unstamped plugin ever runs.
        let normal = StreamPluginEntry {
            enable: true,
            policy_action: None,
            plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(1_000_000)),
        };
        let blocked = StreamPluginEntry {
            enable: true,
            policy_action: Some(PluginPolicyAction::Block { status: 503 }),
            plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(10)),
        };
        let runtime = StreamPluginRuntime::from_stream_plugins(&[normal, blocked], "default", "default/test-esp");
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
            policy_action: None,
            plugin: EdgionStreamPlugin::IpRestriction(ConnectionIpRestrictionRules::default()),
        };
        let mut invalid_rate = cfg_local_only(1);
        invalid_rate.per_listener = None;
        let bypassed = StreamPluginEntry {
            enable: true,
            policy_action: Some(PluginPolicyAction::Bypass),
            plugin: EdgionStreamPlugin::ConnectionRateLimit(invalid_rate),
        };

        let runtime =
            StreamPluginRuntime::from_stream_plugins(&[disabled, bypassed], "default", "default/inert-invalid-entries");
        assert!(runtime.is_empty());
    }

    #[tokio::test]
    async fn invalid_geoip_config_installs_an_always_deny_gate() {
        let invalid = StreamPluginEntry {
            enable: true,
            policy_action: None,
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
                    policy_action: None,
                    plugin: EdgionStreamPlugin::GeoIpLocation(GeoIpLocationConfig::default()),
                },
                StreamPluginEntry {
                    enable: true,
                    policy_action: None,
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
            policy_action: Some(PluginPolicyAction::Block { status: 503 }),
            plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(10)),
        };
        let mut entries = geo_entries().to_vec();
        entries.push(blocked);
        let runtime = StreamPluginRuntime::from_stream_plugins(&entries, "default", "default/test-esp");

        assert!(matches!(
            runtime.run(&ctx).await,
            StreamPluginResult::Deny(reason)
                if reason == "plugin disabled by policy: stream/ConnectionRateLimit"
        ));
    }

    #[tokio::test]
    async fn unstamped_entries_behave_as_before() {
        let entry = StreamPluginEntry {
            enable: true,
            policy_action: None,
            plugin: EdgionStreamPlugin::ConnectionRateLimit(cfg_local_only(1_000_000)),
        };
        let runtime = StreamPluginRuntime::from_stream_plugins(&[entry], "default", "default/test-esp");
        assert!(!runtime.is_empty());
        let ctx = StreamContext::new(IpAddr::from([127, 0, 0, 1]), 8082, StreamLayer::Stage1);
        assert!(matches!(runtime.run(&ctx).await, StreamPluginResult::Allow));
    }
}
