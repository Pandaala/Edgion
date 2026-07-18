//! Runtime for executing TLS route plugins in order (first-deny-wins).

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::plugins::stream::stream_plugin_trait::StreamPluginResult;
use edgion_resources::resources::edgion_stream_plugins::{TlsRoutePluginEntry, TlsRouteStreamPlugin};
use std::sync::Arc;

use super::ip_restriction::TlsRouteIpRestriction;

/// Runtime for executing TLS route stage plugins.
#[derive(Clone)]
pub struct TlsRoutePluginRuntime {
    plugins: Vec<Arc<dyn TlsRoutePlugin>>,
}

impl std::fmt::Debug for TlsRoutePluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsRoutePluginRuntime")
            .field("plugin_count", &self.plugins.len())
            .finish()
    }
}

impl Default for TlsRoutePluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl TlsRoutePluginRuntime {
    pub fn new() -> Self {
        Self { plugins: Vec::new() }
    }

    /// Build runtime from TLS route plugin entries.
    pub fn from_entries(entries: &[TlsRoutePluginEntry], parent_namespace: &str) -> Self {
        let mut plugins: Vec<Arc<dyn TlsRoutePlugin>> = Vec::new();
        // First Block-stamped reason encountered, if any. Inserted at the
        // front of `plugins` after the loop so it dominates the chain
        // regardless of the stamped entry's original position (mirrors the
        // HTTP "gate first" semantics).
        let mut policy_block_reason: Option<String> = None;

        for entry in entries {
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
                        policy_block_reason =
                            Some(format!("plugin disabled by policy: tls-route/{}", entry.type_name()));
                    }
                    continue;
                }
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass) => {
                    tracing::info!(plugin = %entry.type_name(), "tls-route plugin bypassed by plugin policy");
                    continue;
                }
                None => {}
            }

            let plugin: Arc<dyn TlsRoutePlugin> = match &entry.plugin {
                TlsRouteStreamPlugin::IpRestriction(config) => {
                    Arc::new(TlsRouteIpRestriction::new(config, parent_namespace))
                }
            };

            plugins.push(plugin);
        }

        if let Some(reason) = policy_block_reason {
            plugins.insert(0, Arc::new(PolicyBlockTlsRoutePlugin { reason }));
        }

        Self { plugins }
    }

    /// Execute all plugins in order. Returns Deny on first deny, otherwise Allow.
    pub async fn run(&self, ctx: &mut TlsRouteContext) -> StreamPluginResult {
        ctx.push("sni matched");
        for plugin in &self.plugins {
            match plugin.on_tls_route(ctx).await {
                StreamPluginResult::Allow => continue,
                StreamPluginResult::Deny(reason) => return StreamPluginResult::Deny(reason),
            }
        }
        StreamPluginResult::Allow
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

/// Always-deny placeholder for a Block-stamped TLS route plugin entry.
struct PolicyBlockTlsRoutePlugin {
    reason: String,
}

#[async_trait::async_trait]
impl TlsRoutePlugin for PolicyBlockTlsRoutePlugin {
    fn name(&self) -> &str {
        "PolicyBlock"
    }

    async fn on_tls_route(&self, _ctx: &TlsRouteContext) -> StreamPluginResult {
        StreamPluginResult::Deny(self.reason.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_config_data::IpGroup;
    use edgion_resources::resources::edgion_plugins::{DefaultAction, IpRestrictionConfig};
    use edgion_resources::resources::plugin_policy::PluginPolicyAction;
    use std::net::IpAddr;

    fn make_ctx() -> TlsRouteContext {
        TlsRouteContext {
            client_ip: "127.0.0.1".parse::<IpAddr>().unwrap(),
            listener_port: 443,
            sni: "test.example.com".to_string(),
            tls_id: Some("abcd1234".to_string()),
            rsc: crate::Rsc::new("TLSRoute", "default", "test-route"),
            is_mtls: false,
            log: crate::observe::logs::LogBuffer::new(),
        }
    }

    #[tokio::test]
    async fn test_empty_runtime_allows() {
        let rt = TlsRoutePluginRuntime::new();
        assert!(rt.is_empty());
        let mut ctx = make_ctx();
        let result = rt.run(&mut ctx).await;
        assert!(matches!(result, StreamPluginResult::Allow));
        assert!(ctx.log.contains("sni matched"));
    }

    #[tokio::test]
    async fn test_from_empty_entries() {
        let rt = TlsRoutePluginRuntime::from_entries(&[], "default");
        assert!(rt.is_empty());
        assert_eq!(rt.plugin_count(), 0);
    }

    /// A structurally valid IpRestriction config (validate_and_init requires at
    /// least one of allow/deny/allowRefs/denyRefs) that denies an unrelated
    /// range and default-allows everything else — so `make_ctx()`'s
    /// 127.0.0.1 client passes through when the plugin actually runs.
    fn ip_restriction_entry(policy_action: Option<PluginPolicyAction>) -> TlsRoutePluginEntry {
        let config = IpRestrictionConfig {
            deny: Some(vec![IpGroup {
                name: "blocked".to_string(),
                description: None,
                cidrs: vec!["10.0.0.0/8".to_string()],
            }]),
            default_action: DefaultAction::Allow,
            ..Default::default()
        };
        TlsRoutePluginEntry {
            enable: true,
            policy_action,
            plugin: TlsRouteStreamPlugin::IpRestriction(config),
        }
    }

    #[tokio::test]
    async fn block_stamp_denies_connection_bypass_stamp_skips() {
        let blocked = ip_restriction_entry(Some(PluginPolicyAction::Block { status: 503 }));
        let runtime = TlsRoutePluginRuntime::from_entries(&[blocked], "default");
        let mut ctx = make_ctx();
        match runtime.run(&mut ctx).await {
            StreamPluginResult::Deny(reason) => assert!(reason.contains("plugin disabled by policy")),
            other => panic!("expected Deny, got {other:?}"),
        }

        let bypassed = ip_restriction_entry(Some(PluginPolicyAction::Bypass));
        let runtime = TlsRoutePluginRuntime::from_entries(&[bypassed], "default");
        assert!(runtime.is_empty(), "bypassed plugin must not be instantiated");
    }

    #[tokio::test]
    async fn block_stamp_dominates_regardless_of_entry_position() {
        let normal = ip_restriction_entry(None);
        let blocked = ip_restriction_entry(Some(PluginPolicyAction::Block { status: 503 }));
        let runtime = TlsRoutePluginRuntime::from_entries(&[normal, blocked], "default");
        let mut ctx = make_ctx();
        match runtime.run(&mut ctx).await {
            StreamPluginResult::Deny(reason) => {
                assert!(reason.contains("plugin disabled by policy: tls-route/IpRestriction"))
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unstamped_entries_behave_as_before() {
        let entry = ip_restriction_entry(None);
        let runtime = TlsRoutePluginRuntime::from_entries(&[entry], "default");
        assert!(!runtime.is_empty());
        assert_eq!(runtime.plugin_count(), 1);
        let mut ctx = make_ctx();
        assert!(matches!(runtime.run(&mut ctx).await, StreamPluginResult::Allow));
    }
}
