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

        // Look up the referenced EdgionStreamPlugins resource from the store
        let Some(stream_plugins) = self.store.get(&self.store_key) else {
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
            StreamPluginResult::Deny(_) => false,
        }
    }
}
