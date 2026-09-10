//! Bridge between Edgion's StreamPlugin system and Pingora's ConnectionFilter trait.
//!
//! This module provides `StreamPluginConnectionFilter`, which implements Pingora's
//! `ConnectionFilter` trait by delegating to the existing `StreamPluginRuntime`.
//!
//! This allows reusing `EdgionStreamPlugins` resources (e.g., IP restriction) as
//! early TCP-level connection filters after accept, but before inbound PROXY
//! parsing, TLS handshake, or HTTP parsing. The listener's fatal PROXY-policy
//! guard runs before this bridge and remains authoritative.
//!
//! ## Usage
//!
//! Configured via Gateway annotation:
//! ```yaml
//! metadata:
//!   annotations:
//!     edgion.io/edgion-stream-plugins: "policy-name"
//! ```
//! The bare resource name is resolved in the Gateway's own namespace; cross-
//! namespace attachment is not supported.

use async_trait::async_trait;
use pingora_core::listeners::ConnectionFilter;
use std::net::SocketAddr;
use std::sync::Arc;

use super::stream_plugin_store::StreamPluginStore;
use super::stream_plugin_trait::{StreamContext, StreamLayer, StreamPluginResult};

/// Bridge that adapts Edgion's StreamPlugin system to Pingora's ConnectionFilter.
///
/// Holds a reference to the global `StreamPluginStore` and a store key
/// (`namespace/name`). Each invocation that reaches this bridge with an inet
/// peer loads the latest ArcSwap snapshot and clones the selected
/// `CompiledStreamPlugins` `Arc`, pinning its Stage 1 runtime for that admission
/// decision.
///
/// Pingora supplies the physical transport peer. Therefore `addr.ip()` is
/// `DirectPeerIp`, never `RemoteIp`; with inbound PROXY protocol enabled it is
/// normally the load balancer's address because PROXY parsing happens later.
pub struct StreamPluginConnectionFilter {
    /// Global stream plugin store (hot-reloadable via ArcSwap)
    store: Arc<StreamPluginStore>,
    /// Store key in "namespace/name" format, referencing an EdgionStreamPlugins resource
    store_key: String,
    /// The listening port this filter serves (passed to StreamContext for plugin logic)
    listener_port: u16,
}

impl std::fmt::Debug for StreamPluginConnectionFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamPluginConnectionFilter")
            .field("store_key", &self.store_key)
            .field("listener_port", &self.listener_port)
            .finish()
    }
}

impl StreamPluginConnectionFilter {
    /// Create a connection filter bridge for the given store key and listener port.
    pub fn new(store: Arc<StreamPluginStore>, store_key: String, listener_port: u16) -> Self {
        Self {
            store,
            store_key,
            listener_port,
        }
    }
}

#[async_trait]
impl ConnectionFilter for StreamPluginConnectionFilter {
    async fn should_accept(&self, addr: Option<&SocketAddr>) -> bool {
        let Some(addr) = addr else {
            // Pingora has no inet peer. Ordinary Edgion TCP endpoints normally
            // provide Some(addr), so this narrow transport case passes through.
            return true;
        };

        // Look up the referenced EdgionStreamPlugins resource from the store.
        // Fail-close: a missing reference means the configured policy (e.g. an
        // IP allowlist) cannot be enforced, so we reject rather than silently
        // admit traffic that an operator expected to be filtered.
        let Some(stream_plugins) = self.store.get(&self.store_key) else {
            return false;
        };

        // Use the pre-computed runtime (built at config sync time by the store)
        let runtime = &stream_plugins.stream_plugin_runtime;
        if runtime.is_empty() {
            return true; // No active plugins, allow
        }

        let ctx = StreamContext::new(addr.ip(), self.listener_port, StreamLayer::Stage1);
        // ConnectionFilter exposes only a boolean and has no request/TcpContext
        // or canonical access-log metadata channel. Fold Deny's reason into
        // false; per-connection tracing is forbidden on this data-plane path.
        match runtime.run(&ctx).await {
            StreamPluginResult::Allow => true,
            StreamPluginResult::Deny(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::stream::compile_stream_plugins;
    use edgion_common::conf_sync::traits::ConfHandler;
    use edgion_resources::resources::edgion_stream_plugins::EdgionStreamPlugins;

    #[tokio::test]
    async fn listener_reload_preserves_port_budget_and_missing_resource_denies() {
        let store = Arc::new(StreamPluginStore::new());
        let key = "default/local-gate-bridge-reload";
        let resource = |rate| {
            let yaml = format!(
                r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata: {{name: local-gate-bridge-reload, namespace: default}}
spec:
  plugins:
    - type: ConnectionRateLimit
      config:
        perListener: {{rate: {rate}, interval: 1h}}
"#
            );
            Arc::new(serde_yaml::from_str::<EdgionStreamPlugins>(&yaml).unwrap())
        };
        let filter = StreamPluginConnectionFilter::new(Arc::clone(&store), key.to_owned(), 18092);
        let other_port = StreamPluginConnectionFilter::new(Arc::clone(&store), key.to_owned(), 18093);
        let addr: SocketAddr = "10.0.0.1:54321".parse().unwrap();
        store.full_set(&std::collections::HashMap::from([(key.to_owned(), resource(1))]));
        assert!(filter.should_accept(Some(&addr)).await);
        assert!(!filter.should_accept(Some(&addr)).await);
        assert!(other_port.should_accept(Some(&addr)).await);

        store.full_set(&std::collections::HashMap::from([(key.to_owned(), resource(2))]));
        assert!(filter.should_accept(Some(&addr)).await);
        assert!(!filter.should_accept(Some(&addr)).await);
        assert!(other_port.should_accept(Some(&addr)).await);
        assert!(!other_port.should_accept(Some(&addr)).await);

        store.full_set(&std::collections::HashMap::new());
        assert!(!filter.should_accept(Some(&addr)).await);
        store.full_set(&std::collections::HashMap::from([(key.to_owned(), resource(2))]));
        assert!(!filter.should_accept(Some(&addr)).await);
    }

    fn empty_esp_yaml() -> EdgionStreamPlugins {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: empty
  namespace: default
spec: {}
"#;
        serde_yaml::from_str(yaml).expect("YAML must parse")
    }

    #[tokio::test]
    async fn fail_close_when_store_missing_key() {
        let store = Arc::new(StreamPluginStore::new());
        let filter = StreamPluginConnectionFilter::new(store, "default/missing".to_string(), 8080);
        let addr: SocketAddr = "10.0.0.1:54321".parse().unwrap();

        assert!(
            !filter.should_accept(Some(&addr)).await,
            "missing reference must reject (fail-close), not admit"
        );
    }

    #[tokio::test]
    async fn admit_when_store_has_empty_runtime() {
        let store = Arc::new(StreamPluginStore::new());
        let mut map = std::collections::HashMap::new();
        map.insert("default/empty".to_string(), compile_stream_plugins(&empty_esp_yaml()));
        store.replace_all(map);

        let filter = StreamPluginConnectionFilter::new(store, "default/empty".to_string(), 8080);
        let addr: SocketAddr = "10.0.0.1:54321".parse().unwrap();

        assert!(
            filter.should_accept(Some(&addr)).await,
            "operator-created ESP with no plugins is intentional pass-through"
        );
    }

    #[tokio::test]
    async fn admit_when_address_is_none() {
        let store = Arc::new(StreamPluginStore::new());
        let filter = StreamPluginConnectionFilter::new(store, "default/missing".to_string(), 8080);
        assert!(filter.should_accept(None).await);
    }
}
