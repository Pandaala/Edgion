//! Runtime for executing TLS route plugins in order (first-deny-wins).

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::plugins::stream::stream_plugin_trait::StreamPluginResult;
use edgion_resources::resources::edgion_stream_plugins::{TlsRoutePluginEntry, TlsRouteStreamPlugin};
use edgion_resources::resources::plugin_policy::{CompiledPluginPolicy, PluginFamily};
use std::sync::Arc;

use super::ip_restriction::TlsRouteIpRestriction;

/// Runtime for executing TLS route stage plugins.
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

    #[cfg(test)]
    pub(crate) fn from_test_plugins(plugins: Vec<Arc<dyn TlsRoutePlugin>>) -> Self {
        Self { plugins }
    }

    /// Build runtime from TLS route plugin entries.
    pub fn from_entries(entries: &[TlsRoutePluginEntry], parent_namespace: &str) -> Self {
        Self::from_cached_entries(entries, parent_namespace, None, &mut vec![None; entries.len()])
    }

    pub(crate) fn from_cached_entries(
        entries: &[TlsRoutePluginEntry],
        parent_namespace: &str,
        policy: Option<&CompiledPluginPolicy>,
        cache: &mut [Option<Arc<dyn TlsRoutePlugin>>],
    ) -> Self {
        let mut plugins: Vec<Arc<dyn TlsRoutePlugin>> = Vec::new();
        // First policy Block reason encountered, if any. Inserted at the
        // front of `plugins` after the loop so it dominates the chain
        // regardless of the restricted entry's original position (mirrors the
        // HTTP "gate first" semantics).
        let mut policy_block_reason: Option<String> = None;

        for (index, entry) in entries.iter().enumerate() {
            if !entry.is_enabled() {
                continue;
            }

            // Policy is selected by the admitted Gateway scope before entry
            // construction. Stream admission has no HTTP response status.
            let action = policy.and_then(|policy| policy.action_for(PluginFamily::TlsRoute, entry.type_name()));
            match action {
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Block { .. }) => {
                    if policy_block_reason.is_none() {
                        policy_block_reason =
                            Some(format!("plugin disabled by policy: tls-route/{}", entry.type_name()));
                    }
                    continue;
                }
                Some(edgion_resources::resources::plugin_policy::PluginPolicyAction::Bypass) => {
                    continue;
                }
                None => {}
            }

            let plugin = cache[index].get_or_insert_with(|| match &entry.plugin {
                TlsRouteStreamPlugin::IpRestriction(config) => {
                    Arc::new(TlsRouteIpRestriction::new(config, parent_namespace))
                }
            });

            plugins.push(Arc::clone(plugin));
        }

        if let Some(reason) = policy_block_reason {
            plugins.insert(0, Arc::new(PolicyBlockTlsRoutePlugin { reason }));
        }

        Self { plugins }
    }

    /// Execute all plugins in order. Returns Deny on first deny, otherwise Allow.
    pub async fn run(&self, ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
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

/// Always-deny placeholder for a policy-blocked TLS route plugin entry.
struct PolicyBlockTlsRoutePlugin {
    reason: String,
}

#[async_trait::async_trait]
impl TlsRoutePlugin for PolicyBlockTlsRoutePlugin {
    async fn on_tls_route(&self, _ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
        StreamPluginResult::Deny(self.reason.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_config_data::IpGroup;
    use edgion_resources::resources::edgion_plugins::{DefaultAction, IpRestrictionConfig};
    use edgion_resources::resources::plugin_policy::{PluginPolicyConfig, PolicyActionKind};
    use std::net::IpAddr;

    fn policy_runtime(entries: &[TlsRoutePluginEntry], action: PolicyActionKind) -> TlsRoutePluginRuntime {
        let mut policy = PluginPolicyConfig::default();
        policy
            .deny
            .push(edgion_resources::resources::plugin_policy::PluginPolicyEntryConfig {
                name: "tls-route/IpRestriction".to_string(),
                action: Some(action),
                block_status: None,
            });
        let policy = policy.compile().unwrap();
        TlsRoutePluginRuntime::from_cached_entries(entries, "default", Some(&policy), &mut vec![None; entries.len()])
    }

    fn make_ctx(rsc: &crate::Rsc) -> TlsRouteContext<'_> {
        TlsRouteContext {
            direct_peer_ip: "127.0.0.1".parse::<IpAddr>().unwrap(),
            remote_ip: "127.0.0.1".parse::<IpAddr>().unwrap(),
            listener_port: 443,
            sni: "test.example.com",
            tls_id: Some("abcd1234"),
            rsc,
            is_mtls: false,
        }
    }

    #[tokio::test]
    async fn test_empty_runtime_allows() {
        let rt = TlsRoutePluginRuntime::new();
        assert!(rt.is_empty());
        let rsc = crate::Rsc::new("TLSRoute", "default", "test-route");
        let ctx = make_ctx(&rsc);
        let result = rt.run(&ctx).await;
        assert!(matches!(result, StreamPluginResult::Allow));
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
    fn ip_restriction_entry() -> TlsRoutePluginEntry {
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
            plugin: TlsRouteStreamPlugin::IpRestriction(config),
        }
    }

    #[tokio::test]
    async fn block_policy_denies_connection_bypass_policy_skips() {
        let blocked = ip_restriction_entry();
        let runtime = policy_runtime(&[blocked], PolicyActionKind::Block);
        let rsc = crate::Rsc::new("TLSRoute", "default", "test-route");
        let ctx = make_ctx(&rsc);
        match runtime.run(&ctx).await {
            StreamPluginResult::Deny(reason) => assert!(reason.contains("plugin disabled by policy")),
            other => panic!("expected Deny, got {other:?}"),
        }

        let bypassed = ip_restriction_entry();
        let runtime = policy_runtime(&[bypassed], PolicyActionKind::Bypass);
        assert!(runtime.is_empty(), "bypassed plugin must not be instantiated");
    }

    #[tokio::test]
    async fn block_policy_collapses_multiple_restricted_entries() {
        let normal = ip_restriction_entry();
        let blocked = ip_restriction_entry();
        let runtime = policy_runtime(&[normal, blocked], PolicyActionKind::Block);
        let rsc = crate::Rsc::new("TLSRoute", "default", "test-route");
        let ctx = make_ctx(&rsc);
        match runtime.run(&ctx).await {
            StreamPluginResult::Deny(reason) => {
                assert!(reason.contains("plugin disabled by policy: tls-route/IpRestriction"))
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn standalone_entries_are_allowed() {
        let entry = ip_restriction_entry();
        let runtime = TlsRoutePluginRuntime::from_entries(&[entry], "default");
        assert!(!runtime.is_empty());
        assert_eq!(runtime.plugin_count(), 1);
        let rsc = crate::Rsc::new("TLSRoute", "default", "test-route");
        let ctx = make_ctx(&rsc);
        assert!(matches!(runtime.run(&ctx).await, StreamPluginResult::Allow));
    }

    struct BorrowIdentityPlugin {
        sni_ptr: usize,
        tls_id_ptr: usize,
        rsc_ptr: usize,
    }

    #[async_trait::async_trait]
    impl TlsRoutePlugin for BorrowIdentityPlugin {
        async fn on_tls_route(&self, ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
            assert_eq!(ctx.sni.as_ptr() as usize, self.sni_ptr);
            assert_eq!(ctx.tls_id.unwrap().as_ptr() as usize, self.tls_id_ptr);
            assert_eq!(std::ptr::from_ref(ctx.rsc) as usize, self.rsc_ptr);
            StreamPluginResult::Allow
        }
    }

    #[tokio::test]
    async fn async_trait_dispatch_preserves_borrowed_metadata_identity() {
        let sni = "borrowed.example.com".to_string();
        let tls_id = "tls-identity".to_string();
        let rsc = crate::Rsc::new("TLSRoute", "default", "borrowed-route");
        let runtime = TlsRoutePluginRuntime {
            plugins: vec![Arc::new(BorrowIdentityPlugin {
                sni_ptr: sni.as_ptr() as usize,
                tls_id_ptr: tls_id.as_ptr() as usize,
                rsc_ptr: std::ptr::from_ref(&rsc) as usize,
            })],
        };
        let ctx = TlsRouteContext {
            direct_peer_ip: "127.0.0.1".parse().unwrap(),
            remote_ip: "127.0.0.1".parse().unwrap(),
            listener_port: 443,
            sni: &sni,
            tls_id: Some(&tls_id),
            rsc: &rsc,
            is_mtls: true,
        };

        assert!(matches!(runtime.run(&ctx).await, StreamPluginResult::Allow));
    }
}
