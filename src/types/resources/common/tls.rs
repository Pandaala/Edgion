//! Common TLS configuration types for `LinkSys` resources.
//!
//! Unifies what used to be four near-identical copies (`RedisTls` / `EtcdTls` /
//! `EsTls` / `WebhookTls`) into a single pair:
//!
//! - [`TlsConfig`] — on/off switch, certs sub-object, `insecureSkipVerify`.
//! - [`TlsCerts`] — CA + client cert + `secretRef` (+ resolved `Secret`).
//!
//! Design decisions (see `tasks/fix-change/R1-unify-tls-types.md`):
//!
//! - Inline private key (`clientKey`) is **not allowed** — `deny_unknown_fields`
//!   rejects it at YAML parse time, per D2 ("no inline sensitive material").
//!   Client private key MUST be provided through `secretRef` pointing at a
//!   `kubernetes.io/tls` or `Opaque` Secret (key: `tls.key`).
//! - `ca_cert` / `client_cert` inline PEM is still allowed (public material).
//! - `secret` mirrors the `auth.secret` pattern: filled by the Controller after
//!   resolving `secret_ref`, serialized to the Gateway over conf_sync, but
//!   omitted from YAML/CRD schema via `skip_serializing_if = "Option::is_none"`.
//!
//! TLS material is parsed on demand by the Gateway using
//! `crate::types::credentials::resolvers::tls::TlsCertsResolver`.
//!
//! # Zeroize scope (transport)
//!
//! [`TlsCerts::secret`] carries the resolved K8s `Secret` (including the
//! `tls.key` private key) from controller to Gateway via `serde_json` + gRPC
//! through conf_sync; those buffers and the Gateway-side runtime store cache
//! are **not** zero-wiped. Only the `TlsMaterial.client_key_pem` parsed out
//! of this Secret on the Gateway side uses `Zeroizing`. See
//! [`crate::types::credentials::TlsMaterial`] for the full caveat list.

use k8s_openapi::api::core::v1::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SecretReference;

/// TLS configuration shared by all LinkSys backend connections.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TlsConfig {
    /// Enable TLS for the backend connection.
    ///
    /// For providers where the endpoint URL already encodes TLS (e.g. `https://`
    /// or `rediss://`), this flag additionally gates custom CA / client-cert
    /// handling; the transport itself is still driven by the URL scheme.
    #[serde(default)]
    pub enabled: bool,

    /// Certificate material (CA + optional client mTLS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certs: Option<TlsCerts>,

    /// Skip server certificate verification.
    ///
    /// **Dangerous** — only for development / testing. Not every provider
    /// supports this (e.g. `etcd-client` ignores it and logs a warning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure_skip_verify: Option<bool>,
}

/// TLS certificate material.
///
/// CA and client certificate may be provided inline (PEM string, public material)
/// or via `secret_ref`. The client **private key** is only accepted via
/// `secret_ref` — `deny_unknown_fields` rejects any `clientKey` inline value.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TlsCerts {
    /// CA certificate (PEM format). Public material, inline allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<String>,

    /// Client certificate (PEM format) for mTLS. Public material, inline allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,

    /// Reference to a Kubernetes Secret carrying TLS material.
    ///
    /// Recommended secret type: `kubernetes.io/tls` with keys `tls.crt` /
    /// `tls.key`; optional `ca.crt`. `Opaque` is also accepted for compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretReference>,

    /// Resolved Secret object (filled by Controller, never from user YAML).
    ///
    /// Serialized to the Gateway via conf_sync. Not persisted back into the K8s
    /// `spec` (the controller only patches `status`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<Secret>,
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
        assert!(cfg.certs.is_none());
    }

    #[test]
    fn tls_certs_with_ca_and_secret_ref_parses() {
        let yaml = r#"
caCert: "-----BEGIN CERTIFICATE-----\nMIIB..."
secretRef:
  name: my-tls
  namespace: default
"#;
        let certs: TlsCerts = serde_yaml::from_str(yaml).expect("should parse");
        assert!(certs.ca_cert.is_some());
        assert_eq!(certs.secret_ref.as_ref().unwrap().name, "my-tls");
    }

    #[test]
    fn tls_certs_rejects_inline_client_key() {
        let yaml = r#"
clientCert: "-----BEGIN CERTIFICATE-----"
clientKey: "-----BEGIN PRIVATE KEY-----"
"#;
        let r: Result<TlsCerts, _> = serde_yaml::from_str(yaml);
        assert!(r.is_err(), "inline clientKey 必须被 deny_unknown_fields 拒绝");
    }

    #[test]
    fn tls_certs_secret_field_round_trips_through_json() {
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        let mut data = BTreeMap::new();
        data.insert("tls.crt".to_string(), ByteString(b"CERT-BYTES".to_vec()));
        data.insert("tls.key".to_string(), ByteString(b"KEY-BYTES".to_vec()));

        let certs = TlsCerts {
            ca_cert: None,
            client_cert: None,
            secret_ref: Some(SecretReference {
                name: "my-tls".to_string(),
                namespace: None,
            }),
            secret: Some(Secret {
                data: Some(data),
                type_: Some("kubernetes.io/tls".to_string()),
                ..Default::default()
            }),
        };

        let json = serde_json::to_string(&certs).expect("serialize");
        let parsed: TlsCerts = serde_json::from_str(&json).expect("deserialize");
        let data = parsed.secret.expect("secret").data.expect("data");
        assert_eq!(data.get("tls.crt").unwrap().0, b"CERT-BYTES");
        assert_eq!(data.get("tls.key").unwrap().0, b"KEY-BYTES");
    }

    #[test]
    fn tls_certs_secret_is_omitted_from_yaml_when_none() {
        let certs = TlsCerts {
            ca_cert: Some("CA".to_string()),
            client_cert: None,
            secret_ref: None,
            secret: None,
        };
        let yaml = serde_yaml::to_string(&certs).expect("serialize");
        assert!(!yaml.contains("secret:"), "secret=None 时不应出现在 YAML 中: {yaml}");
    }
}
