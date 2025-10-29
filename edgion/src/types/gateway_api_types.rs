//! Native Gateway API type definitions
//!
//! This module provides type-safe definitions for Kubernetes Gateway API resources
//! without depending on the gateway-api crate.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// GatewayClass defines a class of Gateways that can be instantiated
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "GatewayClass",
    plural = "gatewayclasses"
)]
#[serde(rename_all = "camelCase")]
pub struct GatewayClassSpec {
    /// ControllerName is the name of the controller
    pub controller_name: String,

    /// Description is a human-readable description of the class
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// ParametersRef references a resource that contains parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_ref: Option<ParametersReference>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParametersReference {
    /// Group is the group of the referent
    pub group: String,

    /// Kind is the kind of the referent
    pub kind: String,

    /// Name is the name of the referent
    pub name: String,

    /// Namespace is the namespace of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// Gateway defines a network gateway
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "Gateway",
    plural = "gateways",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySpec {
    /// GatewayClassName used for this Gateway
    pub gateway_class_name: String,

    /// Listeners associated with this Gateway
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listeners: Option<Vec<Listener>>,

    /// Addresses requested for this Gateway
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<GatewayAddress>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Listener {
    /// Name of the Listener
    pub name: String,

    /// Hostname associated with the Listener
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// Port on which the Listener is listening
    pub port: i32,

    /// Protocol of the Listener
    pub protocol: String,

    /// TLS configuration for the Listener
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<GatewayTLSConfig>,

    /// AllowedRoutes defines which Routes may be attached
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_routes: Option<AllowedRoutes>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayTLSConfig {
    /// Mode defines the TLS behavior for the TLS session
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    /// CertificateRefs contains references to Kubernetes objects
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_refs: Option<Vec<SecretObjectReference>>,

    /// Options are implementation-specific TLS options
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecretObjectReference {
    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is kind of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Name is the name of the referent
    pub name: String,

    /// Namespace is the namespace of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AllowedRoutes {
    /// Namespaces indicates namespaces from which Routes may be attached
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespaces: Option<RouteNamespaces>,

    /// Kinds specifies the Route kinds that are allowed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<RouteGroupKind>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteNamespaces {
    /// From indicates where Routes should be selected
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    /// Selector must be specified when From is set to "Selector"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteGroupKind {
    /// Group is the group of the Route
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is the kind of the Route
    pub kind: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayAddress {
    /// Type of the address
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub address_type: Option<String>,

    /// Value of the address
    pub value: String,
}

/// HTTPRoute defines HTTP rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "HTTPRoute",
    plural = "httproutes",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteSpec {
    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Hostnames defines the set of hostnames
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostnames: Option<Vec<String>>,

    /// Rules defines the HTTP routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<HTTPRouteRule>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParentReference {
    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is the kind of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Namespace is the namespace of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Name is the name of the referent
    pub name: String,

    /// SectionName is the name of a section within the target resource
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_name: Option<String>,

    /// Port is the network port this Route targets
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteRule {
    /// Matches define conditions used for matching the rule against requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<HTTPRouteMatch>>,

    /// Filters define the filters that are applied to requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<serde_json::Value>>,

    /// BackendRefs defines the backend(s) where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<HTTPBackendRef>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteMatch {
    /// Path specifies a HTTP request path matcher
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<HTTPPathMatch>,

    /// Headers specifies HTTP request header matchers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<HTTPHeaderMatch>>,

    /// QueryParams specifies HTTP query parameter matchers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_params: Option<Vec<HTTPQueryParamMatch>>,

    /// Method specifies HTTP method matcher
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPPathMatch {
    /// Type specifies how to match against the path Value
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Value of the HTTP path to match against
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPHeaderMatch {
    /// Type specifies how to match the header
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Name is the name of the HTTP Header
    pub name: String,

    /// Value is the value of HTTP Header
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPQueryParamMatch {
    /// Type specifies how to match the query parameter
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Name is the name of the query parameter
    pub name: String,

    /// Value is the value of the query parameter
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPBackendRef {
    /// Name is the name of the backend Service
    pub name: String,

    /// Namespace is the namespace of the backend Service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Port specifies the destination port number
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,

    /// Weight specifies the proportion of requests forwarded to the backend
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,

    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is the kind of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// Service is a named abstraction of software service
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    /// Standard object metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<kube::core::ObjectMeta>,

    /// Spec defines the behavior of a service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<ServiceSpec>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSpec {
    /// ClusterIP is the IP address of the service
    #[serde(rename = "clusterIP", default, skip_serializing_if = "Option::is_none")]
    pub cluster_ip: Option<String>,

    /// ClusterIPs is a list of IP addresses assigned to this service
    #[serde(rename = "clusterIPs", default, skip_serializing_if = "Option::is_none")]
    pub cluster_i_ps: Option<Vec<String>>,

    /// Type determines how the Service is exposed
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub service_type: Option<String>,

    /// Ports is the list of ports that are exposed by this service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<ServicePort>>,

    /// Selector is a label query over pods that should match the service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<std::collections::HashMap<String, String>>,

    /// ExternalIPs is a list of IP addresses for which nodes will accept traffic
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_i_ps: Option<Vec<String>>,

    /// LoadBalancerIP is deprecated. Use LoadBalancerClass instead
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_balancer_ip: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServicePort {
    /// Name of this port within the service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Protocol for port (TCP or UDP)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,

    /// Port number that will be exposed by this service
    pub port: i32,

    /// TargetPort is the port to access on the pods targeted by the service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_port: Option<serde_json::Value>,

    /// NodePort is the port on each node on which this service is exposed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_port: Option<i32>,
}

/// Endpoints is a collection of endpoints that implement the actual service
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Endpoints {
    /// Standard object metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<kube::core::ObjectMeta>,

    /// Subsets is the set of all endpoints
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subsets: Option<Vec<EndpointSubset>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSubset {
    /// Addresses is a list of IP addresses
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<EndpointAddress>>,

    /// NotReadyAddresses is a list of addresses of endpoints that are not currently ready
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_ready_addresses: Option<Vec<EndpointAddress>>,

    /// Ports is the list of port numbers available on the related IP addresses
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<EndpointPort>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndpointAddress {
    /// IP is the IP address of this endpoint
    pub ip: String,

    /// Hostname is the Hostname of this endpoint
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// NodeName is the name of the Node hosting this endpoint
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,

    /// TargetRef is a reference to a Kubernetes object
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<ObjectReference>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndpointPort {
    /// Name of this port
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Port number of the endpoint
    pub port: i32,

    /// Protocol for this port (TCP or UDP)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,

    /// AppProtocol is the application protocol for this port
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_protocol: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObjectReference {
    /// Kind of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Namespace of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Name of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// UID of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,

    /// API version of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,

    /// Resource version of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_serialization() {
        let gateway = Gateway {
            metadata: kube::core::ObjectMeta {
                name: Some("test-gateway".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: GatewaySpec {
                gateway_class_name: "test-class".to_string(),
                listeners: Some(vec![Listener {
                    name: "http".to_string(),
                    hostname: None,
                    port: 80,
                    protocol: "HTTP".to_string(),
                    tls: None,
                    allowed_routes: None,
                }]),
                addresses: None,
            },
        };

        let yaml = serde_yaml::to_string(&gateway).unwrap();
        println!("{}", yaml);
        assert!(yaml.contains("gatewayClassName"));
    }
}
