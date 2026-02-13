//! JWE Decrypt plugin configuration.
//!
//! Phase 1 supports compact JWE with:
//! - Key management: `dir`
//! - Content encryption: `A256GCM`

use std::collections::HashMap;

use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::resources::gateway::SecretObjectReference;

/// JWE content encryption algorithm.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub enum JweContentEncryption {
    /// AES GCM using 256-bit key.
    #[default]
    A256GCM,
}

impl JweContentEncryption {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::A256GCM => "A256GCM",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "A256GCM" => Some(Self::A256GCM),
            _ => None,
        }
    }

    pub fn required_key_len(&self) -> usize {
        match self {
            Self::A256GCM => 32,
        }
    }
}

/// JWE key management algorithm.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
pub enum JweKeyManagement {
    /// Direct use of symmetric key as content encryption key.
    #[default]
    Dir,
}

impl JweKeyManagement {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dir => "dir",
        }
    }
}

/// Resolved JWE credential from Secret (populated by controller).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedJweCredential {
    /// Symmetric key, base64-encoded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// JWE Decrypt plugin configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JweDecryptConfig {
    /// Reference to K8s Secret containing decryption key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretObjectReference>,

    /// Key management algorithm. Phase 1 supports only `Dir`.
    #[serde(default)]
    pub key_management_algorithm: JweKeyManagement,

    /// Content encryption algorithm. Phase 1 supports only `A256GCM`.
    #[serde(default)]
    pub content_encryption_algorithm: JweContentEncryption,

    /// Request header name to read JWE token from.
    #[serde(default = "default_header")]
    pub header: String,

    /// Request header name to forward decrypted plaintext to upstream.
    #[serde(default = "default_forward_header")]
    pub forward_header: String,

    /// Optional prefix stripped before parsing JWE token (e.g. `"Bearer "`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strip_prefix: Option<String>,

    /// If true, reject request when token is missing.
    #[serde(default = "default_true")]
    pub strict: bool,

    /// If true, remove original credential header before proxying upstream.
    #[serde(default)]
    pub hide_credentials: bool,

    /// If true, decode Secret value as base64 once more.
    #[serde(default)]
    pub base64_secret: bool,

    /// Max accepted token size.
    #[serde(default = "default_max_token_size")]
    pub max_token_size: usize,

    /// Allowed content encryption algorithms in JWE protected header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_algorithms: Option<Vec<JweContentEncryption>>,

    /// Delay in milliseconds before auth failure response.
    #[serde(default)]
    pub auth_failure_delay_ms: u64,

    /// Store decrypted payload into `jwe_payload` context variable.
    #[serde(default)]
    pub store_payload_in_ctx: bool,

    /// Map JSON payload fields to upstream request headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_to_headers: Option<HashMap<String, String>>,

    /// Max bytes for a single mapped header value.
    #[serde(default = "default_max_header_value_bytes")]
    pub max_header_value_bytes: usize,

    /// Max bytes for all mapped headers.
    #[serde(default = "default_max_total_header_bytes")]
    pub max_total_header_bytes: usize,

    // === Runtime fields (controller populated) ===
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_credential: Option<ResolvedJweCredential>,

    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

fn default_header() -> String {
    "authorization".to_string()
}

