//! BackendTLSPolicy resource definition
//!
//! BackendTLSPolicy provides a way to configure how a Gateway connects to a backend via TLS.

use super::common::{Condition, ParentReference};
use super::gateway::SecretObjectReference;
use k8s_openapi::api::core::v1::Secret;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// API group for BackendTLSPolicy
pub const BACKEND_TLS_POLICY_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for BackendTLSPolicy
pub const BACKEND_TLS_POLICY_KIND: &str = "BackendTLSPolicy";
/// Implementation-specific option key for upstream mTLS client certificate Secret reference.
///
/// Value format:
/// - `secret-name` (same namespace as BackendTLSPolicy)
/// - `namespace/secret-name` (currently treated as invalid by Edgion parser)
pub const OPTION_CLIENT_CERTIFICATE_REF: &str = "edgion.io/client-certificate-ref";

/// BackendTLSPolicy provides a way to configure how a Gateway connects to a backend via TLS.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "BackendTLSPolicy",
    plural = "backendtlspolicies",
    shortname = "btlspolicy",
    namespaced,
    status = "BackendTLSPolicyStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct BackendTLSPolicySpec {
    /// TargetRefs identifies the API object(s) to apply the policy to.
    pub target_refs: Vec<BackendTLSPolicyTargetRef>,

    /// Validation contains backend TLS validation configuration.
    pub validation: BackendTLSPolicyValidation,

    /// Options are a list of key/value pairs to enable extended TLS configuration.
    /// Implementation-specific field for configuring TLS options like minimum TLS version or cipher suites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<HashMap<String, String>>,

    /// Resolved CA certificate Secrets (runtime only, filled by controller).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ca_certificates: Option<Vec<Secret>>,

    /// Resolved client certificate Secret for upstream mTLS (runtime only, filled by controller).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_client_certificate: Option<Secret>,
}

/// BackendTLSPolicyTargetRef identifies an API object to apply policy to.
/// Note: Per Gateway API spec, targetRef can only reference resources in the same namespace.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendTLSPolicyTargetRef {
    /// Group is the group of the target resource.
    pub group: String,

    /// Kind is kind of the target resource.
    pub kind: String,

    /// Name is the name of the target resource.
    pub name: String,

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

    /// SubjectAltNames contains one or more Subject Alternative Names.
    /// When specified, the certificate served from the backend MUST have at least one
    /// Subject Alternative Name matching one of the specified SubjectAltNames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_alt_names: Option<Vec<SubjectAltName>>,

    /// WellKnownCACertificates specifies whether system CA certificates may be used
    /// in the TLS handshake between the gateway and backend pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub well_known_ca_certificates: Option<WellKnownCACertificates>,
}

/// BackendTLSPolicyCACertificateRef identifies a ConfigMap or Secret containing a CA certificate bundle.
/// Note: Per Gateway API spec, caCertificateRef can only reference resources in the same namespace.
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
}

/// SubjectAltName represents a Subject Alternative Name.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubjectAltName {
    /// Type determines the format of the Subject Alternative Name.
    #[serde(rename = "type")]
    pub san_type: SubjectAltNameType,

    /// Hostname contains Subject Alternative Name specified in DNS name format.
    /// Required when Type is set to Hostname, ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// URI contains Subject Alternative Name specified in a full URI format.
    /// Required when Type is set to URI, ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// SubjectAltNameType specifies the type of Subject Alternative Name.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum SubjectAltNameType {
    /// Hostname type - DNS name format
    Hostname,
    /// URI type - full URI format (e.g., SPIFFE ID)
    URI,
}

/// WellKnownCACertificates specifies whether system CA certificates may be used.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum WellKnownCACertificates {
    /// System CA certificates
    System,
}

impl BackendTLSPolicy {
    /// Parse implementation-specific client certificate Secret reference from options.
    ///
    /// Supported value format:
    /// - `secret-name` (same namespace)
    ///
    /// Returns an error message if the value is present but invalid.
    pub fn client_certificate_secret_ref(&self) -> Result<Option<SecretObjectReference>, String> {
        let raw = self
            .spec
            .options
            .as_ref()
            .and_then(|o| o.get(OPTION_CLIENT_CERTIFICATE_REF))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());

