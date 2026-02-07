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
/// (`"namespace/name"`). On every incoming TCP connection, it reads the latest
/// `EdgionStreamPlugins` resource from the store (ArcSwap), ensuring hot-reload.
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

        // Look up the referenced EdgionStreamPlugins resource from the store
        let Some(stream_plugins) = self.store.get(&self.store_key) else {
            // Resource not found (not yet synced or deleted), allow by default
            tracing::debug!(
                store_key = %self.store_key,
                "ConnectionFilter: EdgionStreamPlugins resource not found, allowing connection"
            );
            return true;
        };

        // Use the pre-computed runtime (initialized at config sync time)
        let runtime = &stream_plugins.spec.stream_plugin_runtime;
        if runtime.is_empty() {
            return true; // No active plugins, allow
        }

        let ctx = StreamContext::new(addr.ip(), self.listener_port);
        match runtime.run(&ctx).await {
            StreamPluginResult::Allow => true,
            StreamPluginResult::Deny(reason) => {
                tracing::info!(
                    client_ip = %addr.ip(),
                    listener_port = self.listener_port,
                    store_key = %self.store_key,
                    reason = %reason,
                    "ConnectionFilter: connection denied at TCP level"
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::edgion_stream_plugins::{DefaultAction, IpRestrictionConfig};
    use crate::types::resources::edgion_stream_plugins::{EdgionStreamPlugin, StreamPluginEntry};
    use crate::types::resources::EdgionStreamPlugins;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};

    /// Helper: create an EdgionStreamPlugins with deny-all IP restriction
    fn make_deny_all_plugins() -> EdgionStreamPlugins {
        let config = IpRestrictionConfig {
            default_action: DefaultAction::Deny,
            message: Some("denied by test".to_string()),
            ..Default::default()
        };
        let mut resource = EdgionStreamPlugins {
            metadata: ObjectMeta {
                name: Some("deny-all".to_string()),
                namespace: Some("test".to_string()),
                ..Default::default()
            },
            spec: crate::types::resources::edgion_stream_plugins::EdgionStreamPluginsSpec {
                plugins: Some(vec![StreamPluginEntry {
                    enable: true,
                    plugin: EdgionStreamPlugin::IpRestriction(config),
                }]),
                stream_plugin_runtime: Default::default(),
            },
            status: None,
        };
        resource.init_stream_plugin_runtime();
        resource
    }

    /// Helper: create an EdgionStreamPlugins with allow-all IP restriction
    fn make_allow_all_plugins() -> EdgionStreamPlugins {
        let config = IpRestrictionConfig {
            default_action: DefaultAction::Allow,
            ..Default::default()
        };
        let mut resource = EdgionStreamPlugins {
            metadata: ObjectMeta {
                name: Some("allow-all".to_string()),
                namespace: Some("test".to_string()),
                ..Default::default()
            },
            spec: crate::types::resources::edgion_stream_plugins::EdgionStreamPluginsSpec {
                plugins: Some(vec![StreamPluginEntry {
                    enable: true,
                    plugin: EdgionStreamPlugin::IpRestriction(config),
                }]),
                stream_plugin_runtime: Default::default(),
            },
            status: None,
        };
        resource.init_stream_plugin_runtime();
        resource
    }

    fn make_store_with(key: &str, plugins: EdgionStreamPlugins) -> Arc<StreamPluginStore> {
        let store = Arc::new(StreamPluginStore::new());
        let mut map = HashMap::new();
        map.insert(key.to_string(), Arc::new(plugins));
        store.replace_all(map);
        store
    }

    fn addr(ip: [u8; 4], port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])), port)
    }

    #[tokio::test]
    async fn test_no_addr_allows() {
        let store = Arc::new(StreamPluginStore::new());
        let filter = StreamPluginConnectionFilter::new(store, "test/deny-all".to_string(), 8080);
        assert!(filter.should_accept(None).await);
    }

    #[tokio::test]
    async fn test_resource_not_found_allows() {
        let store = Arc::new(StreamPluginStore::new()); // empty store
        let filter = StreamPluginConnectionFilter::new(store, "test/missing".to_string(), 8080);
        let a = addr([192, 168, 1, 1], 12345);
        assert!(filter.should_accept(Some(&a)).await);
    }

    #[tokio::test]
    async fn test_deny_all_rejects() {
        let store = make_store_with("test/deny-all", make_deny_all_plugins());
        let filter = StreamPluginConnectionFilter::new(store, "test/deny-all".to_string(), 8080);
        let a = addr([10, 0, 0, 1], 12345);
        assert!(!filter.should_accept(Some(&a)).await);
    }

    #[tokio::test]
    async fn test_allow_all_accepts() {
        let store = make_store_with("test/allow-all", make_allow_all_plugins());
        let filter = StreamPluginConnectionFilter::new(store, "test/allow-all".to_string(), 8080);
        let a = addr([10, 0, 0, 1], 12345);
        assert!(filter.should_accept(Some(&a)).await);
    }

    #[tokio::test]
    async fn test_hot_reload() {
        // Start with allow-all
        let store = make_store_with("test/plugins", make_allow_all_plugins());
        let filter = StreamPluginConnectionFilter::new(store.clone(), "test/plugins".to_string(), 8080);
        let a = addr([10, 0, 0, 1], 12345);

        assert!(filter.should_accept(Some(&a)).await, "should allow initially");

        // Hot-reload: swap to deny-all
        let mut map = HashMap::new();
        map.insert("test/plugins".to_string(), Arc::new(make_deny_all_plugins()));
        store.replace_all(map);

        assert!(
            !filter.should_accept(Some(&a)).await,
            "should deny after hot-reload"
        );
    }

    #[tokio::test]
    async fn test_empty_plugins_allows() {
        // EdgionStreamPlugins with no plugins
        let mut resource = EdgionStreamPlugins {
            metadata: ObjectMeta {
                name: Some("empty".to_string()),
                namespace: Some("test".to_string()),
                ..Default::default()
            },
            spec: crate::types::resources::edgion_stream_plugins::EdgionStreamPluginsSpec {
                plugins: Some(vec![]),
                stream_plugin_runtime: Default::default(),
            },
            status: None,
        };
        resource.init_stream_plugin_runtime();

        let store = make_store_with("test/empty", resource);
        let filter = StreamPluginConnectionFilter::new(store, "test/empty".to_string(), 8080);
        let a = addr([10, 0, 0, 1], 12345);
        assert!(filter.should_accept(Some(&a)).await);
    }

    #[tokio::test]
    async fn test_disabled_plugin_allows() {
        // EdgionStreamPlugins with disabled deny plugin
        let config = IpRestrictionConfig {
            default_action: DefaultAction::Deny,
            ..Default::default()
        };
        let mut resource = EdgionStreamPlugins {
            metadata: ObjectMeta {
                name: Some("disabled".to_string()),
                namespace: Some("test".to_string()),
                ..Default::default()
            },
            spec: crate::types::resources::edgion_stream_plugins::EdgionStreamPluginsSpec {
                plugins: Some(vec![StreamPluginEntry {
                    enable: false, // disabled!
                    plugin: EdgionStreamPlugin::IpRestriction(config),
                }]),
                stream_plugin_runtime: Default::default(),
            },
            status: None,
        };
        resource.init_stream_plugin_runtime();

        let store = make_store_with("test/disabled", resource);
        let filter = StreamPluginConnectionFilter::new(store, "test/disabled".to_string(), 8080);
        let a = addr([10, 0, 0, 1], 12345);
        assert!(filter.should_accept(Some(&a)).await, "disabled plugin should not block");
    }
}
