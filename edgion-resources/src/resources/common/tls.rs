//! Common TLS configuration for `LinkSys` backend connections.
//!
//! - [`TlsConfig`] — on/off switch plus the unified outbound verify / CA
//!   trust-source block (flattened from [`super::outbound_tls::OutboundTlsLocal`]).
//!
//! Design decisions:
//!
//! - The `verify` / `caSource` / `caSecretRef` / `caCert` / `clientSecretRef`
//!   fields appear **at the top level** of `TlsConfig` (flattened from
//!   `OutboundTlsLocal`), not inside a nested `certs` sub-object.
//! - Inline private keys are not accepted; `deny_unknown_fields` rejects any
//!   unrecognized field at YAML/JSON parse time ("no inline sensitive material").
//!   Client private key MUST be provided through `clientSecretRef` pointing at a
//!   `kubernetes.io/tls` Secret (`tls.key`).
//! - Inline CA PEM (`caCert`) is still allowed (public material).
//! - `resolved_ca_secret` and `resolved_client_secret` are filled by the
//!   Controller after resolving the respective `*SecretRef`, shipped to the
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
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::outbound_tls::OutboundTlsLocal;

/// TLS configuration shared by all LinkSys backend connections.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TlsConfig {
    /// Enable TLS for the backend connection.
    ///
    /// For providers where the endpoint URL already encodes TLS (e.g. `https://`
    /// or `rediss://`), this flag additionally gates custom CA / client-cert
    /// handling; the transport itself is still driven by the URL scheme.
    #[serde(default)]
    pub enabled: bool,

    /// Unified outbound verify + CA trust source (flattened: verify, caSource,
    /// caSecretRef, caCert, clientSecretRef).
    #[serde(flatten)]
    pub outbound: OutboundTlsLocal,

    /// Resolved CA Secret (filled by Controller, shipped via conf_sync).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_ca_secret: Option<Secret>,

    /// Resolved client-identity Secret (filled by Controller).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_client_secret: Option<Secret>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::super::outbound_tls::CaSource;
    use super::*;

    #[test]
    fn tls_config_minimal_yaml_parses() {
        let yaml = "enabled: true";
        let cfg: TlsConfig = serde_yaml::from_str(yaml).expect("should parse");
        assert!(cfg.enabled);
        assert!(cfg.outbound.verify.is_none());
    }

    #[test]
    fn tls_config_rejects_legacy_insecure_skip_verify() {
        // deny_unknown_fields must reject the deleted key.
        let r: Result<TlsConfig, _> = serde_json::from_str(r#"{"enabled":true,"insecureSkipVerify":true}"#);
        assert!(r.is_err(), "legacy insecureSkipVerify must be rejected");
    }

    #[test]
    fn tls_config_accepts_unified_verify_and_casource() {
        let c: TlsConfig = serde_json::from_str(
            r#"{"enabled":true,"verify":false,"caSource":"SecretRef","caSecretRef":{"name":"ca"}}"#,
        )
        .unwrap();
        assert_eq!(c.outbound.verify, Some(false));
        assert_eq!(c.outbound.ca_source, Some(CaSource::SecretRef));
    }
}
