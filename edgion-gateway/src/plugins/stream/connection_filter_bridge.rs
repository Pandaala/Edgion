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
    let ctx = StreamContext::new(addr.ip(), port, StreamLayer::Stage1);
    matches!(runtime.run(&ctx).await, StreamPluginResult::Allow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::stream::plugin_generation::{StreamPluginGeneration, StreamStages};
    use edgion_resources::resources::plugin_policy::PluginPolicyConfig;

    #[tokio::test]
    async fn unavailable_policy_denies_even_without_peer_or_attachment() {
        assert!(!accept_connection_view(ConnectionView::Unavailable, None, 18092).await);
        let peer = "10.0.0.1:54321".parse().unwrap();
        assert!(!accept_connection_view(ConnectionView::Unavailable, Some(&peer), 18092).await);
        assert!(accept_connection_view(ConnectionView::Empty, Some(&peer), 18092).await);
    }

    #[tokio::test]
    async fn pinned_connection_views_preserve_port_budgets_and_release() {
        let source = Arc::new(serde_yaml::from_str(
            "apiVersion: edgion.io/v1\nkind: EdgionStreamPlugins\nmetadata: {name: bridge-generation, namespace: default}\nspec:\n  plugins:\n    - type: ConnectionRateLimit\n      config:\n        perListener: {rate: 1, interval: 1h}\n"
        ).unwrap());
        let mut generation = StreamPluginGeneration::new(source);
        let policy = PluginPolicyConfig::default().compile().unwrap();
        let stages = StreamStages {
            connection: true,
            tls_route: false,
        };
        let view = generation.compile(&policy, stages);
        let weak = Arc::downgrade(&view);
        let peer = "10.0.0.1:54321".parse().unwrap();
        assert!(accept_connection_view(ConnectionView::Ready(Arc::clone(&view)), Some(&peer), 18092).await);
        assert!(!accept_connection_view(ConnectionView::Ready(Arc::clone(&view)), Some(&peer), 18092).await);
        assert!(accept_connection_view(ConnectionView::Ready(Arc::clone(&view)), Some(&peer), 18093).await);
        assert!(!accept_connection_view(ConnectionView::Unavailable, Some(&peer), 18092).await);
        drop(view);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn logical_owner_and_policy_are_selected_from_one_publication() {
        use crate::plugins::publication::{
            inputs::{GatewayInput, GatewayListener},
            PluginPublisher,
        };
        use std::collections::{HashMap, HashSet};
        let publisher = PluginPublisher::new();
        let class = Arc::new(serde_yaml::from_str(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: GatewayClass\nmetadata: {name: ready}\nspec: {controllerName: edgion.io/gateway-controller}"
        ).unwrap());
        publisher.replace_gateway_classes(HashMap::from([("ready".into(), class)]));
        let listener = ListenerKey::new("0.0.0.0:18443".parse().unwrap(), "TLS");
        let input = |class: &str| GatewayInput {
            class_name: class.into(),
            http_keys: vec![],
            connection_key: None,
            listeners: vec![GatewayListener {
                key: listener.clone(),
                name: "tls".into(),
                hostname: Some("owner.example.com".into()),
                allowed_routes: None,
            }],
        };
        publisher.replace_gateways(HashMap::from([
            ("default/a".into(), input("missing")),
            ("default/b".into(), input("ready")),
        ]));
        let parent = |key: &str| ResolvedParentRef {
            gateway_key: key.into(),
            parent_ns: "default".into(),
            section_name: Some("tls".into()),
        };
        let old = publisher.snapshot();
        let b = [parent("default/b")];
        let scope = resolve_route_scope(&old, &listener, &b, "owner.example.com", "default", "TLSRoute").unwrap();
        assert!(resolve_route_scope(
            &old,
            &listener,
            &[parent("default/a")],
            "owner.example.com",
            "default",
            "TLSRoute"
        )
        .is_none());
        assert!(resolve_route_scope(&old, &listener, &b, "other.example.com", "default", "TLSRoute").is_none());
        assert!(resolve_route_scope(&old, &listener, &b, "owner.example.com", "other", "TLSRoute").is_none());
        let mut wrong_section = parent("default/b");
        wrong_section.section_name = Some("other".into());
        assert!(resolve_route_scope(
            &old,
            &listener,
            &[wrong_section],
            "owner.example.com",
            "default",
            "TLSRoute"
        )
        .is_none());
        // A class rebind invalidates new admissions; the old admission remains pinned.
        publisher.update_gateways(HashMap::from([("default/b".into(), input("missing"))]), &HashSet::new());
        assert!(resolve_route_scope(
            &publisher.snapshot(),
            &listener,
            &b,
            "owner.example.com",
            "default",
            "TLSRoute"
        )
        .is_none());
        assert!(Arc::ptr_eq(
            &scope,
            &resolve_route_scope(&old, &listener, &b, "owner.example.com", "default", "TLSRoute").unwrap()
        ));
    }
    #[tokio::test]
    async fn listener_annotation_can_be_added_and_removed_after_startup() {
        use crate::plugins::publication::{inputs::GatewayInput, PluginPublisher};
        use std::collections::{HashMap, HashSet};
        let publisher = PluginPublisher::new();
        let gateway: edgion_resources::Gateway = serde_yaml::from_str(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: Gateway\nmetadata: {name: dynamic, namespace: default}\nspec:\n  gatewayClassName: dynamic\n  listeners: [{name: tcp, port: 18094, protocol: TCP}]"
        ).unwrap();
        let class = Arc::new(serde_yaml::from_str(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: GatewayClass\nmetadata: {name: dynamic}\nspec: {controllerName: edgion.io/gateway-controller}"
        ).unwrap());
        publisher.replace_gateway_classes(HashMap::from([("dynamic".into(), class)]));
        let mut input = GatewayInput::from_gateway(&gateway);
        let key = input.listeners[0].key.clone();
        let peer = "10.0.0.1:54321".parse().unwrap();
        publisher.replace_gateways(HashMap::from([("default/dynamic".into(), input.clone())]));
        assert!(accept_connection_view(publisher.snapshot().connection(&key), Some(&peer), 18094).await);
        input.connection_key = Some("default/dynamic-policy".into());
        publisher.update_gateways(
            HashMap::from([("default/dynamic".into(), input.clone())]),
            &HashSet::new(),
        );
        assert!(!accept_connection_view(publisher.snapshot().connection(&key), Some(&peer), 18094).await);
        let source = Arc::new(serde_yaml::from_str(
            "apiVersion: edgion.io/v1\nkind: EdgionStreamPlugins\nmetadata: {name: dynamic-policy, namespace: default}\nspec:\n  plugins:\n    - type: IpRestriction\n      config:\n        deny: [{name: blocked, cidrs: [10.0.0.0/8]}]"
        ).unwrap());
        publisher.replace_stream_plugins(HashMap::from([("default/dynamic-policy".into(), source)]));
        assert!(!accept_connection_view(publisher.snapshot().connection(&key), Some(&peer), 18094).await);
        input.connection_key = None;
        publisher.update_gateways(HashMap::from([("default/dynamic".into(), input)]), &HashSet::new());
        assert!(accept_connection_view(publisher.snapshot().connection(&key), Some(&peer), 18094).await);
        assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
    }
}
