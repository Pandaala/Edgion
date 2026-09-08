//! Common TLS configuration for `LinkSys` backend connections.
//!
//! - [`TlsConfig`] — on/off switch plus the unified outbound verify / CA
//!   trust-source block (flattened from [`super::outbound_tls::OutboundTlsLocal`]).
//!
//! Design decisions:
//!
//! - The `verify`, `validation`, and `clientCertificateRef` fields appear **at the top level**
//!   of `TlsConfig` (flattened from `OutboundTlsLocal`), not inside a nested `certs` sub-object.
//! - CA trust sources are configured via the `validation` block (supports `caCertificateRefs`
//!   and `wellKnownCACertificates`). mTLS client identity via `clientCertificateRef`.
//! - Inline private keys are not accepted; `deny_unknown_fields` rejects any
//!   unrecognized field at YAML/JSON parse time ("no inline sensitive material").
//!   Client private key MUST be provided through `clientCertificateRef` pointing at a
//!   `kubernetes.io/tls` Secret (`tls.key`).
//! - `resolved_ca_certificates` and `resolved_client_certificate` are filled by the
//!   Controller after resolving the respective resource references, shipped to the
//!   Gateway via conf_sync, but omitted from YAML/CRD schema via `#[schemars(skip)]`
//!   and `skip_serializing_if`.
//!
//! TLS material is parsed on demand by the Gateway using
//! `crate::credentials::resolvers::tls::TlsCertsResolver`.
//!
//! # Zeroize scope (transport)
//!
//! The resolved K8s `Secret` values (including `tls.key` private key) are
//! carried from controller to Gateway via `serde_json` + gRPC through conf_sync;
//! those buffers and the Gateway-side runtime store cache are **not** zero-wiped.
//! Only the `TlsMaterial.client_key_pem` parsed out of these secrets on the
//! Gateway side uses `Zeroizing`. See [`crate::credentials::TlsMaterial`] for
//! the full caveat list.

use k8s_openapi::api::core::v1::Secret;

crate::redact::impl_redacted_debug!(TlsConfig);
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::outbound_tls::OutboundTlsLocal;

/// TLS configuration shared by all LinkSys backend connections.
#[derive(Deserialize, Serialize, Clone, Default, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TlsConfig {
    /// Enable TLS for the backend connection.
    ///
    /// For providers where the endpoint URL already encodes TLS (e.g. `https://`
    /// or `rediss://`), this flag additionally gates custom CA / client-cert
    /// handling; the transport itself is still driven by the URL scheme.
    #[serde(default)]
    pub enabled: bool,

    /// Unified outbound verify + CA trust source (flattened: verify, validation,
    /// clientCertificateRef).
    #[serde(flatten)]
    pub outbound: OutboundTlsLocal,

    /// Resolved CA Secrets (filled by Controller, shipped via conf_sync).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_ca_certificates: Option<Vec<Secret>>,

    /// Resolved client-identity Secret (filled by Controller).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_client_certificate: Option<Secret>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_config_minimal_yaml_parses() {
        let yaml = "enabled: true";
        let cfg: TlsConfig = serde_yaml::from_str(yaml).expect("should parse");
        assert!(cfg.enabled);
        assert!(cfg.outbound.verify.is_none());
    }

    #[test]
    fn tls_config_accepts_validation_and_client_ref() {
        let c: TlsConfig = serde_json::from_str(
            r#"{"enabled":true,"verify":false,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},"clientCertificateRef":{"name":"id"}}"#,
        ).unwrap();
        assert_eq!(c.outbound.verify, Some(false));
        assert!(c.outbound.validation.unwrap().ca_certificate_refs.is_some());
        assert_eq!(c.outbound.client_certificate_ref.unwrap().name, "id");
    }

    #[test]
    fn tls_config_rejects_legacy_outbound_keys() {
        // Flatten + deny may interact unpredictably; verify the LinkSys TlsConfig
        // path also rejects legacy outbound keys (otherwise it's a silent no-op
        // for redis/etcd/es operators).
        for legacy in [
            r#"{"enabled":true,"caSource":"SecretRef"}"#,
            r#"{"enabled":true,"caCert":"---pem---"}"#,
            r#"{"enabled":true,"caSecretRef":{"name":"ca"}}"#,
            r#"{"enabled":true,"clientSecretRef":{"name":"id"}}"#,
        ] {
            let r: Result<TlsConfig, _> = serde_json::from_str(legacy);
            assert!(r.is_err(), "legacy key in {} should be rejected, got: {:?}", legacy, r);
        }
    }
}
