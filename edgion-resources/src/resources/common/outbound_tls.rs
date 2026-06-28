//! Shared outbound (plugin / LinkSys) TLS verification + CA trust-source schema.
//!
//! `verify` is positive and tri-state at the local layer (None = inherit the
//! global `outboundTls.verify`). `caSource` is a plain string discriminator so
//! the structural CRD coverage guard keeps working (no data-carrying enum).

use k8s_openapi::api::core::v1::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SecretObjectReference;

/// Where the trust anchors for outbound TLS verification come from.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema)]
pub enum CaSource {
    /// Built-in webpki roots compiled into the binary.
    System,
    /// A Kubernetes Secret holding a private CA bundle (REPLACES public roots).
    SecretRef,
    /// Inline PEM CA bundle (public material; REPLACES public roots).
    Inline,
}

/// Returns `true`; used as `#[serde(default = "default_verify_true")]`.
pub fn default_verify_true() -> bool {
    true
}

/// Per-call outbound TLS override. All-None = inherit the global default.
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutboundTlsLocal {
    /// None -> inherit global outboundTls.verify; Some -> override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<bool>,
    /// None -> inherit global caSource and its CA material.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_source: Option<CaSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_secret_ref: Option<SecretObjectReference>,
    /// Inline CA PEM (public material) used when caSource == Inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<String>,
    /// Optional client mTLS Secret (kubernetes.io/tls: tls.crt + tls.key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_ref: Option<SecretObjectReference>,
}

/// Cluster-wide outbound TLS default carried on EdgionGatewayConfig.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutboundTlsConfig {
    /// Global default verify; defaults to true when the field/block is absent.
    #[serde(default = "default_verify_true")]
    pub verify: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_source: Option<CaSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_secret_ref: Option<SecretObjectReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<String>,
    /// Resolved cluster-wide CA Secret (filled by the Controller from
    /// `ca_secret_ref`, shipped via conf_sync). Out of the CRD schema; redacted
    /// in admin output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_ca_secret: Option<Secret>,
}

impl Default for OutboundTlsConfig {
    fn default() -> Self {
        Self {
            verify: true,
            ca_source: None,
            ca_secret_ref: None,
            ca_cert: None,
            resolved_ca_secret: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_source_serializes_as_plain_string() {
        let v = serde_json::to_value(CaSource::SecretRef).unwrap();
        assert_eq!(v, serde_json::json!("SecretRef"));
    }

    #[test]
    fn local_defaults_are_all_none() {
        let l: OutboundTlsLocal = serde_json::from_str("{}").unwrap();
        assert!(l.verify.is_none() && l.ca_source.is_none() && l.client_secret_ref.is_none());
    }

    #[test]
    fn global_verify_defaults_true_when_absent() {
        let g: OutboundTlsConfig = serde_json::from_str("{}").unwrap();
        assert!(g.verify);
    }

    /// `resolved_ca_secret` is a controller-resolved field: it must stay out of
    /// the JSON schema (`#[schemars(skip)]`) so the structural CRD coverage guard
    /// keeps 6/6.
    #[test]
    fn resolved_ca_secret_is_absent_from_schema() {
        let schema = serde_json::to_value(schemars::schema_for!(OutboundTlsConfig)).unwrap();
        let props = &schema["properties"];
        assert!(
            props.get("resolvedCaSecret").is_none() && props.get("resolved_ca_secret").is_none(),
            "resolved_ca_secret must not appear in the schema, got: {props}"
        );
    }

    /// In an admin-redact scope the resolved Secret serializes as `[redacted]`,
    /// while conf_sync (no guard) still ships the real value.
    #[test]
    fn resolved_ca_secret_redacted_in_admin_output() {
        let cfg = OutboundTlsConfig {
            resolved_ca_secret: Some(k8s_openapi::api::core::v1::Secret {
                type_: Some("kubernetes.io/tls".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let admin = {
            let _g = crate::redact::AdminRedactGuard::new();
            serde_json::to_value(&cfg).unwrap()
        };
        assert_eq!(admin["resolvedCaSecret"], "[redacted]");

        let sync = serde_json::to_value(&cfg).unwrap();
        assert!(
            sync["resolvedCaSecret"].is_object(),
            "conf_sync must ship the real Secret object, got: {}",
            sync["resolvedCaSecret"]
        );
    }
}
