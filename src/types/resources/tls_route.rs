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

/// API group for TLSRoute
pub const TLS_ROUTE_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for TLSRoute
pub const TLS_ROUTE_KIND: &str = "TLSRoute";

/// TLSRoute defines TLS rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha2",
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

    /// Resolved listener ports from parentRefs (computed by controller).
    /// Derived from parentRef.port or parentRef.sectionName → Gateway listener.port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ports: Option<Vec<u16>>,
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

    /// Whether to use TLS when connecting to upstream (runtime only, from annotation).
    /// false = plain TCP (default), true = TLS.
    #[serde(skip)]
    #[schemars(skip)]
    pub upstream_tls: bool,

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
            upstream_tls: self.upstream_tls,
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
            .field("upstream_tls", &self.upstream_tls)
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
