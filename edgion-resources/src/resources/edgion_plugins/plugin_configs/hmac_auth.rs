//! HMAC Auth plugin configuration.
//!
//! Supports HTTP Signature style authentication using HMAC algorithms.

use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
/// RequestAccessPolicy. It deliberately contains no response or request
/// mutation policy.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

    // === Runtime fields (controller populated) ===
    /// Resolved credential map: username -> credential.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_credentials: Option<HashMap<String, HmacCredential>>,
}

/// HMAC Auth plugin configuration. Its wire representation remains flat while
/// the matching fields are composed through [`HmacMatchConfig`] internally.
#[derive(Debug, Clone, JsonSchema)]
#[schemars(rename_all = "camelCase")]
pub struct HmacAuthConfig {
    #[schemars(flatten)]
    pub match_config: HmacMatchConfig,
    #[schemars(default)]
    pub hide_credentials: bool,
    pub anonymous: Option<String>,
    #[schemars(default = "default_realm")]
    pub realm: String,
    #[schemars(default)]
    pub auth_failure_delay_ms: u64,
    #[schemars(default)]
    pub upstream_header_fields: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HmacAuthConfigWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret_groups: Option<Vec<HmacAuthSecretGroup>>,
    #[serde(default = "default_algorithms")]
    algorithms: Vec<HmacAlgorithm>,
    #[serde(default = "default_clock_skew")]
    clock_skew: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enforce_headers: Option<Vec<String>>,
    #[serde(default)]
    validate_request_body: bool,
    #[serde(default = "default_min_secret_length")]
    min_secret_length: usize,
    #[serde(default = "default_secret_field")]
    secret_field: String,
    #[serde(default = "default_username_field")]
    username_field: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    resolved_credentials: Option<HashMap<String, HmacCredential>>,
    #[serde(default)]
    hide_credentials: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anonymous: Option<String>,
    #[serde(default = "default_realm")]
    realm: String,
    #[serde(default)]
    auth_failure_delay_ms: u64,
    #[serde(default)]
    upstream_header_fields: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HmacAuthConfigWireRef<'a> {
    #[serde(skip_serializing_if = "option_ref_is_none")]
    secret_groups: &'a Option<Vec<HmacAuthSecretGroup>>,
    algorithms: &'a Vec<HmacAlgorithm>,
    clock_skew: u64,
    #[serde(skip_serializing_if = "option_ref_is_none")]
    enforce_headers: &'a Option<Vec<String>>,
    validate_request_body: bool,
    min_secret_length: usize,
    secret_field: &'a str,
    username_field: &'a str,
    #[serde(
        skip_serializing_if = "option_ref_is_none",
        serialize_with = "serialize_redacted_option_ref"
    )]
    resolved_credentials: &'a Option<HashMap<String, HmacCredential>>,
    hide_credentials: bool,
    #[serde(skip_serializing_if = "option_ref_is_none")]
    anonymous: &'a Option<String>,
    realm: &'a str,
    auth_failure_delay_ms: u64,
    upstream_header_fields: &'a Vec<String>,
}

fn option_ref_is_none<T>(value: &&Option<T>) -> bool {
    value.is_none()
}

fn serialize_redacted_option_ref<T, S>(value: &&Option<T>, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
    S: Serializer,
{
    crate::redact::redact_opt::serialize(*value, serializer)
}

impl<'a> From<&'a HmacAuthConfig> for HmacAuthConfigWireRef<'a> {
    fn from(value: &'a HmacAuthConfig) -> Self {
        Self {
            secret_groups: &value.match_config.secret_groups,
            algorithms: &value.match_config.algorithms,
            clock_skew: value.match_config.clock_skew,
            enforce_headers: &value.match_config.enforce_headers,
            validate_request_body: value.match_config.validate_request_body,
            min_secret_length: value.match_config.min_secret_length,
            secret_field: &value.match_config.secret_field,
            username_field: &value.match_config.username_field,
            resolved_credentials: &value.match_config.resolved_credentials,
            hide_credentials: value.hide_credentials,
            anonymous: &value.anonymous,
            realm: &value.realm,
            auth_failure_delay_ms: value.auth_failure_delay_ms,
            upstream_header_fields: &value.upstream_header_fields,
        }
    }
}

impl From<HmacAuthConfigWire> for HmacAuthConfig {
    fn from(value: HmacAuthConfigWire) -> Self {
        Self {
            match_config: HmacMatchConfig {
                secret_groups: value.secret_groups,
                algorithms: value.algorithms,
                clock_skew: value.clock_skew,
                enforce_headers: value.enforce_headers,
                validate_request_body: value.validate_request_body,
                min_secret_length: value.min_secret_length,
                secret_field: value.secret_field,
                username_field: value.username_field,
                resolved_credentials: value.resolved_credentials,
            },
            hide_credentials: value.hide_credentials,
            anonymous: value.anonymous,
            realm: value.realm,
            auth_failure_delay_ms: value.auth_failure_delay_ms,
            upstream_header_fields: value.upstream_header_fields,
        }
    }
}

impl Serialize for HmacAuthConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        HmacAuthConfigWireRef::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for HmacAuthConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        HmacAuthConfigWire::deserialize(deserializer).map(Into::into)
    }
}

impl std::ops::Deref for HmacAuthConfig {
    type Target = HmacMatchConfig;

    fn deref(&self) -> &Self::Target {
        &self.match_config
    }
}

impl std::ops::DerefMut for HmacAuthConfig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.match_config
    }
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
            resolved_credentials: None,
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

        // `validateRequestBody` is accepted here. Both consumers own lazy body
        // capture after cheap signature checks; non-forwardable failures remain
        // fail-closed through the canonical body-error mapping.

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

