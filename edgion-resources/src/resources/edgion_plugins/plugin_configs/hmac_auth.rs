//! HMAC Auth plugin configuration.
//!
//! Supports HTTP Signature style authentication using HMAC algorithms.

use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::group_name::default_group_name;
use crate::resources::gateway::SecretObjectReference;

/// HMAC signature algorithm.
///
/// Note: HMAC-SHA1 is intentionally excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Hash)]
pub enum HmacAlgorithm {
    /// HMAC using SHA-256.
    #[serde(rename = "hmac-sha256")]
    HmacSha256,
    /// HMAC using SHA-384.
    #[serde(rename = "hmac-sha384")]
    HmacSha384,
    /// HMAC using SHA-512.
    #[serde(rename = "hmac-sha512")]
    HmacSha512,
}

impl HmacAlgorithm {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HmacSha256 => "hmac-sha256",
            Self::HmacSha384 => "hmac-sha384",
            Self::HmacSha512 => "hmac-sha512",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "hmac-sha256" => Some(Self::HmacSha256),
            "hmac-sha384" => Some(Self::HmacSha384),
            "hmac-sha512" => Some(Self::HmacSha512),
            _ => None,
        }
    }
}

/// HMAC credential loaded from K8s Secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HmacCredential {
    /// HMAC shared secret bytes.
    pub secret: Vec<u8>,

    /// Metadata headers to forward to upstream.
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Group this credential originated from. Always set by controller in
    /// current versions; `#[serde(default)]` provides a fallback so that
    /// Gateway-new / Controller-old rolling upgrades don't break
    /// deserialization of `resolved_credentials` payloads predating this field
    /// (lesson L2).
    #[serde(default = "default_group_name")]
    pub group: String,
}

/// A named group of Secret references whose HmacCredentials all carry this
/// group's name. Surfaces in PluginLog as `group=<name>` for audit attribution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HmacAuthSecretGroup {
    /// Group name; charset = Kubernetes label-value
    /// (`^[a-zA-Z0-9]([-_.a-zA-Z0-9]*[a-zA-Z0-9])?$`, <=63 chars).
    pub name: String,

    /// Human-readable description (not used by runtime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Secret references for this group. Must be non-empty at preparse.
    pub secret_refs: Vec<SecretObjectReference>,
}

/// HMAC Auth plugin configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HmacAuthConfig {
    // === Credential Source ===
    /// Named groups of Secret references. Optional so anonymous-only
    /// deployments (no credentials) remain valid.
    ///
    /// Each group's `secret_refs` are resolved and merged into a single
    /// runtime credential map; `HmacCredential.group` records origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_groups: Option<Vec<HmacAuthSecretGroup>>,

    // === Signature Verification ===
    /// Allowed HMAC algorithms.
    #[serde(default = "default_algorithms")]
    pub algorithms: Vec<HmacAlgorithm>,

    /// Maximum allowed clock skew in seconds.
    #[serde(default = "default_clock_skew")]
    #[schemars(range(min = 1))]
    pub clock_skew: u64,

    /// Headers that must be included in signed headers list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforce_headers: Option<Vec<String>>,

    /// Validate request body digest (Phase 2 capability).
    #[serde(default)]
    pub validate_request_body: bool,

    // === Security Options ===
    /// Remove Authorization/Proxy-Authorization headers before forwarding upstream.
    #[serde(default)]
    pub hide_credentials: bool,

    /// Anonymous username when signature is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<String>,

    /// WWW-Authenticate realm.
    #[serde(default = "default_realm")]
    pub realm: String,

    /// Delay before auth failure response (milliseconds).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,

    // === Credential Fields ===
    /// Minimum acceptable secret length in bytes.
    /// Secrets shorter than this trigger a warning during credential resolution.
    /// Defaults to 32 (matching SHA-256 output size).
    #[serde(default = "default_min_secret_length")]
    pub min_secret_length: usize,

    /// Field name in credential entry for secret value.
    #[serde(default = "default_secret_field")]
    pub secret_field: String,

    /// Field name in credential entry for username.
    #[serde(default = "default_username_field")]
    pub username_field: String,

    // === Upstream Headers ===
    /// Whitelist of metadata header fields allowed to forward upstream.
    #[serde(default)]
    pub upstream_header_fields: Vec<String>,

    // === Runtime fields (controller populated) ===
    /// Resolved credential map: username -> credential.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_credentials: Option<HashMap<String, HmacCredential>>,
}

fn default_algorithms() -> Vec<HmacAlgorithm> {
    vec![
        HmacAlgorithm::HmacSha256,
        HmacAlgorithm::HmacSha384,
        HmacAlgorithm::HmacSha512,
    ]
}

fn default_clock_skew() -> u64 {
    300
}

fn default_realm() -> String {
    "edgion".to_string()
}

fn default_min_secret_length() -> usize {
    32
}

fn default_secret_field() -> String {
    "secret".to_string()
}

fn default_username_field() -> String {
    "username".to_string()
}

impl Default for HmacAuthConfig {
    fn default() -> Self {
        Self {
            secret_groups: None,
            algorithms: default_algorithms(),
            clock_skew: default_clock_skew(),
            enforce_headers: None,
            validate_request_body: false,
            hide_credentials: false,
            anonymous: None,
            realm: default_realm(),
            auth_failure_delay_ms: 0,
            min_secret_length: default_min_secret_length(),
            secret_field: default_secret_field(),
            username_field: default_username_field(),
            upstream_header_fields: vec![],
            resolved_credentials: None,
        }
    }
}