        let Some(raw) = raw else {
            return Ok(None);
        };

        if raw.contains('/') {
            let msg = format!(
                "Option {} must be a secret name in the same namespace, got '{}'",
                OPTION_CLIENT_CERTIFICATE_REF, raw
            );
            return Err(msg);
        }

        Ok(Some(SecretObjectReference {
            group: Some(String::new()),
            kind: Some("Secret".to_string()),
            name: raw.to_string(),
            namespace: None,
        }))
    }

    /// Get the namespace of this resource
    pub fn namespace(&self) -> Option<&str> {
        self.metadata.namespace.as_deref()
    }

    /// Get the name of this resource
    pub fn name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or("")
    }

    /// Check if this policy applies to a given target
    /// Per Gateway API spec, targetRef can only reference resources in the same namespace as the policy.
    pub fn applies_to(&self, group: &str, kind: &str, name: &str, namespace: Option<&str>) -> bool {
        let policy_ns = self.namespace();

        self.spec.target_refs.iter().any(|target| {
            // Check group, kind, and name match
            let matches = target.group == group && target.kind == kind && target.name == name;

            if !matches {
                return false;
            }

            // Check namespace match - target must be in the same namespace as the policy
            match (namespace, policy_ns) {
                (Some(resource_ns), Some(policy_ns)) => policy_ns == resource_ns,
                (None, None) => true,
                _ => false,
            }
        })
    }
}

// ============================================================================
// BackendTLSPolicy Status (Gateway API standard)
// ============================================================================

/// BackendTLSPolicyStatus describes the status of the BackendTLSPolicy
/// Following Gateway API PolicyAncestorStatus pattern
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackendTLSPolicyStatus {
    /// Ancestors is a list of ancestor resources (usually Gateways) that are
    /// associated with the policy, and the status of the policy with respect to
    /// each ancestor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<PolicyAncestorStatus>,
}

/// PolicyAncestorStatus describes the status of a policy with respect to an
/// ancestor resource. Ancestors refer to the Gateway(s) that this policy
/// is associated with.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyAncestorStatus {
    /// AncestorRef corresponds with a ParentRef in the spec that this
    /// PolicyAncestorStatus struct describes the status of.
    pub ancestor_ref: ParentReference,

    /// ControllerName is a domain/path string that indicates the name of the
    /// controller that wrote this status.
    pub controller_name: String,

    /// Conditions describes the status of the Policy with respect to the given Ancestor.
    pub conditions: Vec<Condition>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn make_policy(options: Option<HashMap<String, String>>) -> BackendTLSPolicy {
        BackendTLSPolicy {
            metadata: ObjectMeta {
                name: Some("test".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: "".to_string(),
                    kind: "Service".to_string(),
                    name: "svc".to_string(),
                    section_name: None,
                }],
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: "backend.example.com".to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: None,
                },
                options,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
            },
            status: None,
        }
    }

    #[test]
    fn test_client_certificate_secret_ref_none() {
        let policy = make_policy(None);
        assert!(policy.client_certificate_secret_ref().unwrap().is_none());
    }

    #[test]
    fn test_client_certificate_secret_ref_valid() {
        let mut options = HashMap::new();
        options.insert(
            OPTION_CLIENT_CERTIFICATE_REF.to_string(),
            "client-cert-secret".to_string(),
        );
        let policy = make_policy(Some(options));
        let ref_ = policy.client_certificate_secret_ref().unwrap().unwrap();
        assert_eq!(ref_.name, "client-cert-secret");
        assert!(ref_.namespace.is_none());
    }

    #[test]
    fn test_client_certificate_secret_ref_invalid_namespace_value() {
        let mut options = HashMap::new();
        options.insert(
            OPTION_CLIENT_CERTIFICATE_REF.to_string(),
            "default/client-cert-secret".to_string(),
        );
        let policy = make_policy(Some(options));
        assert!(policy.client_certificate_secret_ref().is_err());
    }
}
