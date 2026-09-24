//! HMAC Auth plugin configuration.
//!
//! Supports HTTP Signature style authentication using HMAC algorithms.

use crate::resources::edgion_plugins::auth_realm::validate_auth_realm;
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
        if let Err(error) = validate_auth_realm(&self.realm) {
            return Some(error.to_string());
        }
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
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/hmac_auth_test.rs"]
mod tests;