impl HmacAuthConfig {
    pub fn detect_validation_error(&self) -> Option<String> {
        fn has_control_chars(s: &str) -> bool {
            s.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0')
        }

        if self.algorithms.is_empty() {
            return Some("algorithms cannot be empty".to_string());
        }
        if self.clock_skew == 0 {
            return Some("clockSkew must be > 0".to_string());
        }
        if self.realm.trim().is_empty() {
            return Some("realm cannot be empty".to_string());
        }
        if self.secret_field.trim().is_empty() {
            return Some("secretField cannot be empty".to_string());
        }
        if self.username_field.trim().is_empty() {
            return Some("usernameField cannot be empty".to_string());
        }
        if let Some(required) = &self.enforce_headers {
            if required.is_empty() {
                return Some("enforceHeaders cannot be empty when provided".to_string());
            }
            if required.iter().any(|h| h.trim().is_empty()) {
                return Some("enforceHeaders contains empty header name".to_string());
            }
            if required.iter().any(|h| has_control_chars(h)) {
                return Some("enforceHeaders contains invalid control characters".to_string());
            }
        }
        if self.upstream_header_fields.iter().any(|h| h.trim().is_empty()) {
            return Some("upstreamHeaderFields contains empty header name".to_string());
        }
        if self.upstream_header_fields.iter().any(|h| has_control_chars(h)) {
            return Some("upstreamHeaderFields contains invalid control characters".to_string());
        }

        // Validate secret_groups structure (cross-group username uniqueness
        // is checked at controller resolve time, not here).
        if let Some(groups) = &self.secret_groups {
            if groups.is_empty() {
                return Some("secretGroups cannot be empty when present".to_string());
            }
            let mut seen: HashSet<&str> = HashSet::new();
            for g in groups {
                if let Err(e) = crate::resources::common::group_name::validate_group_name(&g.name) {
                    return Some(format!("secretGroups: {}", e));
                }
                if !seen.insert(g.name.as_str()) {
                    return Some(format!("duplicate group name '{}'", g.name));
                }
                if g.secret_refs.is_empty() {
                    return Some(format!("group '{}' has no secretRefs", g.name));
                }
            }
        }

        if self.validate_request_body {
            return Some("validateRequestBody is not supported yet (requires request body cache support)".to_string());
        }

        let has_secret_groups = self.secret_groups.as_ref().is_some_and(|v| !v.is_empty());
        let has_resolved = self
            .resolved_credentials
            .as_ref()
            .is_some_and(|credentials| !credentials.is_empty());
        if self.anonymous.is_none() && !has_secret_groups && !has_resolved {
            return Some("secretGroups (or resolvedCredentials) is required when anonymous is not set".to_string());
        }

        None
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for HmacAuthConfig {
    fn validation_error(&self) -> Option<String> {
        self.detect_validation_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let config = HmacAuthConfig::default();
        assert_eq!(config.algorithms.len(), 3);
        assert_eq!(config.clock_skew, 300);
        assert_eq!(config.realm, "edgion");
        assert_eq!(config.secret_field, "secret");
        assert_eq!(config.username_field, "username");
        assert!(!config.validate_request_body);
    }

    #[test]
    fn test_detect_validation_error_algorithms_empty() {
        let config = HmacAuthConfig {
            algorithms: vec![],
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("algorithms cannot be empty"));
    }

    #[test]
    fn test_detect_validation_error_clock_skew_zero() {
        let config = HmacAuthConfig {
            clock_skew: 0,
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("clockSkew must be > 0"));
    }

    #[test]
    fn test_detect_validation_error_validate_body_unsupported() {
        let config = HmacAuthConfig {
            validate_request_body: true,
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("validateRequestBody is not supported yet"));
    }

    #[test]
    fn test_detect_validation_error_requires_credentials_or_anonymous() {
        let config = HmacAuthConfig {
            secret_groups: None,
            resolved_credentials: None,
            anonymous: None,
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("secretGroups"));
    }

    #[test]
    fn test_detect_validation_error_allows_anonymous_without_credentials() {
        let config = HmacAuthConfig {
            secret_groups: None,
            resolved_credentials: None,
            anonymous: Some("guest".to_string()),
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert!(err.is_none());
    }

    #[test]
    fn test_validate_secret_groups_empty() {
        let config = HmacAuthConfig {
            secret_groups: Some(vec![]),
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert_eq!(err.unwrap(), "secretGroups cannot be empty when present");
    }

    #[test]
    fn test_validate_secret_groups_duplicate_name() {
        let config = HmacAuthConfig {
            secret_groups: Some(vec![
                HmacAuthSecretGroup {
                    name: "a".into(),
                    description: None,
                    secret_refs: vec![SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "s1".into(),
                        namespace: None,
                    }],
                },
                HmacAuthSecretGroup {
                    name: "a".into(),
                    description: None,
                    secret_refs: vec![SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "s2".into(),
                        namespace: None,
                    }],
                },
            ]),
            ..Default::default()
        };
        assert!(config
            .detect_validation_error()
            .unwrap()
            .contains("duplicate group name 'a'"));
    }

    #[test]
    fn test_validate_secret_groups_invalid_name() {
        let config = HmacAuthConfig {
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "-bad".into(),
                description: None,
                secret_refs: vec![SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "s".into(),
                    namespace: None,
                }],
            }]),
            ..Default::default()
        };
        assert!(config
            .detect_validation_error()
            .unwrap()
            .contains("must start with alphanumeric"));
    }

    #[test]
    fn test_validate_secret_groups_empty_refs() {
        let config = HmacAuthConfig {
            secret_groups: Some(vec![HmacAuthSecretGroup {
                name: "a".into(),
                description: None,
                secret_refs: vec![],
            }]),
            ..Default::default()
        };
        assert!(config
            .detect_validation_error()
            .unwrap()
            .contains("group 'a' has no secretRefs"));
    }
}
