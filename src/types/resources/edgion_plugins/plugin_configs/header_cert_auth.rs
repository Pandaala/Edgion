//! Header Cert Auth plugin configuration.
//!
//! Supports two certificate source modes:
//! - Header: extract client certificate from HTTP header (for CDN/LB terminated TLS)
//! - Connection: reuse client certificate info from mTLS handshake context

use k8s_openapi::api::core::v1::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::resources::gateway::SecretObjectReference;

/// Certificate source mode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
pub enum CertSourceMode {
    /// Read certificate from configured HTTP header.
    #[default]
    Header,
    /// Read certificate metadata from mTLS connection context.
    Connection,
}

/// Certificate header encoding format (Header mode only).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CertHeaderFormat {
    /// Base64 body of PEM certificate without BEGIN/END delimiters.
    #[default]
    Base64Encoded,
    /// URL-encoded full PEM certificate.
    UrlEncoded,
}

/// Consumer identity mapping strategy.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConsumerBy {
    /// Use first SAN, fallback to CN.
    #[default]
    SanOrCn,
    /// Use CN only.
    Cn,
    /// Use certificate SHA-256 fingerprint.
    Fingerprint,
}

/// Upstream header names set after authentication.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamHeaderConfig {
    /// Header for resolved consumer identity.
    #[serde(default = "default_consumer_header")]
    pub consumer_header: String,
    /// Header for certificate subject DN.
    #[serde(default = "default_dn_header")]
    pub dn_header: String,
    /// Header for first SAN value.
    #[serde(default = "default_san_header")]
    pub san_header: String,
    /// Header for SHA-256 fingerprint.
    #[serde(default = "default_fingerprint_header")]
    pub fingerprint_header: String,
}

/// Header Cert Auth plugin configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeaderCertAuthConfig {
    // === Certificate source ===
    /// Certificate source mode.
    #[serde(default)]
    pub mode: CertSourceMode,

    // === Header mode ===
    /// Header name carrying client cert (Header mode only).
    #[serde(default = "default_cert_header_name")]
    pub certificate_header_name: String,
    /// Encoding format for certificate header value.
    #[serde(default)]
    pub certificate_header_format: CertHeaderFormat,
    /// Remove source credential header before forwarding upstream.
    #[serde(default = "default_true")]
    pub hide_credentials: bool,

    // === Header mode CA verification ===
    /// CA Secret references for Header mode certificate verification.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ca_secret_refs: Vec<SecretObjectReference>,
    /// Certificate verify depth.
    #[serde(default = "default_verify_depth")]
    pub verify_depth: u8,

    // === Identity mapping ===
    /// Skip consumer mapping and pass cert DN/SAN directly.
    #[serde(default)]
    pub skip_consumer_lookup: bool,
    /// Strategy for consumer identity resolution.
    #[serde(default)]
    pub consumer_by: ConsumerBy,
    /// Header names for upstream identity propagation.
    #[serde(default)]
    pub upstream_headers: UpstreamHeaderConfig,

    // === Failure handling ===
    /// Allow request as anonymous when authentication fails.
    #[serde(default)]
    pub allow_anonymous: bool,
    /// Authentication failure HTTP status code.
    #[serde(default = "default_error_status")]
    pub error_status: u16,
    /// Authentication failure message.
    #[serde(default = "default_error_message")]
    pub error_message: String,
    /// Optional delay before auth failure response (ms).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,

    // === Runtime fields (controller populated) ===
    /// Resolved CA secrets for Header mode verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_ca_secrets: Option<Vec<Secret>>,

    // === Validation fields ===
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_cert_header_name() -> String {
    "X-Client-Cert".to_string()
}
fn default_verify_depth() -> u8 {
    1
}
fn default_error_status() -> u16 {
    401
}
fn default_error_message() -> String {
    "TLS certificate failed verification".to_string()
}
fn default_consumer_header() -> String {
    "X-Consumer-Username".to_string()
}
fn default_dn_header() -> String {
    "X-Client-Cert-Dn".to_string()
}
fn default_san_header() -> String {
    "X-Client-Cert-San".to_string()
}
fn default_fingerprint_header() -> String {
    "X-Client-Cert-Fingerprint".to_string()
}

impl Default for UpstreamHeaderConfig {
    fn default() -> Self {
        Self {
            consumer_header: default_consumer_header(),
            dn_header: default_dn_header(),
            san_header: default_san_header(),
            fingerprint_header: default_fingerprint_header(),
        }
    }
}

impl Default for HeaderCertAuthConfig {
    fn default() -> Self {
        Self {
            mode: CertSourceMode::Header,
            certificate_header_name: default_cert_header_name(),
            certificate_header_format: CertHeaderFormat::Base64Encoded,
            hide_credentials: default_true(),
            ca_secret_refs: Vec::new(),
            verify_depth: default_verify_depth(),
            skip_consumer_lookup: false,
            consumer_by: ConsumerBy::SanOrCn,
            upstream_headers: UpstreamHeaderConfig::default(),
            allow_anonymous: false,
            error_status: default_error_status(),
            error_message: default_error_message(),
            auth_failure_delay_ms: 0,
            resolved_ca_secrets: None,
            validation_error: None,
        }
    }
}

impl HeaderCertAuthConfig {
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub fn detect_validation_error(&self) -> Option<String> {
        if self.verify_depth == 0 {
            return Some("verifyDepth must be >= 1".to_string());
        }
        if !(400..=599).contains(&self.error_status) {
            return Some("errorStatus must be within 400..=599".to_string());
        }
        if self.error_message.trim().is_empty() {
            return Some("errorMessage cannot be empty".to_string());
        }
        if self.certificate_header_name.trim().is_empty() {
            return Some("certificateHeaderName cannot be empty".to_string());
        }
        if self.upstream_headers.consumer_header.trim().is_empty()
            || self.upstream_headers.dn_header.trim().is_empty()
            || self.upstream_headers.san_header.trim().is_empty()
            || self.upstream_headers.fingerprint_header.trim().is_empty()
        {
            return Some("upstreamHeaders entries cannot be empty".to_string());
        }
        if self.mode == CertSourceMode::Header {
            let has_ca_refs = !self.ca_secret_refs.is_empty();
            let has_resolved = self.resolved_ca_secrets.as_ref().is_some_and(|v| !v.is_empty());
            if !has_ca_refs && !has_resolved {
                return Some("Header mode requires caSecretRefs (or resolvedCaSecrets)".to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = HeaderCertAuthConfig::default();
        assert_eq!(cfg.mode, CertSourceMode::Header);
        assert_eq!(cfg.certificate_header_name, "X-Client-Cert");
        assert_eq!(cfg.error_status, 401);
        assert!(cfg.hide_credentials);
    }

    #[test]
    fn test_detect_validation_error_requires_ca_for_header_mode() {
        let cfg = HeaderCertAuthConfig::default();
        let err = cfg.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("caSecretRefs"));
    }

    #[test]
    fn test_detect_validation_error_connection_mode_without_ca_ok() {
        let cfg = HeaderCertAuthConfig {
            mode: CertSourceMode::Connection,
            ..Default::default()
        };
        assert!(cfg.detect_validation_error().is_none());
    }
}
