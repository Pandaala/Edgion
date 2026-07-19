//! TLSRoute resource definition
//!
//! TLSRoute defines TLS rules for mapping requests to backends

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::{ParentReference, RefDenied};
use crate::ListenerKey;

/// TLSRoute defines TLS rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "TLSRoute",
    plural = "tlsroutes",
    status = "TLSRouteStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct TLSRouteSpec {
    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Hostnames defines a set of SNI names that should match against the SNI attribute
    /// of TLS ClientHello message in TLS handshake
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(inner(
        length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
        pattern(crate::gwapi_types::HOSTNAME_PATTERN)
    ))]
    pub hostnames: Option<Vec<String>>,

    /// Rules defines the TLS routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<TLSRouteRule>>,

    /// Pre-computed listener keys from parentRef resolution (set by Controller).
    /// Used by Gateway to bind this route to per-listener route tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_listeners: Option<Vec<ListenerKey>>,

    /// Effective hostnames after intersecting route hostnames with listener hostnames.
    /// Set by Controller; used by Gateway for route matching instead of raw `hostnames`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub effective_hostnames: Option<Vec<String>>,
}

/// TLSRouteRule defines TLS routing rules
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TLSRouteRule {
    /// BackendRefs defines the backends where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<TLSBackendRef>>,
}

/// TLSBackendRef defines a backend for TLS traffic
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TLSBackendRef {
    /// Name is the name of the backend Service
    pub name: String,

    /// Namespace is the namespace of the backend Service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Port specifies the destination port number
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
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

    /// Cross-namespace reference denial info
    /// Set by Controller when this backend's cross-namespace reference
    /// is not permitted (no matching ReferenceGrant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub ref_denied: Option<RefDenied>,
}

// ============================================================================
// TLSRoute v1alpha3 (legacy, converted to v1 when needed)
// ============================================================================

/// TLSRoute (v1alpha3 — legacy, converted to v1 when needed)
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha3",
    kind = "TLSRouteAlpha3",
    plural = "tlsroutes",
    status = "TLSRouteStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct TLSRouteAlpha3Spec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(inner(
        length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
        pattern(crate::gwapi_types::HOSTNAME_PATTERN)
    ))]
    pub hostnames: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<TLSRouteRule>>,
}

impl From<TLSRouteAlpha3> for TLSRoute {
    fn from(alpha3: TLSRouteAlpha3) -> Self {
        Self {
            metadata: alpha3.metadata,
            spec: TLSRouteSpec {
                parent_refs: alpha3.spec.parent_refs,
                hostnames: alpha3.spec.hostnames,
                rules: alpha3.spec.rules,
                resolved_listeners: None,
                effective_hostnames: None,
            },
            status: alpha3.status,
        }
    }
}

impl From<TLSRoute> for TLSRouteAlpha3 {
    fn from(v1: TLSRoute) -> Self {
        Self {
            metadata: v1.metadata,
            spec: TLSRouteAlpha3Spec {
                parent_refs: v1.spec.parent_refs,
                hostnames: v1.spec.hostnames,
                rules: v1.spec.rules,
            },
            status: v1.status,
        }
    }
}

// ============================================================================
// TLSRoute Status (Gateway API standard)
// ============================================================================

use super::http_route::RouteParentStatus;

/// TLSRouteStatus describes the status of the TLSRoute
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct TLSRouteStatus {
    /// Parents describe the status of the route with respect to each parent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<RouteParentStatus>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn make_tls_route() -> TLSRoute {
        TLSRoute {
            metadata: ObjectMeta {
                name: Some("test-tls-route".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: TLSRouteSpec {
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some("default".to_string()),
                    name: "my-gateway".to_string(),
                    section_name: Some("tls-listener".to_string()),
                    port: None,
                }]),
                hostnames: Some(vec!["example.com".to_string()]),
                rules: Some(vec![TLSRouteRule {
                    backend_refs: Some(vec![TLSBackendRef {
                        name: "backend-svc".to_string(),
                        namespace: None,
                        port: Some(443),
                        weight: Some(1),
                        group: None,
                        kind: None,
                        ref_denied: None,
                    }]),
                }]),
                resolved_listeners: None,
                effective_hostnames: None,
            },
            status: Some(TLSRouteStatus { parents: vec![] }),
        }
    }

    #[test]
    fn test_alpha3_to_v1_conversion() {
        let route = make_tls_route();
        let alpha3: TLSRouteAlpha3 = route.clone().into();
        let restored: TLSRoute = alpha3.into();

        assert_eq!(restored.metadata.name, route.metadata.name);
        assert_eq!(restored.metadata.namespace, route.metadata.namespace);
        assert_eq!(restored.spec.hostnames, route.spec.hostnames);
        assert!(restored.spec.resolved_listeners.is_none());
        assert_eq!(
            restored.spec.parent_refs.as_ref().unwrap().len(),
            route.spec.parent_refs.as_ref().unwrap().len()
        );
        assert_eq!(
            restored.spec.rules.as_ref().unwrap().len(),
            route.spec.rules.as_ref().unwrap().len()
        );
        assert_eq!(
            restored.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0].name,
            "backend-svc"
        );
    }

    #[test]
    fn test_v1_to_alpha3_roundtrip() {
        let route = make_tls_route();
        let alpha3: TLSRouteAlpha3 = route.clone().into();
        let back_to_v1: TLSRoute = alpha3.into();
        let back_to_alpha3: TLSRouteAlpha3 = back_to_v1.into();
        let final_v1: TLSRoute = back_to_alpha3.into();

        assert_eq!(final_v1.metadata.name, route.metadata.name);
        assert_eq!(final_v1.spec.hostnames, route.spec.hostnames);
        assert_eq!(final_v1.status.as_ref().unwrap().parents.len(), 0);
    }

    #[test]
    fn test_alpha3_deserialization_compat() {
        let route = make_tls_route();
        let alpha3: TLSRouteAlpha3 = route.into();
        let json = serde_json::to_string(&alpha3).unwrap();

        let restored: TLSRoute = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.metadata.name.as_deref(), Some("test-tls-route"));
        assert_eq!(restored.spec.hostnames.as_ref().unwrap()[0], "example.com");
    }
}
