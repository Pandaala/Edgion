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
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use super::stream_plugin_trait::{StreamContext, StreamLayer, StreamPluginResult};
use crate::plugins::publication::{global_publisher, ConnectionView, PluginPublication, PluginScope};
use crate::runtime::matching::route::{check_gateway_listener_match, ResolvedParentRef};
use edgion_resources::ListenerKey;

/// Resolve the logical route owner and policy from one immutable publication.
/// The index is used only for ownership; business Gateway metadata stays with
/// its existing protocol context.
pub(crate) fn resolve_route_scope(
    publication: &PluginPublication,
    listener: &ListenerKey,
    parents: &[ResolvedParentRef],
    hostname: &str,
    namespace: &str,
    kind: &str,
) -> Option<Arc<PluginScope>> {
    let index = publication.listener_index(listener)?;
    let owner = check_gateway_listener_match(parents, index, hostname, namespace, kind)?;
    let gateway = publication.gateway(owner.gateway_key())?;
    gateway.scope.available.then(|| Arc::clone(&gateway.scope))
}

/// Dynamic listener admission. Every call pins the latest common connection
/// view for all serving Gateways on this physical listener.
#[derive(Debug)]
pub struct StreamPluginConnectionFilter {
    listener_key: ListenerKey,
    listener_port: u16,
}

impl StreamPluginConnectionFilter {
    pub fn new(listener_key: ListenerKey, listener_port: u16) -> Self {
        Self {
            listener_key,
            listener_port,
        }
    }
}

#[async_trait]
impl ConnectionFilter for StreamPluginConnectionFilter {
    async fn should_accept(&self, addr: Option<&SocketAddr>) -> bool {
        let publication = global_publisher().snapshot();
        let view = publication.connection(&self.listener_key);
        drop(publication);
        accept_connection_view(view, addr, self.listener_port).await
    }
}

fn canonical_peer_ip(addr: &SocketAddr) -> IpAddr {
    addr.ip().to_canonical()
}

async fn accept_connection_view(view: ConnectionView, addr: Option<&SocketAddr>, port: u16) -> bool {
    let resource = match view {
        ConnectionView::Empty => return true,
        ConnectionView::Unavailable => return false,
        ConnectionView::Ready(resource) => resource,
    };
    let runtime = &resource.stream_plugin_runtime;
    if runtime.is_empty() {
        return true;
    }
    let Some(addr) = addr else {
        // Preserve the narrow non-inet transport behavior after the policy
        // availability gate. Ordinary TCP endpoints provide an inet peer.
        return true;
    };
    let ctx = StreamContext::new(canonical_peer_ip(addr), port, StreamLayer::Stage1);
    matches!(runtime.run(&ctx).await, StreamPluginResult::Allow)
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/stream/connection_filter_bridge_test.rs"]
mod tests;
