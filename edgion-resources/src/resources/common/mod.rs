//! Common reusable types for Edgion custom resources.

pub mod group_name;
pub mod log_labels;
pub mod secret_object_ref;
pub mod secret_slot;
pub mod service_kind;
pub mod tls;

pub use log_labels::{resolve_log_ctx_keys, resolve_log_labels};
pub use secret_object_ref::SecretObjectReference;
pub use secret_slot::SecretSlot;
pub use service_kind::EdgionService;
pub use tls::{TlsCerts, TlsConfig};

// ---- Types shared across all route resources ----
// (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute, etc.)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Helper for `#[serde(skip_serializing_if)]` on `bool` fields.
pub fn is_false(v: &bool) -> bool {
    !*v
}

/// Per Gateway API spec, `""` and `"core"` both represent the Kubernetes core API group.
#[inline]
pub fn is_core_api_group(group: &str) -> bool {
    group.is_empty() || group == "core"
}

/// Check if two API groups are equivalent, treating `""` and `"core"` as the same.
pub fn api_groups_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    is_core_api_group(a) && is_core_api_group(b)
}

/// ParentReference identifies a parent resource (usually Gateway)
///
/// This type is shared across all route resources and follows the
/// Gateway API specification for parent references.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
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

impl ParentReference {
    /// Build parent key (gateway key) from parent_ref and route metadata
    ///
    /// Priority:
    /// 1. parent_ref.namespace if present
    /// 2. route_namespace if present
    /// 3. "default" as fallback
    ///
    /// Returns: "{namespace}/{name}"
    pub fn build_parent_key(&self, route_namespace: Option<&str>) -> String {
        let namespace = self.namespace.as_deref().or(route_namespace).unwrap_or("default");
        format!("{}/{}", namespace, self.name)
    }
}

/// Cross-namespace reference denial info
///
/// Set by Controller when a cross-namespace reference is not permitted
/// (no matching ReferenceGrant). This info is synced to Gateway and used
/// to reject requests and log denial details to access log.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RefDenied {
    /// The target namespace that was denied
    pub target_namespace: String,

    /// The target resource name that was denied
    pub target_name: String,

    /// Optional reason for denial (e.g., "NoMatchingReferenceGrant")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Condition contains details for one aspect of the current state of this API Resource.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    /// LastTransitionTime is the last time the condition transitioned from one status to another.
    pub last_transition_time: String,

    /// Message is a human readable message indicating details about the transition.
    pub message: String,

    /// ObservedGeneration represents the .metadata.generation that the condition was set based upon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    /// Reason contains a programmatic identifier indicating the reason for the condition's last transition.
    pub reason: String,

    /// Status of the condition, one of True, False, Unknown.
    pub status: String,

    /// Type of condition.
    #[serde(rename = "type")]
    pub type_: String,
}

/// Resolved, comparable TCP keepalive configuration for an L4 upstream.
///
/// Deliberately an Edgion-local type rather than Pingora's `TcpKeepalive`:
/// it is `PartialEq`/`Eq` (Pingora's is not). Converted to Pingora's runtime
/// type at the data-plane edge via [`TcpKeepaliveConfig::to_pingora`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpKeepaliveConfig {
    pub idle: std::time::Duration,
    pub interval: std::time::Duration,
    pub count: usize,
}

impl TcpKeepaliveConfig {
    /// Convert to Pingora's runtime keepalive type. `user_timeout` is
    /// Linux-only; `Duration::ZERO` tells the kernel to use the system default.
    pub fn to_pingora(&self) -> pingora_core::protocols::l4::ext::TcpKeepalive {
        pingora_core::protocols::l4::ext::TcpKeepalive {
            idle: self.idle,
            interval: self.interval,
            count: self.count,
            #[cfg(target_os = "linux")]
            user_timeout: std::time::Duration::ZERO,
        }
    }
}
