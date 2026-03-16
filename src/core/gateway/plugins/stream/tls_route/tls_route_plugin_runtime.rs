//! Runtime for executing TLS route plugins in order (first-deny-wins).

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::core::gateway::plugins::stream::stream_plugin_trait::StreamPluginResult;
use crate::types::resources::edgion_stream_plugins::{TlsRoutePluginEntry, TlsRouteStreamPlugin};
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

    /// Build runtime from TLS route plugin entries (called during preparse).
    pub fn from_entries(entries: &[TlsRoutePluginEntry]) -> Self {
        let mut plugins: Vec<Arc<dyn TlsRoutePlugin>> = Vec::new();

        for entry in entries {
            if !entry.is_enabled() {
                tracing::debug!(plugin_type = entry.type_name(), "Skipping disabled TLS route plugin");
                continue;
            }

            let plugin: Option<Arc<dyn TlsRoutePlugin>> = match &entry.plugin {
                TlsRouteStreamPlugin::IpRestriction(config) => Some(Arc::new(TlsRouteIpRestriction::new(config))),
            };

            if let Some(p) = plugin {
                tracing::debug!(plugin_name = p.name(), "Added TLS route plugin to runtime");
                plugins.push(p);
            }
        }

        Self { plugins }
    }

    /// Execute all plugins in order. Returns Deny on first deny, otherwise Allow.
    pub async fn run(&self, ctx: &TlsRouteContext) -> StreamPluginResult {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn make_ctx() -> TlsRouteContext {
        TlsRouteContext {
            client_ip: "127.0.0.1".parse::<IpAddr>().unwrap(),
            listener_port: 443,
            sni: "test.example.com".to_string(),
            tls_id: Some("abcd1234".to_string()),
            matched_route_ns: "default".to_string(),
            matched_route_name: "test-route".to_string(),
            is_mtls: false,
        }
    }

    #[tokio::test]
    async fn test_empty_runtime_allows() {
        let rt = TlsRoutePluginRuntime::new();
        assert!(rt.is_empty());
        let result = rt.run(&make_ctx()).await;
        assert!(matches!(result, StreamPluginResult::Allow));
    }

    #[tokio::test]
    async fn test_from_empty_entries() {
        let rt = TlsRoutePluginRuntime::from_entries(&[]);
        assert!(rt.is_empty());
        assert_eq!(rt.plugin_count(), 0);
    }
}
