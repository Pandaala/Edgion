//! BackendTLSPolicy resource definition
//!
//! BackendTLSPolicy provides a way to configure how a Gateway connects to a backend via TLS.

use super::common::PolicyAncestorStatus;

crate::redact::impl_redacted_debug!(BackendTLSPolicySpec);
use super::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind, ResolvedPolicyTargetClass};
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

impl BackendTLSPolicyTargetRef {
    /// Parse the accepted direct policy target group/kind pair.
    pub fn target_kind(&self) -> Result<PolicyTargetKind, String> {
        match (self.group.as_str(), self.kind.as_str()) {
            ("" | "core", "Service") => Ok(PolicyTargetKind::Service),
            ("edgion.io", "EdgionBackend") => Ok(PolicyTargetKind::EdgionBackend),
            _ => Err(format!(
                "targetRef {}/{} {}: supported targets are core-group Service or edgion.io/EdgionBackend",
                self.group, self.kind, self.name
            )),
        }
    }

    /// Resolve this same-namespace reference to the canonical policy identity.
    pub fn canonical_key(&self, namespace: &str) -> Result<PolicyTargetKey, String> {
        Ok(PolicyTargetKey {
            target_kind: self.target_kind()?,
            namespace: namespace.to_string(),
            name: self.name.clone(),
        })
    }
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

/// BackendTLSPolicyCACertificateRef identifies a ConfigMap or Secret containing a CA certificate bundle.
///
/// For BackendTLSPolicy, per Gateway API spec, refs must be in the same namespace as the policy;
/// `namespace` is ignored by the `DirectSameNs` fetch strategy. For non-BTP consumers (e.g.
/// EdgionGatewayConfig outbound TLS), an explicit `namespace` enables cross-namespace trust-anchor
/// references subject to a `ReferenceGrant` when the `GrantGated` fetch strategy is used.
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, PartialEq, Eq)]
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

    /// Namespace of the referent. When absent the controller infers the same namespace
    /// as the owning resource. For cluster-scoped resources (e.g. EdgionGatewayConfig)
    /// this field is required; refs without a namespace are skipped.
    /// Cross-namespace refs require a matching ReferenceGrant when the `GrantGated`
    /// fetch strategy is in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
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
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum WellKnownCACertificates {
    /// System CA certificates
    System,
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
    /// Parse implementation-specific client certificate Secret reference from options.
    pub fn client_certificate_secret_ref(&self) -> Result<Option<SecretObjectReference>, String> {
        parse_client_certificate_option(self.spec.options.as_ref())
    }

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
                current_status: None,
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: "".to_string(),
                    kind: "Service".to_string(),
                    name: "svc".to_string(),
                    section_name: None,
                }],
                resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: "backend.example.com".to_string(),
                    subject_alt_names: None,
                    well_known_ca_certificates: None,
                },
                options,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: false,
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

    #[test]
    fn test_client_certificate_secret_ref_rejects_empty_and_invalid_names() {
        for value in ["", "   ", "Invalid_Name", "-leading", "trailing-"] {
            let policy = make_policy(Some(HashMap::from([(
                OPTION_CLIENT_CERTIFICATE_REF.to_string(),
                value.to_string(),
            )])));
            assert!(policy.client_certificate_secret_ref().is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn backend_tls_runtime_identity_validation_covers_hostname_and_typed_sans() {
        let mut validation = BackendTLSPolicyValidation {
            ca_certificate_refs: None,
            hostname: "backend.example.com".to_string(),
            subject_alt_names: Some(vec![
                SubjectAltName {
                    san_type: SubjectAltNameType::Hostname,
                    hostname: Some("*.example.com".to_string()),
                    uri: None,
                },
                SubjectAltName {
                    san_type: SubjectAltNameType::URI,
                    hostname: None,
                    uri: Some("spiffe://trust.example/workload".to_string()),
                },
            ]),
            well_known_ca_certificates: Some(WellKnownCACertificates::System),
        };
        assert!(backend_tls_identity_validation_errors(&validation).is_empty());

        validation.hostname.clear();
        assert!(!backend_tls_identity_validation_errors(&validation).is_empty());
        validation.hostname = "backend.example.com".to_string();

        for sans in [
            Vec::new(),
            vec![SubjectAltName {
                san_type: SubjectAltNameType::Hostname,
                hostname: None,
                uri: None,
            }],
            vec![SubjectAltName {
                san_type: SubjectAltNameType::Hostname,
                hostname: Some("backend.example.com".to_string()),
                uri: Some("spiffe://trust.example/workload".to_string()),
            }],
            vec![SubjectAltName {
                san_type: SubjectAltNameType::URI,
                hostname: None,
                uri: Some("not an absolute uri".to_string()),
            }],
            vec![SubjectAltName {
                san_type: SubjectAltNameType::URI,
                hostname: None,
                uri: Some(format!("https://example.com/{}", "a".repeat(240))),
            }],
            (0..6)
                .map(|index| SubjectAltName {
                    san_type: SubjectAltNameType::Hostname,
                    hostname: Some(format!("backend-{index}.example.com")),
                    uri: None,
                })
                .collect(),
        ] {
            validation.subject_alt_names = Some(sans);
            assert!(!backend_tls_identity_validation_errors(&validation).is_empty());
        }
    }

    #[test]
    fn test_well_known_ca_certificates_system_serialization() {
        let validation = BackendTLSPolicyValidation {
            ca_certificate_refs: None,
            hostname: "backend.example.com".to_string(),
            subject_alt_names: None,
            well_known_ca_certificates: Some(WellKnownCACertificates::System),
        };

        let json = serde_json::to_value(&validation).unwrap();
        assert_eq!(json["wellKnownCACertificates"], "System");
        assert!(json.get("caCertificateRefs").is_none());
    }

    #[test]
    fn test_well_known_ca_certificates_deserialization() {
        let json = serde_json::json!({
            "hostname": "backend.example.com",
            "wellKnownCACertificates": "System"
        });

        let validation: BackendTLSPolicyValidation = serde_json::from_value(json).unwrap();
        assert_eq!(
            validation.well_known_ca_certificates,
            Some(WellKnownCACertificates::System)
        );
        assert!(validation.ca_certificate_refs.is_none());
    }

    #[test]
    fn test_use_system_ca_default_false() {
        let policy = make_policy(None);
        assert!(!policy.spec.use_system_ca);
    }

    #[test]
    fn test_use_system_ca_skip_serialization_when_false() {
        let policy = make_policy(None);
        let json = serde_json::to_value(&policy.spec).unwrap();
        assert!(json.get("useSystemCa").is_none());
    }

    #[test]
    fn canonical_target_identity_is_typed() {
        let service = BackendTLSPolicyTargetRef {
            group: "core".to_string(),
            kind: "Service".to_string(),
            name: "shared".to_string(),
            section_name: Some("https".to_string()),
        };
        let backend = BackendTLSPolicyTargetRef {
            group: "edgion.io".to_string(),
            kind: "EdgionBackend".to_string(),
            name: "shared".to_string(),
            section_name: None,
        };

        assert_eq!(
            service.canonical_key("tenant").unwrap().target_kind,
            PolicyTargetKind::Service
        );
        assert_eq!(
            backend.canonical_key("tenant").unwrap().target_kind,
            PolicyTargetKind::EdgionBackend
        );
        assert_ne!(
            service.canonical_key("tenant").unwrap(),
            backend.canonical_key("tenant").unwrap()
        );
    }

    #[test]
    fn resolved_target_class_is_transport_only() {
        let policy = make_policy(None);
        let json = serde_json::to_value(&policy.spec).unwrap();
        assert_eq!(json["resolvedTargetClass"], "service");

        let schema = schemars::schema_for!(BackendTLSPolicySpec);
        let schema_json = serde_json::to_value(schema).unwrap();
        assert!(!schema_json.to_string().contains("resolvedTargetClass"));
    }
}
