//! HMAC Auth plugin configuration.
//!
//! Supports HTTP Signature style authentication using HMAC algorithms.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::resources::edgion_plugins::hmac_match::{
    default_algorithms, default_clock_skew, default_min_secret_length, default_secret_field, default_username_field,
};
#[cfg(test)]
use crate::resources::gateway::SecretObjectReference;

pub use crate::resources::edgion_plugins::hmac_match::{
    HmacAlgorithm, HmacAuthSecretGroup, HmacCredential, HmacMatchConfig,
};

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
                negate: false,
                resolved_credentials: value.resolved_credentials,
                descriptor: Box::from(""),
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

fn default_realm() -> String {
    "edgion".to_string()
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