impl Default for HmacAuthConfig {
    fn default() -> Self {
        Self {
            match_config: HmacMatchConfig::default(),
            hide_credentials: false,
            anonymous: None,
            realm: default_realm(),
            auth_failure_delay_ms: 0,
            upstream_header_fields: vec![],
        }
    }
}

impl HmacAuthConfig {
    pub fn detect_validation_error(&self) -> Option<String> {
        if let Some(error) = self.match_config.detect_validation_error() {
            return Some(error);
        }
        if self.realm.trim().is_empty() {
            return Some("realm cannot be empty".to_string());
        }
        if self.upstream_header_fields.iter().any(|h| h.trim().is_empty()) {
            return Some("upstreamHeaderFields contains empty header name".to_string());
        }
        if self
            .upstream_header_fields
            .iter()
            .any(|h| h.bytes().any(|b| b == b'\r' || b == b'\n' || b == b'\0'))
        {
            return Some("upstreamHeaderFields contains invalid control characters".to_string());
        }
        if self.anonymous.is_none() && !self.match_config.has_credentials_source() {
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
            match_config: HmacMatchConfig {
                algorithms: vec![],
                ..Default::default()
            },
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("algorithms cannot be empty"));
    }

    #[test]
    fn test_detect_validation_error_clock_skew_zero() {
        let config = HmacAuthConfig {
            match_config: HmacMatchConfig {
                clock_skew: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert!(err.is_some());
        assert!(err.unwrap().contains("clockSkew must be > 0"));
    }

    #[test]
    fn test_validate_request_body_is_accepted() {
        // validateRequestBody is now a supported config: it is enforced at
        // runtime via the body gate, not rejected at config validation. With a
        // valid credential source it must produce no validation error.
        let config = HmacAuthConfig {
            match_config: HmacMatchConfig {
                validate_request_body: true,
                ..Default::default()
            },
            anonymous: Some("anon".to_string()),
            ..Default::default()
        };
        assert!(config.detect_validation_error().is_none());
    }

    #[test]
    fn test_detect_validation_error_requires_credentials_or_anonymous() {
        let config = HmacAuthConfig {
            match_config: HmacMatchConfig::default(),
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
            match_config: HmacMatchConfig::default(),
            anonymous: Some("guest".to_string()),
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert!(err.is_none());
    }

    #[test]
    fn test_validate_secret_groups_empty() {
        let config = HmacAuthConfig {
            match_config: HmacMatchConfig {
                secret_groups: Some(vec![]),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = config.detect_validation_error();
        assert_eq!(err.unwrap(), "secretGroups cannot be empty when present");
    }

    #[test]
    fn test_validate_secret_groups_duplicate_name() {
        let config = HmacAuthConfig {
            match_config: HmacMatchConfig {
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
            },
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
            match_config: HmacMatchConfig {
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
            },
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
            match_config: HmacMatchConfig {
                secret_groups: Some(vec![HmacAuthSecretGroup {
                    name: "a".into(),
                    description: None,
                    secret_refs: vec![],
                }]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config
            .detect_validation_error()
            .unwrap()
            .contains("group 'a' has no secretRefs"));
    }

    #[test]
    fn standalone_wire_stays_flat_and_round_trips() {
        let config: HmacAuthConfig =
            serde_yaml::from_str("anonymous: guest\nrealm: edge\nvalidateRequestBody: true\nhideCredentials: true\n")
                .unwrap();
        assert_eq!(config.anonymous.as_deref(), Some("guest"));
        assert!(config.match_config.validate_request_body);
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("validateRequestBody: true"), "{yaml}");
        assert!(!yaml.contains("matchConfig"), "{yaml}");
    }

    #[test]
    fn standalone_wire_keeps_lenient_unknown_field_behavior() {
        let config: HmacAuthConfig = serde_yaml::from_str("anonymous: guest\nfutureField: ignored\n").unwrap();
        assert_eq!(config.anonymous.as_deref(), Some("guest"));
    }

    #[test]
    fn standalone_schema_preserves_defaults_and_flat_shape() {
        let schema = serde_json::to_value(schemars::schema_for!(HmacAuthConfig)).unwrap();
        let properties = schema
            .pointer("/properties")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        assert!(properties.contains_key("validateRequestBody"));
        assert!(properties.contains_key("realm"));
        assert!(!properties.contains_key("matchConfig"));
        let required = schema
            .pointer("/required")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(!required.iter().any(|field| field == "realm"));
        assert!(!required.iter().any(|field| field == "validateRequestBody"));
    }

    #[test]
    fn resolved_credentials_remain_redacted_through_flat_wire_adapter() {
        let mut config = HmacAuthConfig {
            anonymous: Some("guest".to_string()),
            ..Default::default()
        };
        config.match_config.resolved_credentials = Some(HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: b"do-not-expose".to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        )]));
        let json = {
            let _guard = crate::redact::AdminRedactGuard::new();
            serde_json::to_string(&config).unwrap()
        };
        assert!(!json.contains("do-not-expose"), "{json}");
        assert!(json.contains("[redacted]"), "{json}");
    }

    #[test]
    fn resolved_credentials_round_trip_outside_admin_redaction() {
        let mut config = HmacAuthConfig::default();
        config.match_config.resolved_credentials = Some(HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: b"conf-sync-secret".to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        )]));

        let wire = serde_json::to_vec(&config).unwrap();
        let decoded: HmacAuthConfig = serde_json::from_slice(&wire).unwrap();
        assert_eq!(
            decoded
                .match_config
                .resolved_credentials
                .as_ref()
                .and_then(|credentials| credentials.get("alice"))
                .map(|credential| credential.secret.as_slice()),
            Some(b"conf-sync-secret".as_slice())
        );
    }
}
