//! BackendTLSPolicy resource definition
//!
//! BackendTLSPolicy provides a way to configure how a Gateway connects to a backend via TLS.

use super::common::PolicyAncestorStatus;

crate::redact::impl_redacted_debug!(BackendTLSPolicySpec);
use super::edgion_backend_traffic_policy::ResolvedPolicyTargetClass;
use super::gateway::SecretObjectReference;
use k8s_openapi::api::core::v1::Secret;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Implementation-specific option key for upstream mTLS client certificate Secret reference.
///
/// Value format:
/// - `secret-name` (same namespace as BackendTLSPolicy)
/// - `namespace/secret-name` (currently treated as invalid by Edgion parser)
pub const OPTION_CLIENT_CERTIFICATE_REF: &str = "edgion.io/client-certificate-ref";

/// BackendTLSPolicy provides a way to configure how a Gateway connects to a backend via TLS.
#[derive(CustomResource, Deserialize, Serialize, Clone, JsonSchema)]
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
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<BackendTLSPolicyStatus>,

    /// TargetRefs identifies the API object(s) to apply the policy to.
    pub target_refs: Vec<BackendTLSPolicyTargetRef>,

    /// Controller-resolved target capability used to gate runtime publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_target_class: Option<ResolvedPolicyTargetClass>,

    /// Validation contains backend TLS validation configuration.
    pub validation: BackendTLSPolicyValidation,

    /// Options are a list of key/value pairs to enable extended TLS configuration.
    /// Implementation-specific field for configuring TLS options like minimum TLS version or cipher suites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<HashMap<String, String>>,

    /// Resolved CA certificate Secrets (runtime only, filled by controller).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_ca_certificates: Option<Vec<Secret>>,

    /// Resolved client certificate Secret for upstream mTLS (runtime only, filled by controller).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_client_certificate: Option<Secret>,

    /// Whether to use system CA certificates for backend TLS verification (runtime only, filled by controller).
    /// Set to true when wellKnownCACertificates is "System".
    #[serde(default, skip_serializing_if = "crate::resources::common::is_false")]
    #[schemars(skip)]
    pub use_system_ca: bool,
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
    #[schemars(
        length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
        pattern(crate::gwapi_types::PRECISE_HOSTNAME_PATTERN)
    )]
    pub hostname: String,

    /// SubjectAltNames contains one or more Subject Alternative Names.
    /// When specified, the certificate served from the backend MUST have at least one
    /// Subject Alternative Name matching one of the specified SubjectAltNames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_alt_names: Option<Vec<SubjectAltName>>,

    /// WellKnownCACertificates specifies whether system CA certificates may be used
    /// in the TLS handshake between the gateway and backend pod.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "wellKnownCACertificates")]
    pub well_known_ca_certificates: Option<WellKnownCACertificates>,
}

/// Required standard local reference to a CA bundle in the policy namespace.
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendTLSPolicyCACertificateRef {
    /// Group is the group of the referent. For example, "".
    /// This field is required; an explicit empty string denotes the core group.
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
    #[schemars(
        length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
        pattern(crate::gwapi_types::HOSTNAME_PATTERN)
    )]
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WellKnownCACertificates {
    /// System CA certificates
    System,
    /// A Gateway API CA name accepted by newer CRDs but unsupported by Edgion.
    Other(String),
}

impl Serialize for WellKnownCACertificates {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::System => serializer.serialize_str("System"),
            Self::Other(name) => serializer.serialize_str(name),
        }
    }
}

impl<'de> Deserialize<'de> for WellKnownCACertificates {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(if name == "System" {
            Self::System
        } else {
            Self::Other(name)
        })
    }
}

impl JsonSchema for WellKnownCACertificates {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WellKnownCACertificates".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as JsonSchema>::json_schema(generator)
    }
}

/// Runtime validation twin for BackendTLSPolicy hostname and SAN admission rules.
///
/// Controller FS/etcd ingest and Gateway defensive publication must not rely on
/// Kubernetes schema validation having run first.
pub fn backend_tls_identity_validation_errors(validation: &BackendTLSPolicyValidation) -> Vec<String> {
    use regex::Regex;

    let mut errors = Vec::new();
    let precise_hostname = Regex::new(crate::gwapi_types::PRECISE_HOSTNAME_PATTERN)
        .expect("canonical precise-hostname pattern must compile");
    if validation.hostname.len() > crate::gwapi_types::HOSTNAME_MAX_LEN as usize
        || !precise_hostname.is_match(&validation.hostname)
    {
        errors.push("validation.hostname must be a valid precise hostname".to_string());
    }

    if let Some(subject_alt_names) = &validation.subject_alt_names {
        if subject_alt_names.is_empty() || subject_alt_names.len() > 5 {
            errors.push("validation.subjectAltNames must contain between one and five entries".to_string());
        }
        let hostname =
            Regex::new(crate::gwapi_types::HOSTNAME_PATTERN).expect("canonical hostname pattern must compile");
        for san in subject_alt_names {
            let valid = match san.san_type {
                SubjectAltNameType::Hostname => {
                    san.uri.is_none()
                        && san.hostname.as_ref().is_some_and(|value| {
                            value.len() <= crate::gwapi_types::HOSTNAME_MAX_LEN as usize && hostname.is_match(value)
                        })
                }
                SubjectAltNameType::URI => {
                    san.hostname.is_none()
                        && san.uri.as_ref().is_some_and(|value| {
                            value.len() <= crate::gwapi_types::HOSTNAME_MAX_LEN as usize
                                && url::Url::parse(value).is_ok()
                        })
                }
            };
            if !valid {
                errors.push("validation.subjectAltNames contains an invalid typed value".to_string());
            }
        }
    }
    errors
}

/// Parse implementation-specific client certificate Secret reference from a BTP options map.
///
/// Supported value format:
/// - `secret-name` (same namespace)
///
/// Returns an error message if the value is present but invalid.
pub fn parse_client_certificate_option(
    options: Option<&HashMap<String, String>>,
) -> Result<Option<SecretObjectReference>, String> {
    let Some(raw) = options.and_then(|options| options.get(OPTION_CLIENT_CERTIFICATE_REF)) else {
        return Ok(None);
    };
    let raw = raw.trim();

    if !valid_kube_name(raw) {
        let msg = format!(
            "Option {} must be a valid same-namespace Secret name",
            OPTION_CLIENT_CERTIFICATE_REF
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

fn valid_kube_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && label.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && label.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
        })
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
    ///
    /// Always serialized, even when empty: K8s status persistence uses JSON
    /// Merge Patch, and merge-patching an empty status object `{}` cannot
    /// clear a previously-persisted non-empty list — only an explicit
    /// `"ancestors": []` replaces it. Do not add skip_serializing_if here.
    #[serde(default)]
    pub ancestors: Vec<PolicyAncestorStatus>,
}

#[cfg(test)]
#[path = "../../tests/unit/resources/backend_tls_policy_test.rs"]
mod tests;
