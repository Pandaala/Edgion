//! Gateway resource definition
//!
//! Gateway defines a network gateway

use k8s_openapi::api::core::v1::Secret;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// API group for Gateway
pub const GATEWAY_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for Gateway
pub const GATEWAY_KIND: &str = "Gateway";

use super::common::Condition;

/// Gateway defines a network gateway
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "Gateway",
    plural = "gateways",
    status = "GatewayStatus",
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

    /// Resolved Secret data (filled by Controller, not user-configured)
    /// Contains the actual TLS certificates referenced by certificate_refs
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Vec<Secret>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, JsonSchema)]
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

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    /// Addresses assigned to the Gateway
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<GatewayStatusAddress>>,

    /// Conditions describe the current conditions of the Gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<Condition>>,

    /// Listeners provide status for each unique listener port defined in the Gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listeners: Option<Vec<ListenerStatus>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatusAddress {
    /// Type of the address
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub address_type: Option<String>,

    /// Value of the address
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerStatus {
    /// Name is the name of the Listener that this status corresponds to.
    pub name: String,

    /// SupportedKinds is the list indicating the Kinds supported by this listener.
    pub supported_kinds: Vec<RouteGroupKind>,

    /// AttachedRoutes represents the total number of Routes that have been
    /// successfully attached to this Listener.
    pub attached_routes: i32,

    /// Conditions describe the current conditions of this listener.
    pub conditions: Vec<Condition>,
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
            status: None,
        };

        let yaml = serde_yaml::to_string(&gateway).unwrap();
        println!("{}", yaml);
        assert!(yaml.contains("gatewayClassName"));
    }
}
