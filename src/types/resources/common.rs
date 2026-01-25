//! Common types shared across all route resources
//!
//! This module contains types that are used by multiple route resources
//! (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute, etc.)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
