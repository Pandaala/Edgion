//! JWE Decrypt plugin configuration.
//!
//! Phase 1 supports compact JWE with:
//! - Key management: `dir`
//! - Content encryption: `A256GCM`

use std::collections::HashMap;

crate::redact::impl_redacted_debug!(ResolvedJweCredential, JweDecryptConfig);

use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::resources::gateway::SecretObjectReference;

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
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedJweCredential {
    /// Symmetric key, base64-encoded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// Why Controller-resolved JWE key material cannot build a decryptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedJweCredentialError {
    Missing,
    TransportDecodeFailed,
    SecretDecodeFailed,
    InvalidLength { required: usize, actual: usize },
}

impl ResolvedJweCredentialError {
    pub fn status_detail(self) -> String {
        match self {
            Self::Missing => "required Secret data key is missing".to_string(),
            Self::TransportDecodeFailed => "resolved credential transport encoding is invalid".to_string(),
            Self::SecretDecodeFailed => "Secret value is not valid base64 for base64Secret mode".to_string(),
            Self::InvalidLength { required, actual } => {
                format!("decoded key must be {required} bytes (got {actual})")
            }
        }
    }
}

/// JWE Decrypt plugin configuration.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
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
    #[schemars(range(min = 1))]
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

    /// Map JSON payload fields to upstream request headers. Destination header
    /// names are case-insensitive and must be unique within this plugin instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_to_headers: Option<HashMap<String, String>>,

    /// Max bytes for a single mapped header value.
    #[serde(default = "default_max_header_value_bytes")]
    #[schemars(range(min = 1))]
    pub max_header_value_bytes: usize,

    /// Max bytes for all mapped headers.
    #[serde(default = "default_max_total_header_bytes")]
    #[schemars(range(min = 1))]
    pub max_total_header_bytes: usize,

    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_credential: Option<ResolvedJweCredential>,
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
        }
    }
}

