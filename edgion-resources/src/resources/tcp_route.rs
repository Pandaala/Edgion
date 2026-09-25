//! TCPRoute resource definition
//!
//! TCPRoute defines TCP rules for mapping requests to backends

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::{ParentReference, RefDenied, RouteListenerAttachment};
use crate::ListenerKey;

/// TCPRoute defines TCP rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "TCPRoute",
    plural = "tcproutes",
    status = "TCPRouteStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct TCPRouteSpec {
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<TCPRouteStatus>,

    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Controller-owned status identity, published with this processed route by conf_sync.
    /// Overwritten by the Controller; never an operator-authored field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_status_controller: Option<String>,

    /// Rules defines the TCP routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<TCPRouteRule>>,

    /// Pre-computed listener keys from parentRef resolution (set by Controller).
    /// Used by Gateway to bind this route to per-listener route tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_listeners: Option<Vec<ListenerKey>>,

    /// Exact Controller-resolved Gateway attachments for scoped data planes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_listener_attachments: Option<Vec<RouteListenerAttachment>>,
}

/// TCPRouteRule defines TCP routing rules
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TCPRouteRule {
    /// BackendRefs defines the backends where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<TCPBackendRef>>,
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
// TCPRoute Status (Gateway API standard)
// ============================================================================

use super::http_route::RouteParentStatus;

/// TCPRouteStatus describes the status of the TCPRoute
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct TCPRouteStatus {
    /// Parents describe the status of the route with respect to each parent.
    ///
    /// Always serialized, even when empty: K8s status persistence uses JSON
    /// Merge Patch, and merge-patching an empty status object `{}` cannot
    /// clear a previously-persisted non-empty list — only an explicit
    /// `"parents": []` replaces it (reachable when all parentRefs are removed
    /// from a live route). Do not add skip_serializing_if here.
    #[serde(default)]
    pub parents: Vec<RouteParentStatus>,
}
