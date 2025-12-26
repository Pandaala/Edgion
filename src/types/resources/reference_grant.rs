//! ReferenceGrant resource definition
//!
//! ReferenceGrant identifies kinds of resources in other namespaces that are
//! trusted to reference the specified kinds of resources in the same namespace
//! as the policy.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// API group for ReferenceGrant
pub const REFERENCE_GRANT_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for ReferenceGrant
pub const REFERENCE_GRANT_KIND: &str = "ReferenceGrant";

/// ReferenceGrant identifies kinds of resources in other namespaces that are
/// trusted to reference the specified kinds of resources in the same namespace.
///
/// Each ReferenceGrant can be used to represent a unique trust relationship.
/// Additional Reference Grants can be used to add to the set of trusted sources
/// of inbound references for the namespace they are defined within.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1beta1",
    kind = "ReferenceGrant",
    plural = "referencegrants",
    shortname = "refgrant",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceGrantSpec {
    /// From describes the trusted namespaces and kinds that can reference the
    /// resources described in "To". Each entry in this list MUST be considered
    /// to be an additional place that references can be valid from, or to put
    /// this another way, entries MUST be combined using OR.
    pub from: Vec<ReferenceGrantFrom>,

    /// To describes the resources that may be referenced by the resources
    /// described in "From". Each entry in this list MUST be considered to be an
    /// additional place that references can be valid to, or to put this another
    /// way, entries MUST be combined using OR.
    pub to: Vec<ReferenceGrantTo>,
}

/// ReferenceGrantFrom describes trusted namespaces and kinds.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceGrantFrom {
    /// Group is the group of the referent.
    /// When empty, the Kubernetes core API group is inferred.
    pub group: String,

    /// Kind is the kind of the referent. Although implementations may support
    /// additional resources, the following types are part of the "Core"
    /// support level for this field:
    ///
    /// * Gateway (when used to permit a SecretObjectReference)
    /// * HTTPRoute
    /// * TCPRoute
    /// * TLSRoute
    /// * UDPRoute
    pub kind: String,

    /// Namespace is the namespace of the referent.
    pub namespace: String,
}

/// ReferenceGrantTo describes what Kinds are allowed as targets of the
/// references.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceGrantTo {
    /// Group is the group of the referent.
    /// When empty, the Kubernetes core API group is inferred.
    pub group: String,

    /// Kind is the kind of the referent. Although implementations may support
    /// additional resources, the following types are part of the "Core"
    /// support level for this field:
    ///
    /// * Secret (when used to permit a SecretObjectReference)
    /// * Service
    pub kind: String,

    /// Name is the name of the referent. When unspecified or empty, this policy
    /// refers to all resources of the specified Group and Kind in the local
    /// namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ReferenceGrant {
    /// Get the namespace of this resource
    pub fn namespace(&self) -> Option<&str> {
        self.metadata.namespace.as_deref()
    }

    /// Get the name of this resource
    pub fn name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or("")
    }

    /// Check if a reference from (namespace, group, kind) to (group, kind, name)
    /// is allowed by this ReferenceGrant.
    ///
    /// Note: This is a helper method for future validation logic.
    /// Current implementation is a placeholder.
    pub fn allows_reference(
        &self,
        _from_namespace: &str,
        _from_group: &str,
        _from_kind: &str,
        _to_group: &str,
        _to_kind: &str,
        _to_name: Option<&str>,
    ) -> bool {
        // Placeholder for future validation logic
        // This will be implemented when we add actual permission checking
        true
    }
}

