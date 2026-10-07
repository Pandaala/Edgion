//! Pure attachment facts for policy publication. No runtime store reads occur
//! while extracting or comparing these owned inputs.

use std::collections::HashSet;
use std::sync::Arc;

use edgion_resources::constants::annotations::edgion as annotations;
use edgion_resources::listener_key::resolve_listener_addr;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::gateway::AllowedRoutes;
use edgion_resources::resources::grpc_route::GRPCRouteFilterType;
use edgion_resources::resources::http_route::{HTTPRouteFilterType, LocalObjectReference, RouteParentStatus};
use edgion_resources::resources::refs::extension_ref::is_supported_edgion_plugins_ref;
use edgion_resources::{GRPCRoute, Gateway, HTTPRoute, HTTPRouteRule, ListenerKey, TCPRoute, TLSRoute, UDPRoute};
use kube::api::ObjectMeta;

use crate::plugins::stream::plugin_generation::StreamStages;
use crate::routes::common::{
    filter_accepted_parent_refs, resolve_gateway_stream_plugin_store_key, resolve_parent_refs,
    resolve_stream_plugin_store_key,
};
use crate::runtime::matching::route::{check_allowed_routes, ResolvedParentRef};
use crate::runtime::store::listener_gateway_info::{is_listener_conflicted, is_listener_not_programmed};

#[derive(Clone, Debug)]
pub(crate) struct GatewayListener {
    pub port: i32,
    pub key: ListenerKey,
    pub name: String,
    pub hostname: Option<String>,
    pub allowed_routes: Option<AllowedRoutes>,
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
        let namespace = gateway.metadata.namespace.as_deref().unwrap_or("default");
        let annotations = gateway.metadata.annotations.as_ref();
        let http_keys = annotations
            .and_then(|map| map.get(annotations::EDGION_PLUGINS))
            .map(|value| annotations::parse_plugin_ref_list(value, namespace))
            .unwrap_or_default();
        let connection_key = annotations
            .and_then(|map| map.get(annotations::EDGION_STREAM_PLUGINS))
            .and_then(|value| {
                // A qualified Gateway attachment is invalid. An impossible
                // store key retains the existing connection fail-close signal.
                resolve_gateway_stream_plugin_store_key(value, namespace).unwrap_or_else(|()| Some(String::new()))
            });
        let listeners = gateway
            .spec
            .listeners
            .iter()
            .flatten()
            .filter(|listener| {
                !is_listener_conflicted(gateway, &listener.name) && !is_listener_not_programmed(gateway, &listener.name)
            })
            .map(|listener| GatewayListener {
                port: listener.port,
                key: ListenerKey::new(resolve_listener_addr(gateway, listener.port), &listener.protocol),
                name: listener.name.clone(),
                hostname: listener.hostname.clone(),
                allowed_routes: listener.allowed_routes.clone(),
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
        for (_, rule) in effective_http_rules(&route) {
            for filter in rule.filters.iter().flatten().chain(
                rule.backend_refs
                    .iter()
                    .flatten()
                    .flat_map(|backend| backend.filters.iter().flatten()),
            ) {
                if filter.filter_type == HTTPRouteFilterType::ExtensionRef {
                    add_extension_ref(&mut input.http_keys, filter.extension_ref.as_ref(), &input.namespace);
                }
            }
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
        for (index, rule) in route
            .effective_rules()
            .iter()
            .flat_map(|rules| rules.iter())
            .enumerate()
        {
            if invalid.is_some_and(|indices| indices.contains(&index)) {
                continue;
            }
            for filter in rule.filters.iter().flatten().chain(
                rule.backend_refs
                    .iter()
                    .flatten()
                    .flat_map(|backend| backend.filters.iter().flatten()),
            ) {
                if filter.filter_type == GRPCRouteFilterType::ExtensionRef {
                    add_extension_ref(&mut input.http_keys, filter.extension_ref.as_ref(), &input.namespace);
                }
            }
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
        input.stream_key = resolve_stream_plugin_store_key(&route.metadata);
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
        input.stream_key = resolve_stream_plugin_store_key(&route.metadata);
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
        input.stream_key = resolve_stream_plugin_store_key(&route.metadata);
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

fn add_extension_ref(keys: &mut HashSet<String>, reference: Option<&LocalObjectReference>, namespace: &str) {
    if let Some(reference) = reference {
        if is_supported_edgion_plugins_ref(reference) {
            keys.insert(format!(
                "{}/{}",
                reference.resolved_namespace.as_deref().unwrap_or(namespace),
                reference.name
            ));
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/publication/inputs_test.rs"]
mod tests;
