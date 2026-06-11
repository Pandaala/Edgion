//! Bridge between Edgion's StreamPlugin system and Pingora's ConnectionFilter trait.
//!
//! This module provides `StreamPluginConnectionFilter`, which implements Pingora's
//! `ConnectionFilter` trait by delegating to the existing `StreamPluginRuntime`.
//!
//! This allows reusing `EdgionStreamPlugins` resources (e.g., IP restriction) as
//! early TCP-level connection filters — before TLS handshake or HTTP parsing.
//!
//! ## Usage
//!
//! Configured via Gateway annotation:
//! ```yaml
//! metadata:
//!   annotations:
//!     edgion.io/edgion-stream-plugins: "namespace/name"
//! ```

use async_trait::async_trait;
use pingora_core::listeners::ConnectionFilter;
use std::net::SocketAddr;
use std::sync::Arc;

use super::stream_plugin_store::StreamPluginStore;
use super::stream_plugin_trait::{StreamContext, StreamPluginResult};

/// Bridge that adapts Edgion's StreamPlugin system to Pingora's ConnectionFilter.
///
/// Holds a reference to the global `StreamPluginStore` and a store key
/// (`namespace/name`). On every incoming TCP connection, it reads the latest
/// `EdgionStreamPlugins` from the store (ArcSwap), ensuring hot-reload support.
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
    /// Create a new connection filter bridge.
    ///
    /// # Parameters
    /// - `store`: Reference to the global StreamPluginStore
    /// - `store_key`: Resource key in "namespace/name" format
    /// - `listener_port`: The listening port for StreamContext
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
            return true; // No address info, allow by default
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

        let ctx = StreamContext::new(addr.ip(), self.listener_port);
        match runtime.run(&ctx).await {
            StreamPluginResult::Allow => true,
            StreamPluginResult::Deny(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::stream::compile_stream_plugins;
    use edgion_resources::resources::edgion_stream_plugins::EdgionStreamPlugins;

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
