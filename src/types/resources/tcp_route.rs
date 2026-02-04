//! TCPRoute resource definition
//!
//! TCPRoute defines TCP rules for mapping requests to backends

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

use super::common::{ParentReference, RefDenied};
use crate::core::lb::BackendSelector;
use crate::core::plugins::StreamPluginRuntime;

/// API group for TCPRoute
pub const TCP_ROUTE_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for TCPRoute
pub const TCP_ROUTE_KIND: &str = "TCPRoute";

/// TCPRoute defines TCP rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha2",
    kind = "TCPRoute",
    plural = "tcproutes",
    status = "TCPRouteStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct TCPRouteSpec {
    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Rules defines the TCP routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<TCPRouteRule>>,
}

/// TCPRouteRule defines TCP routing rules
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TCPRouteRule {
    /// BackendRefs defines the backends where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<TCPBackendRef>>,

    /// Backend finder for load balancing (not serialized/deserialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_finder: BackendSelector<TCPBackendRef>,

    /// Stream plugin runtime (runtime only, not serialized)
    /// This is populated from route annotations during pre-processing
    #[serde(skip)]
    #[schemars(skip)]
    pub stream_plugin_runtime: Arc<StreamPluginRuntime>,
}

impl Clone for TCPRouteRule {
    fn clone(&self) -> Self {
        Self {
            backend_refs: self.backend_refs.clone(),
            backend_finder: BackendSelector::new(),
            stream_plugin_runtime: self.stream_plugin_runtime.clone(),
        }
    }
}

impl fmt::Debug for TCPRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TCPRouteRule")
            .field("backend_refs", &self.backend_refs)
            .field("backend_finder", &"<skipped>")
            .field("stream_plugin_runtime", &self.stream_plugin_runtime)
            .finish()
    }
}

/// TCPBackendRef defines a backend for TCP traffic
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TCPBackendRef {
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

    /// Cross-namespace reference denial info
    /// Set by Controller when this backend's cross-namespace reference
    /// is not permitted (no matching ReferenceGrant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_denied: Option<RefDenied>,
}

// ============================================================================
// TCPRoute Status (Gateway API standard)
// ============================================================================

use super::http_route::RouteParentStatus;

/// TCPRouteStatus describes the status of the TCPRoute
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct TCPRouteStatus {
    /// Parents describe the status of the route with respect to each parent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<RouteParentStatus>,
}
