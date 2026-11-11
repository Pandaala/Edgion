//! Shared outbound (plugin / LinkSys) TLS verification schema.
//!
//! `verify` is positive and tri-state at the local layer (None = inherit the
//! global `outboundTls.verify`).
//!
//! CA trust sources are configured via the `validation` block, which supports:
//! - `caCertificateRefs`: references to Secret/ConfigMap resources holding CA bundles
//! - `wellKnownCACertificates`: use the process OS CA bundle (`edgion_common::system_trust`)
//!
//! mTLS client identity is specified via `clientCertificateRef` (kubernetes.io/tls Secret).

use k8s_openapi::api::core::v1::Secret;

crate::redact::impl_redacted_debug!(OutboundTlsConfig);
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SecretObjectReference;
use crate::resources::backend_tls_policy::{SubjectAltName, WellKnownCACertificates};

/// CA material reference for the finite custom outbound TLS fields.
///
/// Group omission denotes the core group. Owning field adapters decide the
/// allowed target kinds, namespace policy and lowercase `core` convenience.
/// Kind and name remain required; Secret and ConfigMap are not inferred.
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CustomCACertificateRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 253), pattern(crate::gwapi_types::REFERENCE_GROUP_PATTERN))]
    pub group: Option<String>,
    #[schemars(length(min = 1, max = 63), pattern(crate::gwapi_types::REFERENCE_KIND_PATTERN))]
    pub kind: String,
    #[schemars(length(min = 1, max = 253))]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 63), pattern(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$"))]
    pub namespace: Option<String>,
}

/// Returns `true`; used as `#[serde(default = "default_verify_true")]`.
pub fn default_verify_true() -> bool {
    true
}

/// Outbound TLS verification config — reuses the BackendTLSPolicy validation
/// shape (leaf types), but `hostname` is optional (outbound has an endpoint host).
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutboundTlsValidation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_certificate_refs: Option<Vec<CustomCACertificateRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "wellKnownCACertificates")]
    pub well_known_ca_certificates: Option<WellKnownCACertificates>,
    /// Optional SNI override (default: endpoint host) + default SAN to check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
        pattern(crate::gwapi_types::PRECISE_HOSTNAME_PATTERN)
    )]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_alt_names: Option<Vec<SubjectAltName>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutboundTlsLocal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<OutboundTlsValidation>,
    /// Top-level outbound mTLS identity (kubernetes.io/tls Secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_certificate_ref: Option<SecretObjectReference>,
}

/// Cluster-wide outbound TLS default carried on EdgionGatewayConfig.
#[derive(Deserialize, Serialize, Clone, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OutboundTlsConfig {
    /// Global default verify; defaults to true when the field/block is absent.
    #[serde(default = "default_verify_true")]
    pub verify: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<OutboundTlsValidation>,
    /// Top-level outbound mTLS identity (kubernetes.io/tls Secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_certificate_ref: Option<SecretObjectReference>,
    /// Resolved cluster-wide CA Secrets (filled by the Controller from
    /// `validation.caCertificateRefs`, shipped via conf_sync). Out of the CRD schema; redacted
    /// in admin output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_ca_certificates: Option<Vec<Secret>>,
    /// Resolved client-identity Secret (filled by the Controller from
    /// `clientCertificateRef`, shipped via conf_sync). Out of the CRD schema; redacted
    /// in admin output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_client_certificate: Option<Secret>,
}

impl Default for OutboundTlsConfig {
    fn default() -> Self {
        Self {
            verify: true,
            validation: None,
            client_certificate_ref: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/outbound_tls_test.rs"]
mod tests;
