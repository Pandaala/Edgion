//! Pure attachment facts for policy publication. No runtime store reads occur
//! while extracting or comparing these owned inputs.

use edgion_resources::resources::refs::plugin_attachment_fields::{HttpPluginAttachments, StreamPluginAttachment};
use std::collections::HashSet;
use std::sync::Arc;

use crate::plugins::runtime::gateway_api_filters::extension_ref::extension_ref_key;
#[cfg(test)]
use edgion_resources::constants::annotations::edgion as annotations;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::gateway::AllowedRoutes;
use edgion_resources::resources::http_route::RouteParentStatus;
use edgion_resources::resources::refs::route_fields::{GrpcRouteExtensionRefs, HttpRouteExtensionRefs};

use edgion_resources::{GRPCRoute, Gateway, HTTPRoute, HTTPRouteRule, ListenerKey, TCPRoute, TLSRoute, UDPRoute};
use kube::api::ObjectMeta;

use crate::plugins::stream::plugin_generation::StreamStages;
use crate::routes::common::{filter_accepted_parent_refs, resolve_parent_refs, resolve_stream_plugin_store_key};
use crate::runtime::matching::route::{check_allowed_routes, ResolvedParentRef};
use crate::runtime::store::listener_gateway_info::{gateway_listener_entries, ListenerEntry};

#[derive(Clone, Debug)]
pub(crate) struct GatewayListener {
    pub port: i32,
    pub key: ListenerKey,
    pub name: String,
    pub hostname: Option<String>,
    pub allowed_routes: Option<AllowedRoutes>,
    pub entry: ListenerEntry,
}

#[derive(Clone, Debug)]
pub(crate) struct GatewayInput {
    pub class_name: String,
    pub http_keys: Vec<String>,
    pub connection_key: Option<String>,
    pub listeners: Vec<GatewayListener>,
}

