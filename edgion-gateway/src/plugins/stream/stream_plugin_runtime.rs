//! Stream plugin runtime for executing stream plugins

use super::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use crate::plugins::stream::{
    ConnectionRateLimitPlugin, GeoIpLocationStreamPlugin, GlobalConnectionIpRestrictionPlugin, StreamIpRestriction,
};
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
        let mut plugins: Vec<Arc<dyn StreamPlugin>> = Vec::new();
        // First Block-stamped reason encountered, if any. Inserted at the
        // front of `plugins` after the loop so it dominates the chain
        // regardless of the stamped entry's original position (mirrors the
        // HTTP "gate first" semantics).
        let mut policy_block_reason: Option<String> = None;

        for (index, entry) in entries.iter().enumerate() {
            if !entry.is_enabled() {
                continue;
            }

            // Plugin policy: stamped entries are never instantiated. Block
            // denies the connection (no HTTP status exists pre-TLS; the
            // stamped status is ignored per spec §5). Bypass skips silently
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
                EdgionStreamPlugin::IpRestriction(rules) => Arc::new(StreamIpRestriction::new(rules, parent_namespace)),
                EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) => {
                    match GlobalConnectionIpRestrictionPlugin::new(gcir, parent_namespace) {
                        Ok(p) => Arc::new(p),
                        Err(e) => {
                            // Base validation failed — should not happen if controller validated
                            // before syncing, but log and skip the plugin rather than panicking.
                            tracing::error!(
                                error = %e,
                                "GlobalConnectionIpRestriction plugin skipped: base config failed validation"
                            );
                            continue;
                        }
                    }
                }
                EdgionStreamPlugin::ConnectionRateLimit(cfg) => Arc::new(ConnectionRateLimitPlugin::new(
                    cfg,
                    // Stream entries carry no alias, so the scope always uses the index form.
                    &crate::plugins::common::entry_scope(resource_key, None, index),
                )),
                EdgionStreamPlugin::GeoIpLocation(cfg) => match GeoIpLocationStreamPlugin::new(cfg) {
                    Ok(p) => Arc::new(p),
                    Err(e) => {
                        // Should not happen if the controller validated before syncing
                        // (both paths share `GeoIpLocationConfig::check_fields`); log and
                        // skip rather than panic.
                        tracing::error!(error = %e, "GeoIpLocation stream plugin skipped: config failed validation");
                        continue;
                    }
                },
            };

            plugins.push(plugin);
        }

        if let Some(reason) = policy_block_reason {
            plugins.insert(0, Arc::new(PolicyBlockStreamPlugin { reason }));
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

/// Always-deny placeholder for a Block-stamped stream plugin entry.
struct PolicyBlockStreamPlugin {
    reason: String,
}

#[async_trait::async_trait]
impl StreamPlugin for PolicyBlockStreamPlugin {
    fn name(&self) -> &str {
        "PolicyBlock"
    }

    async fn on_connection(&self, _ctx: &StreamContext) -> StreamPluginResult {
        StreamPluginResult::Deny(self.reason.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::stream::stream_plugin_trait::StreamLayer;
    use edgion_resources::resources::edgion_plugins::{OnRedisFailure, RateLimitAlgorithm};
    use edgion_resources::resources::edgion_stream_plugins::{ConnectionRateLimitConfig, PerListenerLimit};
    use edgion_resources::resources::plugin_policy::PluginPolicyAction;
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
