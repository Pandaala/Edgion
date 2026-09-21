//! Pure attachment facts for policy publication. No runtime store reads occur
//! while extracting or comparing these owned inputs.

use std::collections::HashSet;
use std::sync::Arc;

use edgion_resources::constants::annotations::edgion as annotations;
use edgion_resources::listener_key::resolve_listener_addr;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::extension_ref::is_supported_edgion_plugins_ref;
use edgion_resources::resources::gateway::AllowedRoutes;
use edgion_resources::resources::grpc_route::GRPCRouteFilterType;
use edgion_resources::resources::http_route::{HTTPRouteFilterType, LocalObjectReference, RouteParentStatus};
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
                key: ListenerKey::new(
                    resolve_listener_addr(gateway.spec.addresses.as_ref(), listener.port),
                    &listener.protocol,
                ),
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
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn gateway() -> Gateway {
        crate::test_current_status::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": {"name": "gateway", "namespace": "apps"},
            "spec": {"gatewayClassName": "public", "listeners": [
                {"name": "web", "port": 8080, "protocol": "HTTP", "hostname": "example.com"}
            ]}
        }))
        .unwrap()
    }

    fn route(kind: &str, rules: Value) -> Value {
        json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": kind,
            "metadata": {"name": "route", "namespace": "apps"},
            "spec": {"resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{"name": "gateway", "sectionName": "web"}],
                "resolvedListeners": ["0.0.0.0:8080/HTTP"], "rules": rules}
        })
    }

    fn extension(name: &str) -> Value {
        json!({"type": "ExtensionRef", "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": name}})
    }

    #[test]
    fn gateway_projection_preserves_annotation_and_serving_listener_contracts() {
        let mut gateway = gateway();
        gateway.metadata.annotations = Some(std::collections::BTreeMap::from([
            (
                annotations::EDGION_PLUGINS.to_owned(),
                "local shared/plugin local".to_owned(),
            ),
            (
                annotations::EDGION_STREAM_PLUGINS.to_owned(),
                " shared/connection ".to_owned(),
            ),
        ]));
        let input = GatewayInput::from_gateway(&gateway);
        assert_eq!(input.class_name, "public");
        assert_eq!(input.http_keys, ["apps/local", "shared/plugin"]);
        assert_eq!(input.connection_key.as_deref(), Some(""));
        assert_eq!(input.listeners[0].hostname.as_deref(), Some("example.com"));
        assert_eq!(input.listeners[0].key, ListenerKey("0.0.0.0:8080/HTTP".to_owned()));

        for (value, expected) in [(" local ", Some("apps/local")), ("  ", None)] {
            gateway
                .metadata
                .annotations
                .as_mut()
                .unwrap()
                .insert(annotations::EDGION_STREAM_PLUGINS.to_owned(), value.to_owned());
            assert_eq!(GatewayInput::from_gateway(&gateway).connection_key.as_deref(), expected);
        }
        for (condition, status) in [("Conflicted", "True"), ("Programmed", "False"), ("Accepted", "False")] {
            gateway.spec.current_status = Some(serde_json::from_value(json!({"listeners": [{
                "name": "web", "supportedKinds": [], "attachedRoutes": 0,
                "conditions": [{"type": condition, "status": status, "reason": "Test", "message": "", "lastTransitionTime": "2026-09-11T00:00:00Z"}]
            }]})).unwrap());
            assert!(GatewayInput::from_gateway(&gateway).listeners.is_empty(), "{condition}");
        }
    }

    #[test]
    fn http_delegation_preserves_source_identity_and_resolved_extension_namespaces() {
        let mut raw = route("HTTPRoute", json!([{"filters": [extension("original")]}]));
        let mut delegated = extension("delegated");
        delegated["extensionRef"]["resolvedNamespace"] = json!("child");
        raw["spec"]["invalidRuleIndices"] = json!([0]);
        raw["spec"]["resolvedRules"] = json!([{
            "filters": [delegated],
            "backendRefs": [{"name": "backend", "port": 80, "filters": [extension("backend-plugin")]}]
        }]);
        let source: Arc<HTTPRoute> = Arc::new(crate::test_current_status::from_value(raw).unwrap());
        let input = RouteInput::from_http(Arc::clone(&source));
        assert!(Arc::ptr_eq(input.http_route.as_ref().unwrap(), &source));
        assert_eq!(
            input.http_keys,
            HashSet::from(["child/delegated".to_owned(), "apps/backend-plugin".to_owned()])
        );
        assert_eq!(
            effective_http_rules(&source)
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            [0]
        );
    }

    #[test]
    fn http_demand_skips_invalid_rules_native_backend_cors_and_unsupported_refs() {
        let mut wrong_kind = extension("wrong-kind");
        wrong_kind["extensionRef"]["kind"] = json!("Secret");
        let mut wrong_group = extension("wrong-group");
        wrong_group["extensionRef"]["group"] = json!("other.io");
        let mut empty_group = extension("empty-group");
        empty_group["extensionRef"]["group"] = json!("");
        let mut raw = route(
            "HTTPRoute",
            json!([
                {"filters": [extension("invalid")]},
                {"filters": [extension("cors-invalid")], "backendRefs": [{"name": "backend", "port": 80, "filters": [{"type": "CORS", "cors": {}}]}]},
                {"filters": [extension("valid"), wrong_kind, wrong_group, empty_group]}
            ]),
        );
        raw["spec"]["invalidRuleIndices"] = json!([0]);
        let source = Arc::new(crate::test_current_status::from_value::<HTTPRoute>(raw).unwrap());
        assert_eq!(
            effective_http_rules(&source)
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            [2]
        );
        let input = RouteInput::from_http(source);
        assert_eq!(input.http_keys, HashSet::from(["apps/valid".to_owned()]));
    }

    #[test]
    fn grpc_demand_uses_effective_rules_and_backend_filters() {
        let mut raw = route("GRPCRoute", json!([{"filters": [extension("original")]}]));
        raw["spec"]["invalidRuleIndices"] = json!([0]);
        let mut source: GRPCRoute = crate::test_current_status::from_value(raw).unwrap();
        assert!(RouteInput::from_grpc(&source).http_keys.is_empty());
        let mut delegated = extension("delegated");
        delegated["extensionRef"]["resolvedNamespace"] = json!("child");
        source.spec.resolved_rules = Some(
            serde_json::from_value(json!([{
                "filters": [delegated],
                "backendRefs": [{"name": "backend", "port": 80, "filters": [extension("backend-plugin")]}]
            }]))
            .unwrap(),
        );
        let input = RouteInput::from_grpc(&source);
        assert_eq!(
            input.http_keys,
            HashSet::from(["child/delegated".to_owned(), "apps/backend-plugin".to_owned()])
        );
        assert!(input.http_route.is_none());
    }

    #[test]
    fn accepted_parents_keep_order_and_attach_only_to_allowed_serving_listeners() {
        let mut raw = route("HTTPRoute", json!([{}]));
        raw["spec"]["parentRefs"] = json!([
            {"name": "other", "kind": "Service"},
            {"name": "denied"},
            {"name": "gateway", "sectionName": "web"},
            {"name": "second"}
        ]);
        let mut source: HTTPRoute = crate::test_current_status::from_value(raw).unwrap();
        source.spec.current_status.as_mut().unwrap().parents[1].conditions[0].status = "False".into();
        let mut input = RouteInput::from_http(Arc::new(source));
        assert_eq!(
            input
                .parents
                .iter()
                .map(|parent| parent.gateway_key.as_str())
                .collect::<Vec<_>>(),
            ["apps/gateway", "apps/second"]
        );
        let mut gateway = GatewayInput::from_gateway(&gateway());
        assert!(input.attaches_to("apps/gateway", &gateway));
        assert!(!input.attaches_to("apps/other", &gateway));
        gateway.listeners[0].name = "other".to_owned();
        assert!(!input.attaches_to("apps/gateway", &gateway));
        gateway.listeners[0].name = "web".to_owned();
        input.namespace = "tenant".to_owned();
        assert!(
            !input.attaches_to("apps/gateway", &gateway),
            "default namespace policy is Same"
        );
        gateway.listeners[0].allowed_routes = Some(
            serde_json::from_value(json!({"namespaces": {"from": "All"}, "kinds": [{"kind": "HTTPRoute"}]})).unwrap(),
        );
        assert!(input.attaches_to("apps/gateway", &gateway));
        input.kind = RouteKind::Grpc;
        assert!(!input.attaches_to("apps/gateway", &gateway));
        input.kind = RouteKind::Http;
        input.listeners.clear();
        assert!(!input.attaches_to("apps/gateway", &gateway));
    }

    #[test]
    fn stream_routes_select_their_existing_stage_and_annotation_contract() {
        let mut raw = route("TCPRoute", json!([{}]));
        raw["metadata"]["annotations"] = json!({annotations::EDGION_STREAM_PLUGINS: " local "});
        let tcp = RouteInput::from_tcp(&serde_json::from_value(raw.clone()).unwrap());
        assert_eq!(tcp.stream_key.as_deref(), Some("apps/local"));
        assert!(tcp.stages.connection && !tcp.stages.tls_route);
        raw["kind"] = json!("UDPRoute");
        let udp = RouteInput::from_udp(&serde_json::from_value(raw.clone()).unwrap());
        assert_eq!(udp.stream_key, tcp.stream_key);
        assert_eq!(udp.stages, tcp.stages);
        raw["kind"] = json!("TLSRoute");
        raw["spec"]["hostnames"] = json!(["api.example.com"]);
        raw["spec"]["resolvedListeners"] = json!([{
            "listenerKey": "0.0.0.0:8443/TLS", "gatewayNamespace": "apps",
            "gatewayName": "gateway", "listenerName": "web", "hostnames": ["api.example.com"]
        }]);
        raw["metadata"]["annotations"] = json!({annotations::EDGION_STREAM_PLUGINS: " shared/policy "});
        let tls = RouteInput::from_tls(&crate::test_current_status::from_value(raw).unwrap());
        assert_eq!(tls.stream_key.as_deref(), Some("shared/policy"));
        assert!(!tls.stages.connection && tls.stages.tls_route);
        assert!(tls.http_keys.is_empty() && tls.http_route.is_none());
    }
}
