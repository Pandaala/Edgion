//! TLSRoute resource definition
//!
//! TLSRoute defines TLS rules for mapping requests to backends

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

use super::common::{ParentReference, RefDenied};
use super::http_route_preparse::BackendExtensionInfo;
use crate::core::gateway::lb::BackendSelector;
use crate::core::gateway::plugins::PluginRuntime;
use crate::types::ListenerKey;

/// API group for TLSRoute
pub const TLS_ROUTE_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for TLSRoute
pub const TLS_ROUTE_KIND: &str = "TLSRoute";

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
    pub hostnames: Option<Vec<String>>,

    /// Rules defines the TLS routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<TLSRouteRule>>,

    /// Pre-computed listener keys from parentRef resolution (set by Controller).
    /// Used by Gateway to bind this route to per-listener route tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_listeners: Option<Vec<ListenerKey>>,

    /// Effective hostnames after intersecting route hostnames with listener hostnames.
    /// Set by Controller; used by Gateway for route matching instead of raw `hostnames`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_hostnames: Option<Vec<String>>,
}

/// TLSRouteRule defines TLS routing rules
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TLSRouteRule {
    /// BackendRefs defines the backends where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<TLSBackendRef>>,

    /// Backend finder for load balancing (not serialized/deserialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_finder: BackendSelector<TLSBackendRef>,

    /// Filter runtime (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,

    /// Proxy Protocol version to send to upstream (runtime only, from annotation).
    /// None = disabled, Some(2) = PP2.
    #[serde(skip)]
    #[schemars(skip)]
    pub proxy_protocol_version: Option<u8>,

    /// Store key for dynamic stream plugin lookup (runtime only, from annotation).
    /// Format: "namespace/name" referencing an EdgionStreamPlugins resource.
    #[serde(skip)]
    #[schemars(skip)]
    pub stream_plugin_store_key: Option<String>,

    /// Max upstream connect attempts (runtime only, from annotation).
    /// 1 = no retry (default). Values > 1 enable retry with next-backend selection.
    #[serde(skip)]
    #[schemars(skip)]
    pub max_connect_retries: u32,
}

impl Clone for TLSRouteRule {
    fn clone(&self) -> Self {
        Self {
            backend_refs: self.backend_refs.clone(),
            backend_finder: BackendSelector::new(),
            plugin_runtime: self.plugin_runtime.clone(),
            proxy_protocol_version: self.proxy_protocol_version,
            stream_plugin_store_key: self.stream_plugin_store_key.clone(),
            max_connect_retries: self.max_connect_retries,
        }
    }
}

impl fmt::Debug for TLSRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TLSRouteRule")
            .field("backend_refs", &self.backend_refs)
            .field("backend_finder", &"<skipped>")
            .field("plugin_runtime", &self.plugin_runtime)
            .field("proxy_protocol_version", &self.proxy_protocol_version)
            .field("stream_plugin_store_key", &self.stream_plugin_store_key)
            .field("max_connect_retries", &self.max_connect_retries)
            .finish()
    }
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

    /// Parsed extension info (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub extension_info: BackendExtensionInfo,

    /// Filter runtime (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub plugin_runtime: Arc<PluginRuntime>,

    /// Cross-namespace reference denial info
    /// Set by Controller when this backend's cross-namespace reference
    /// is not permitted (no matching ReferenceGrant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
                        extension_info: Default::default(),
                        plugin_runtime: Default::default(),
                        ref_denied: None,
                    }]),
                    backend_finder: BackendSelector::new(),
                    plugin_runtime: Default::default(),
                    proxy_protocol_version: None,
                    stream_plugin_store_key: None,
                    max_connect_retries: 1,
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