fn default_forward_header() -> String {
    "authorization".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_token_size() -> usize {
    8192
}

fn default_max_header_value_bytes() -> usize {
    4096
}

fn default_max_total_header_bytes() -> usize {
    16384
}

impl Default for JweDecryptConfig {
    fn default() -> Self {
        Self {
            secret_ref: None,
            key_management_algorithm: JweKeyManagement::default(),
            content_encryption_algorithm: JweContentEncryption::default(),
            header: default_header(),
            forward_header: default_forward_header(),
            strip_prefix: None,
            strict: default_true(),
            hide_credentials: false,
            base64_secret: false,
            max_token_size: default_max_token_size(),
            allowed_algorithms: None,
            auth_failure_delay_ms: 0,
            store_payload_in_ctx: false,
            payload_to_headers: None,
            max_header_value_bytes: default_max_header_value_bytes(),
            max_total_header_bytes: default_max_total_header_bytes(),
            resolved_credential: None,
            validation_error: None,
        }
    }
}

impl JweDecryptConfig {
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub fn detect_validation_error(&self) -> Option<String> {
        if self.secret_ref.is_none() && self.resolved_credential.is_none() {
            return Some("secretRef is required".to_string());
        }
        if self.header.trim().is_empty() {
            return Some("header cannot be empty".to_string());
        }
        if self.forward_header.trim().is_empty() {
            return Some("forwardHeader cannot be empty".to_string());
        }
        if self.max_token_size == 0 {
            return Some("maxTokenSize must be > 0".to_string());
        }
        if self.max_header_value_bytes == 0 {
            return Some("maxHeaderValueBytes must be > 0".to_string());
        }
        if self.max_total_header_bytes == 0 {
            return Some("maxTotalHeaderBytes must be > 0".to_string());
        }
        if self.max_total_header_bytes < self.max_header_value_bytes {
            return Some("maxTotalHeaderBytes must be >= maxHeaderValueBytes".to_string());
        }
        if let Some(allowed) = &self.allowed_algorithms {
            if allowed.is_empty() {
                return Some("allowedAlgorithms cannot be empty".to_string());
            }
            if !allowed.contains(&self.content_encryption_algorithm) {
                return Some("allowedAlgorithms must include contentEncryptionAlgorithm".to_string());
            }
        }
        if let Some(resolved) = &self.resolved_credential {
            if let Some(secret_b64) = &resolved.secret {
                let decoded = match base64::engine::general_purpose::STANDARD.decode(secret_b64) {
                    Ok(v) => v,
                    Err(e) => return Some(format!("resolvedCredential.secret is not valid base64: {}", e)),
                };
                let final_secret = if self.base64_secret {
                    let s = match String::from_utf8(decoded) {
                        Ok(v) => v,
                        Err(e) => {
                            return Some(format!(
                                "resolvedCredential.secret(base64-decoded) is not UTF-8 for base64Secret mode: {}",
                                e
                            ));
                        }
                    };
                    match base64::engine::general_purpose::STANDARD.decode(s.trim()) {
                        Ok(v) => v,
                        Err(e) => {
                            return Some(format!(
                                "resolvedCredential.secret (base64Secret mode) is not valid base64: {}",
                                e
                            ));
                        }
                    }
                } else {
                    decoded
                };

                let required = self.content_encryption_algorithm.required_key_len();
                if final_secret.len() != required {
                    return Some(format!(
                        "resolvedCredential.secret decoded length must be {} bytes for {} (got {})",
                        required,
                        self.content_encryption_algorithm.as_str(),
                        final_secret.len()
                    ));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn test_detect_validation_error_requires_secret_ref_or_resolved() {
        let cfg = JweDecryptConfig::default();
        let err = cfg.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("secretRef is required"));
    }

    #[test]
    fn test_detect_validation_error_allowed_algorithms_empty() {
        let mut cfg = JweDecryptConfig::default();
        cfg.secret_ref = Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "jwe-secret".to_string(),
            namespace: None,
        });
        cfg.allowed_algorithms = Some(vec![]);
        let err = cfg.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("allowedAlgorithms cannot be empty"));
    }

    #[test]
    fn test_detect_validation_error_key_len_mismatch() {
        let mut cfg = JweDecryptConfig::default();
        cfg.resolved_credential = Some(ResolvedJweCredential {
            secret: Some(base64::engine::general_purpose::STANDARD.encode("short-key")),
        });
        let err = cfg.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("decoded length must be 32 bytes"));
    }

    #[test]
    fn test_detect_validation_error_valid_resolved_secret() {
        let mut cfg = JweDecryptConfig::default();
        cfg.resolved_credential = Some(ResolvedJweCredential {
            secret: Some(base64::engine::general_purpose::STANDARD.encode("0123456789abcdef0123456789abcdef")),
        });
        let err = cfg.detect_validation_error();
        assert!(err.is_none());
    }
}
