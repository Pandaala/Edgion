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

use super::common::api_groups_match;

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
    /// # Arguments
    /// * `from_namespace` - Namespace of the source resource
    /// * `from_group` - Group of the source resource
    /// * `from_kind` - Kind of the source resource
    /// * `to_group` - Group of the target resource
    /// * `to_kind` - Kind of the target resource
    /// * `to_name` - Optional name of the target resource
    ///
    /// # Returns
    /// `true` if this grant allows the reference, `false` otherwise
    ///
    /// # Logic
    /// 1. Check if any "from" entry matches (namespace, group, kind)
    /// 2. Check if any "to" entry matches (group, kind, name)
    /// 3. Both must match for the reference to be allowed
    pub fn allows_reference(
        &self,
        from_namespace: &str,
        from_group: &str,
        from_kind: &str,
        to_group: &str,
        to_kind: &str,
        to_name: Option<&str>,
    ) -> bool {
        // Check if from matches
        let from_matches =
            self.spec.from.iter().any(|f| {
                f.namespace == from_namespace && api_groups_match(&f.group, from_group) && f.kind == from_kind
            });

        if !from_matches {
            return false;
        }

        // Check if to matches
        self.spec.to.iter().any(|t| {
            api_groups_match(&t.group, to_group)
                && t.kind == to_kind
                && (t.name.is_none() || t.name.as_deref() == to_name)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    #[allow(clippy::too_many_arguments)]
    fn create_test_grant(
        namespace: &str,
        name: &str,
        from_namespace: &str,
        from_group: &str,
        from_kind: &str,
        to_group: &str,
        to_kind: &str,
        to_name: Option<&str>,
    ) -> ReferenceGrant {
        ReferenceGrant {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantSpec {
                from: vec![ReferenceGrantFrom {
                    group: from_group.to_string(),
                    kind: from_kind.to_string(),
                    namespace: from_namespace.to_string(),
                }],
                to: vec![ReferenceGrantTo {
                    group: to_group.to_string(),
                    kind: to_kind.to_string(),
                    name: to_name.map(|s| s.to_string()),
                }],
            },
        }
    }

    #[test]
    fn test_allows_reference_basic() {
        // Grant: allow HTTPRoute from ns-source to access Service in ns-target
        let grant = create_test_grant(
            "ns-target",
            "test-grant",
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            None,
        );

        // Should allow: matching from and to
        assert!(grant.allows_reference(
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            Some("my-service")
        ));

        // Should deny: wrong from namespace
        assert!(!grant.allows_reference(
            "ns-other",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            Some("my-service")
        ));

        // Should deny: wrong from kind
        assert!(!grant.allows_reference(
            "ns-source",
            "gateway.networking.k8s.io",
            "TCPRoute",
            "",
            "Service",
            Some("my-service")
        ));

        // Should deny: wrong to kind
        assert!(!grant.allows_reference(
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Secret",
            Some("my-secret")
        ));
    }

    #[test]
    fn test_allows_reference_with_specific_name() {
        // Grant: allow HTTPRoute from ns-source to access specific Service "allowed-svc" in ns-target
        let grant = create_test_grant(
            "ns-target",
            "test-grant",
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            Some("allowed-svc"),
        );

        // Should allow: specific service name matches
        assert!(grant.allows_reference(
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            Some("allowed-svc")
        ));

        // Should deny: different service name
        assert!(!grant.allows_reference(
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            Some("other-svc")
        ));
    }

    #[test]
    fn test_allows_reference_wildcard_name() {
        // Grant: allow HTTPRoute from ns-source to access any Service in ns-target (name: None)
        let grant = create_test_grant(
            "ns-target",
            "test-grant",
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            None,
        );

        // Should allow: any service name
        assert!(grant.allows_reference(
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            Some("any-service")
        ));

        assert!(grant.allows_reference(
            "ns-source",
            "gateway.networking.k8s.io",
            "HTTPRoute",
            "",
            "Service",
            Some("another-service")
        ));
    }
}