impl JweDecryptConfig {
    pub fn compile_claim_header_projection(&self) -> Result<super::ClaimHeaderProjectionPlan, &'static str> {
        super::compile_claim_header_projection_with_owned_destinations(
            self.payload_to_headers.as_ref(),
            &[self.forward_header.as_str()],
        )
    }

    pub fn detect_validation_error(&self) -> Option<String> {
        if self.secret_ref.is_none() {
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
        if let Err(error) = self.compile_claim_header_projection() {
            return Some(error.to_string());
        }
        if let Some(allowed) = &self.allowed_algorithms {
            if allowed.is_empty() {
                return Some("allowedAlgorithms cannot be empty".to_string());
            }
            if !allowed.contains(&self.content_encryption_algorithm) {
                return Some("allowedAlgorithms must include contentEncryptionAlgorithm".to_string());
            }
        }
        None
    }

    /// Decode Controller-owned transport state into one generation's JWE key.
    ///
    /// This is intentionally separate from operator-facing configuration
    /// validation. A malformed resolved Secret must build a fail-closed auth
    /// runtime and surface through `ResolvedRefs`, not seal the plugin as an
    /// invalid configuration.
    pub fn resolved_key_material(&self) -> Result<Zeroizing<Vec<u8>>, ResolvedJweCredentialError> {
        let encoded = self
            .resolved_credential
            .as_ref()
            .and_then(|credential| credential.secret.as_deref())
            .ok_or(ResolvedJweCredentialError::Missing)?;
        let transport_decoded = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| ResolvedJweCredentialError::TransportDecodeFailed)?,
        );
        let key = if self.base64_secret {
            let encoded_key =
                std::str::from_utf8(&transport_decoded).map_err(|_| ResolvedJweCredentialError::SecretDecodeFailed)?;
            Zeroizing::new(
                base64::engine::general_purpose::STANDARD
                    .decode(encoded_key.trim())
                    .map_err(|_| ResolvedJweCredentialError::SecretDecodeFailed)?,
            )
        } else {
            transport_decoded
        };
        let required = self.content_encryption_algorithm.required_key_len();
        if key.len() != required {
            return Err(ResolvedJweCredentialError::InvalidLength {
                required,
                actual: key.len(),
            });
        }
        Ok(key)
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for JweDecryptConfig {
    fn validation_error(&self) -> Option<String> {
        self.detect_validation_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn operator_validation_requires_secret_ref() {
        let cfg = JweDecryptConfig::default();
        let err = cfg.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("secretRef is required"));
    }

    #[test]
    fn test_detect_validation_error_allowed_algorithms_empty() {
        let cfg = JweDecryptConfig {
            secret_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "jwe-secret".to_string(),
                namespace: None,
            }),
            allowed_algorithms: Some(vec![]),
            ..Default::default()
        };
        let err = cfg.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("allowedAlgorithms cannot be empty"));
    }

    #[test]
    fn test_detect_validation_error_key_len_mismatch() {
        let cfg = JweDecryptConfig {
            secret_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "jwe-secret".to_string(),
                namespace: None,
            }),
            resolved_credential: Some(ResolvedJweCredential {
                secret: Some(base64::engine::general_purpose::STANDARD.encode("short-key")),
            }),
            ..Default::default()
        };
        assert!(cfg.detect_validation_error().is_none());
        assert_eq!(
            cfg.resolved_key_material().unwrap_err(),
            ResolvedJweCredentialError::InvalidLength {
                required: 32,
                actual: 9,
            }
        );
    }

    #[test]
    fn test_detect_validation_error_valid_resolved_secret() {
        let cfg = JweDecryptConfig {
            secret_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "jwe-secret".to_string(),
                namespace: None,
            }),
            resolved_credential: Some(ResolvedJweCredential {
                secret: Some(base64::engine::general_purpose::STANDARD.encode("0123456789abcdef0123456789abcdef")),
            }),
            ..Default::default()
        };
        let err = cfg.detect_validation_error();
        assert!(err.is_none());
    }

    #[test]
    fn resolved_credential_cannot_replace_operator_secret_ref() {
        use crate::resources::edgion_plugins::PluginValidation;
        let config = JweDecryptConfig {
            resolved_credential: Some(ResolvedJweCredential { secret: None }),
            ..Default::default()
        };
        assert_eq!(config.validation_error(), Some("secretRef is required".to_string()));
        assert_eq!(
            config.resolved_key_material().unwrap_err(),
            ResolvedJweCredentialError::Missing
        );
    }

    #[test]
    fn resolved_key_material_classifies_transport_and_inner_base64_failures() {
        let secret_ref = Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "jwe-secret".to_string(),
            namespace: None,
        });
        let transport_invalid = JweDecryptConfig {
            secret_ref: secret_ref.clone(),
            resolved_credential: Some(ResolvedJweCredential {
                secret: Some("not-base64!".to_string()),
            }),
            ..Default::default()
        };
        assert_eq!(
            transport_invalid.resolved_key_material().unwrap_err(),
            ResolvedJweCredentialError::TransportDecodeFailed
        );

        let inner_invalid = JweDecryptConfig {
            secret_ref,
            base64_secret: true,
            resolved_credential: Some(ResolvedJweCredential {
                secret: Some(base64::engine::general_purpose::STANDARD.encode("not-base64!")),
            }),
            ..Default::default()
        };
        assert_eq!(
            inner_invalid.resolved_key_material().unwrap_err(),
            ResolvedJweCredentialError::SecretDecodeFailed
        );

        let inner_non_utf8 = JweDecryptConfig {
            secret_ref: transport_invalid.secret_ref.clone(),
            base64_secret: true,
            resolved_credential: Some(ResolvedJweCredential {
                secret: Some(base64::engine::general_purpose::STANDARD.encode([0xff])),
            }),
            ..Default::default()
        };
        assert_eq!(
            inner_non_utf8.resolved_key_material().unwrap_err(),
            ResolvedJweCredentialError::SecretDecodeFailed
        );

        let key = b"0123456789abcdef0123456789abcdef";
        let double_encoded = JweDecryptConfig {
            secret_ref: transport_invalid.secret_ref,
            base64_secret: true,
            resolved_credential: Some(ResolvedJweCredential {
                secret: Some(
                    base64::engine::general_purpose::STANDARD
                        .encode(base64::engine::general_purpose::STANDARD.encode(key)),
                ),
            }),
            ..Default::default()
        };
        assert_eq!(double_encoded.resolved_key_material().unwrap().as_slice(), key);
    }

    #[test]
    fn validation_rejects_case_insensitive_payload_header_destination_collision() {
        let config = JweDecryptConfig {
            secret_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "jwe-secret".to_string(),
                namespace: None,
            }),
            payload_to_headers: Some(HashMap::from([
                ("sub".to_string(), "X-Identity".to_string()),
                ("email".to_string(), "x-identity".to_string()),
            ])),
            ..Default::default()
        };

        assert_eq!(
            config.validation_error().as_deref(),
            Some(super::super::DUPLICATE_HEADER_DESTINATION)
        );
    }

    #[test]
    fn validation_rejects_payload_destination_owned_by_forward_header() {
        let config = JweDecryptConfig {
            secret_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "jwe-secret".to_string(),
                namespace: None,
            }),
            forward_header: "X-Decrypted".to_string(),
            payload_to_headers: Some(HashMap::from([("sub".to_string(), "x-decrypted".to_string())])),
            ..Default::default()
        };

        assert_eq!(
            config.validation_error().as_deref(),
            Some(super::super::DUPLICATE_HEADER_DESTINATION)
        );
    }
}
