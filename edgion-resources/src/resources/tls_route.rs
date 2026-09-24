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
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<TLSRouteStatus>,

    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Controller-owned status identity, published with this processed route by conf_sync.
    /// Overwritten by the Controller; never an operator-authored field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_status_controller: Option<String>,

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

    /// Controller-resolved attachments and their individual hostname intersections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_listeners: Option<Vec<ResolvedTlsListener>>,
}

/// Internal conf-sync data; never exposed as operator-authored CRD fields.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTlsListener {
    pub listener_key: ListenerKey,
    pub gateway_namespace: String,
    pub gateway_name: String,
    pub listener_name: String,
    pub hostnames: Vec<String>,
}

impl TLSRouteSpec {
    /// TLSRoute v1 requires explicit SNI names, including on unrestricted listeners.
    pub fn validate_hostnames(&self) -> Result<(), crate::HostnameError> {
        crate::validate_route_hostnames(self.hostnames.as_deref(), true)
    }
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
// TLSRoute Status (Gateway API standard)
// ============================================================================

use super::http_route::RouteParentStatus;

/// TLSRouteStatus describes the status of the TLSRoute
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct TLSRouteStatus {
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

#[cfg(test)]
#[path = "../../tests/unit/resources/tls_route_test.rs"]
mod tests;