impl GatewayInput {
    pub(crate) fn from_gateway(gateway: &Gateway) -> Self {
        let http_keys = HttpPluginAttachments::Gateway(gateway)
            .entries()
            .into_iter()
            .flatten()
            .map(|reference| reference.into_key())
            .collect();
        let connection_key = resolve_stream_plugin_store_key(StreamPluginAttachment::Gateway(gateway));
        let listeners = gateway_listener_entries(gateway)
            .into_iter()
            .map(|(key, entry)| GatewayListener {
                port: entry.port,
                name: entry.info.listener_name.clone().expect("projected listener has a name"),
                hostname: entry.hostname.clone(),
                allowed_routes: entry.allowed_routes.clone(),
                key,
                entry,
            })
            .collect();
        Self {
            class_name: gateway.spec.gateway_class_name.clone(),
            http_keys,
            connection_key,
            listeners,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RouteKind {
    Http,
    Grpc,
    Tcp,
    Udp,
    Tls,
}

impl RouteKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Http => "HTTPRoute",
            Self::Grpc => "GRPCRoute",
            Self::Tcp => "TCPRoute",
            Self::Udp => "UDPRoute",
            Self::Tls => "TLSRoute",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RouteInput {
    pub namespace: String,
    pub kind: RouteKind,
    pub parents: Vec<ResolvedParentRef>,
    pub listeners: HashSet<ListenerKey>,
    pub http_keys: HashSet<String>,
    pub stream_key: Option<String>,
    pub stages: StreamStages,
    /// Preserve the callback's original Arc for the admission source token.
    pub http_route: Option<Arc<HTTPRoute>>,
}

impl RouteInput {
    fn base(
        metadata: &ObjectMeta,
        kind: RouteKind,
        parents: Option<&[ParentReference]>,
        status: Option<&[RouteParentStatus]>,
        expected_controller: Option<&str>,
        listeners: Option<&[ListenerKey]>,
    ) -> Self {
        let namespace = metadata.namespace.as_deref().unwrap_or("default");
        let accepted =
            filter_accepted_parent_refs(parents, status, Some(namespace), expected_controller).unwrap_or_default();
        Self {
            namespace: namespace.to_owned(),
            kind,
            parents: resolve_parent_refs(&accepted, namespace),
            listeners: listeners.iter().flat_map(|keys| keys.iter().cloned()).collect(),
            http_keys: HashSet::new(),
            stream_key: None,
            stages: StreamStages::default(),
            http_route: None,
        }
    }

    pub(crate) fn from_http(route: Arc<HTTPRoute>) -> Self {
        let mut input = Self::base(
            &route.metadata,
            RouteKind::Http,
            route.spec.parent_refs.as_deref(),
            route
                .spec
                .current_status
                .as_ref()
                .map(|status| status.parents.as_slice()),
            route.spec.resolved_status_controller.as_deref(),
            route.spec.resolved_listeners.as_deref(),
        );
        if let Ok(references) = HttpRouteExtensionRefs::from_route(&route) {
            let rules: HashSet<_> = effective_http_rules(&route).map(|(index, _)| index).collect();
            input.http_keys.extend(
                references
                    .effective()
                    .filter(|field| rules.contains(&field.slot().rule()))
                    .filter_map(extension_ref_key),
            );
        }
        input.http_route = Some(route);
        input
    }

    pub(crate) fn from_grpc(route: &GRPCRoute) -> Self {
        let mut input = Self::base(
            &route.metadata,
            RouteKind::Grpc,
            route.spec.parent_refs.as_deref(),
            route
                .spec
                .current_status
                .as_ref()
                .map(|status| status.parents.as_slice()),
            route.spec.resolved_status_controller.as_deref(),
            route.spec.resolved_listeners.as_deref(),
        );
        let invalid = if route.spec.resolved_rules.is_some() {
            None
        } else {
            route.spec.invalid_rule_indices.as_ref()
        };
        if let Ok(references) = GrpcRouteExtensionRefs::from_route(route) {
            input.http_keys.extend(
                references
                    .effective()
                    .filter(|field| !invalid.is_some_and(|indices| indices.contains(&field.slot().rule())))
                    .filter_map(extension_ref_key),
            );
        }
        input
    }

    pub(crate) fn from_tcp(route: &TCPRoute) -> Self {
        let mut input = Self::base(
            &route.metadata,
            RouteKind::Tcp,
            route.spec.parent_refs.as_deref(),
            route
                .spec
                .current_status
                .as_ref()
                .map(|status| status.parents.as_slice()),
            route.spec.resolved_status_controller.as_deref(),
            route.spec.resolved_listeners.as_deref(),
        );
        input.stream_key = resolve_stream_plugin_store_key(StreamPluginAttachment::Tcp(route));
        input.stages.connection = true;
        input
    }

    pub(crate) fn from_udp(route: &UDPRoute) -> Self {
        let mut input = Self::base(
            &route.metadata,
            RouteKind::Udp,
            route.spec.parent_refs.as_deref(),
            route
                .spec
                .current_status
                .as_ref()
                .map(|status| status.parents.as_slice()),
            route.spec.resolved_status_controller.as_deref(),
            route.spec.resolved_listeners.as_deref(),
        );
        input.stream_key = resolve_stream_plugin_store_key(StreamPluginAttachment::Udp(route));
        input.stages.connection = true;
        input
    }

    pub(crate) fn from_tls(route: &TLSRoute) -> Self {
        let listeners: Vec<_> = route
            .spec
            .resolved_listeners
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|attachment| attachment.listener_key.clone())
            .collect();
        let mut input = Self::base(
            &route.metadata,
            RouteKind::Tls,
            route.spec.parent_refs.as_deref(),
            route
                .spec
                .current_status
                .as_ref()
                .map(|status| status.parents.as_slice()),
            route.spec.resolved_status_controller.as_deref(),
            Some(&listeners),
        );
        input.stream_key = resolve_stream_plugin_store_key(StreamPluginAttachment::Tls(route));
        input.stages.tls_route = true;
        input
    }

    /// Demand follows accepted parents and serving listener intersections.
    /// Request hostname matching remains the canonical route matcher's job.
    pub(crate) fn attaches_to(&self, gateway_key: &str, gateway: &GatewayInput) -> bool {
        self.parents.iter().any(|parent| {
            parent.gateway_key == gateway_key
                && gateway.listeners.iter().any(|listener| {
                    self.listeners.contains(&listener.key)
                        && parent.section_name.as_ref().is_none_or(|name| name == &listener.name)
                        && check_allowed_routes(
                            &listener.allowed_routes,
                            &self.namespace,
                            self.kind.as_str(),
                            &parent.parent_ns,
                        )
                })
        })
    }
}

/// The same effective rule sequence and original indices as HTTP route table
/// construction, including native CORS rejection after delegated-rule selection.
pub(crate) fn effective_http_rules(route: &HTTPRoute) -> impl Iterator<Item = (usize, &HTTPRouteRule)> {
    let invalid = if route.spec.resolved_rules.is_some() {
        None
    } else {
        route.spec.invalid_rule_indices.as_ref()
    };
    route
        .effective_rules()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(move |(index, rule)| {
            !invalid.is_some_and(|indices| indices.contains(index)) && rule.unsupported_cors_reason().is_none()
        })
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/publication/inputs_test.rs"]
mod tests;
