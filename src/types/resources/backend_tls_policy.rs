//! BackendTLSPolicy resource definition
//!
//! BackendTLSPolicy provides a way to configure how a Gateway connects to a backend via TLS.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// API group for BackendTLSPolicy
pub const BACKEND_TLS_POLICY_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for BackendTLSPolicy
pub const BACKEND_TLS_POLICY_KIND: &str = "BackendTLSPolicy";

/// BackendTLSPolicy provides a way to configure how a Gateway connects to a backend via TLS.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "BackendTLSPolicy",
    plural = "backendtlspolicies",
    shortname = "btlspolicy",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct BackendTLSPolicySpec {
    /// TargetRefs identifies the API object(s) to apply the policy to.
    pub target_refs: Vec<BackendTLSPolicyTargetRef>,

    /// Validation contains backend TLS validation configuration.
    pub validation: BackendTLSPolicyValidation,
}

/// BackendTLSPolicyTargetRef identifies an API object to apply policy to.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendTLSPolicyTargetRef {
    /// Group is the group of the target resource.
    pub group: String,

    /// Kind is kind of the target resource.
    pub kind: String,

    /// Name is the name of the target resource.
    pub name: String,

    /// Namespace is the namespace of the target resource.
    /// When unspecified, the local namespace is inferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// SectionName is the name of a section within the target resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_name: Option<String>,
}

/// BackendTLSPolicyValidation contains backend TLS validation configuration.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendTLSPolicyValidation {
    /// CACertificateRefs contains one or more references to Kubernetes objects that
    /// contain a PEM-encoded TLS CA certificate bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_certificate_refs: Option<Vec<BackendTLSPolicyCACertificateRef>>,

    /// Hostname is used for two purposes in the connection between the gateway and the backend:
    /// 1. Hostname MUST be used as the SNI to connect to the backend
    /// 2. Hostname MUST be used for authentication and MUST match the certificate
    ///    served by the matching backend.
    pub hostname: String,

    /// WellKnownCACertificates specifies whether system CA certificates may be used
    /// in the TLS handshake between the gateway and backend pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub well_known_ca_certificates: Option<WellKnownCACertificates>,
}

/// BackendTLSPolicyCACertificateRef identifies a ConfigMap or Secret containing a CA certificate bundle.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendTLSPolicyCACertificateRef {
    /// Group is the group of the referent. For example, "".
    /// When unspecified or empty string, core API group is inferred.
    #[serde(default)]
    pub group: String,

    /// Kind is the kind of the referent. For example "Secret" or "ConfigMap".
    pub kind: String,

    /// Name is the name of the referent.
    pub name: String,

    /// Namespace is the namespace of the referent. When unspecified, the local
    /// namespace is inferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// WellKnownCACertificates specifies whether system CA certificates may be used.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum WellKnownCACertificates {
    /// System CA certificates
    System,
}

impl BackendTLSPolicy {
    /// Get the namespace of this resource
    pub fn namespace(&self) -> Option<&str> {
        self.metadata.namespace.as_deref()
    }

    /// Get the name of this resource
    pub fn name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or("")
    }

    /// Check if this policy applies to a given target
    pub fn applies_to(&self, group: &str, kind: &str, name: &str, namespace: Option<&str>) -> bool {
        let policy_ns = self.namespace();
        
        self.spec.target_refs.iter().any(|target| {
            // Check group, kind, and name match
            let matches = target.group == group 
                && target.kind == kind 
                && target.name == name;
            
            if !matches {
                return false;
            }
            
            // Check namespace match
            match (&target.namespace, namespace, policy_ns) {
                // Target has explicit namespace
                (Some(target_ns), Some(resource_ns), _) => target_ns == resource_ns,
                // Target uses implicit namespace (same as policy)
                (None, Some(resource_ns), Some(policy_ns)) => policy_ns == resource_ns,
                // Both use implicit namespace
                (None, None, _) => true,
                _ => false,
            }
        })
    }
}

