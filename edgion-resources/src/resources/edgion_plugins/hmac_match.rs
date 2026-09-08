//! HMAC request-matching configuration shared by `HmacAuth` and `Condition`.
//!
//! Lives as a sibling of `conditions.rs` so `Condition` can name
//! `HmacMatchConfig` without importing `plugin_configs` (that path loads
//! `request_access_policy.rs`, which imports `conditions.rs`).

use std::collections::{HashMap, HashSet};

crate::redact::impl_redacted_debug!(HmacCredential, HmacMatchConfig);

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
#[derive(Clone, Serialize, Deserialize)]
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
    /// deserialization of `resolved_credentials` payloads predating this field.
    #[serde(default = "default_group_name")]
    pub group: String,
}

/// A named group of Secret references whose HmacCredentials all carry this
/// group's name. Surfaces in PluginLog as `group=<name>` for audit attribution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HmacAuthSecretGroup {
    /// Group name; charset = Kubernetes label-value
    /// ([`crate::gwapi_types::GROUP_NAME_PATTERN`], <=63 chars).
    #[schemars(
        length(max = crate::gwapi_types::GROUP_NAME_MAX_LEN),
        pattern(crate::gwapi_types::GROUP_NAME_PATTERN)
    )]
    pub name: String,

    /// Human-readable description (not used by runtime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Secret references for this group. Must be non-empty at preparse.
    pub secret_refs: Vec<SecretObjectReference>,
}

/// Canonical HMAC request-matching configuration shared by HmacAuth and
/// Condition. It deliberately contains no response or request mutation policy.
///
/// HMAC fields stay siblings of `type` on the wire (`type: hmacAuth` plus
/// `secretGroups`, `algorithms`, …). `negate` is a sibling too; the standalone
/// HmacAuth plugin ignores it.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HmacMatchConfig {
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

    /// Verify the request body against the signed `Digest` header
    /// (`Digest: SHA-256=<base64(sha256(body))>`). When enabled, the HMAC
    /// verifier captures the canonical request body lazily after the cheaper
    /// signature checks and hashes it through the shared body storage.
    ///
    /// Entry-level `PluginConditions` reject `validateRequestBody: true`
    /// (D-27). RequestAccessPolicy `allOf` and RequestRestriction `allOf`
    /// still accept it.
    #[serde(default)]
    pub validate_request_body: bool,

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

    /// Invert the match result. Default false. Ignored by the HmacAuth plugin.
    #[serde(default)]
    pub negate: bool,

    // === Runtime fields (controller populated) ===
    /// Resolved credential map: username -> credential.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_credentials: Option<HashMap<String, HmacCredential>>,

    /// Runtime-only match descriptor (`<ordinal>:<type>`).
    #[serde(skip)]
    #[schemars(skip)]
    pub descriptor: Box<str>,
}

pub(crate) fn default_algorithms() -> Vec<HmacAlgorithm> {
    vec![
        HmacAlgorithm::HmacSha256,
        HmacAlgorithm::HmacSha384,
        HmacAlgorithm::HmacSha512,
    ]
}

pub(crate) fn default_clock_skew() -> u64 {
    300
}

pub(crate) fn default_min_secret_length() -> usize {
    32
}

pub(crate) fn default_secret_field() -> String {
    "secret".to_string()
}

pub(crate) fn default_username_field() -> String {
    "username".to_string()
}

impl Default for HmacMatchConfig {
    fn default() -> Self {
        Self {
            secret_groups: None,
            algorithms: default_algorithms(),
            clock_skew: default_clock_skew(),
            enforce_headers: None,
            validate_request_body: false,
            min_secret_length: default_min_secret_length(),
            secret_field: default_secret_field(),
            username_field: default_username_field(),
            negate: false,
            resolved_credentials: None,
            descriptor: Box::from(""),
        }
    }
}

impl HmacMatchConfig {
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

        // `validateRequestBody` is accepted here. Entry-level Condition
        // positions reject it separately (D-27). Both RAP consumers own lazy
        // body capture after cheap signature checks; non-forwardable failures
        // remain fail-closed through the canonical body-error mapping.

        None
    }

    pub fn has_credentials_source(&self) -> bool {
        self.secret_groups.as_ref().is_some_and(|groups| !groups.is_empty())
            || self
                .resolved_credentials
                .as_ref()
                .is_some_and(|credentials| !credentials.is_empty())
    }
}
