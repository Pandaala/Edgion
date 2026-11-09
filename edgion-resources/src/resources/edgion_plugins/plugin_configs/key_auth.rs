//! Key Auth plugin configuration
//!
//! API Key authentication plugin that validates requests against configured keys
//! stored in Kubernetes Secrets.

use schemars::JsonSchema;

crate::redact::impl_redacted_debug!(KeyAuthConfig, KeyMetadata);
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::common::KeyGet;
use crate::resources::common::group_name::default_group_name;
use crate::resources::edgion_plugins::auth_realm::{
    validate_anonymous_header_text, validate_auth_realm, validate_owned_header_names,
};
use crate::resources::edgion_plugins::plugin_validation::PluginValidation;
use crate::resources::gateway::SecretObjectReference;

/// A named group of Secret references whose keys all carry this group's
/// name in `KeyMetadata.group`. Used for audit / tenant attribution in
/// PluginLog.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyAuthSecretGroup {
    /// Group name. Charset: Kubernetes label-value
    /// ([`crate::gwapi_types::GROUP_NAME_PATTERN`], <=63 chars).
    /// Surfaces in PluginLog as `group=<name>`.
    #[schemars(
        length(max = crate::gwapi_types::GROUP_NAME_MAX_LEN),
        pattern(crate::gwapi_types::GROUP_NAME_PATTERN)
    )]
    pub name: String,

    /// Human-readable description (not used by runtime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Secret references for this group. Must be non-empty.
    pub secret_refs: Vec<SecretObjectReference>,
}

/// Key Auth plugin configuration
///
/// Supports API key authentication from various sources (header, query, cookie, etc.).
/// Keys are stored in Kubernetes Secrets in YAML format.
///
/// ## Key Source Configuration
///
/// Use `key_sources` to specify where to look for the API key. Sources are tried in order.
///
/// ```yaml
/// keySources:
///   - type: header
///     name: "X-API-Key"
///   - type: query
///     name: "api_key"
///   - type: cookie
///     name: "api_key"
/// ```
///
/// If not specified, defaults to header "apikey" then query "apikey".
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyAuthConfig {
    // === Key Source ===
    /// Sources to extract API key from (tried in order)
    /// Supports: header, query, cookie, ctx
    /// Default: [header:"apikey", query:"apikey"]
    #[serde(default = "default_key_sources")]
    pub key_sources: Vec<KeyGet>,

    // === Security Options ===
    /// Remove API key from request before forwarding to upstream (default: false)
    #[serde(default)]
    pub hide_credentials: bool,

    /// Delay in milliseconds before returning an authentication failure response.
    /// Increases the time cost for brute-force / credential-stuffing attacks.
    /// Default: 0 (no delay).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,

    /// Username for anonymous access when no key provided (optional)
    /// If set, requests without key will be allowed with this username
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<String>,

    /// Realm for WWW-Authenticate header (default: "API Gateway")
    #[serde(default = "default_realm")]
    pub realm: String,

    // === Key Storage ===
    /// Secret field name that contains the API key value (default: "key")
    #[serde(default = "default_key_field")]
    pub key_field: String,

    /// Named groups of Secret references. Optional so anonymous-only
    /// deployments (no keys) remain valid.
    ///
    /// Each group's `secret_refs` are resolved and merged into a single
    /// runtime key map; the resulting `KeyMetadata.group` records origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_groups: Option<Vec<KeyAuthSecretGroup>>,

    // === Upstream Headers ===
    /// Secret fields to add as upstream headers; if empty, no headers are forwarded.
    #[serde(default)]
    pub upstream_header_fields: Vec<String>,

    // === Runtime fields (populated by controller) ===
    /// Resolved API Keys mapping: key -> metadata
    /// This is populated during Secret parsing and should not be set by users
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_keys: Option<HashMap<String, KeyMetadata>>,
}

/// API Key metadata attached to a resolved key.
#[derive(Clone, Serialize, Deserialize)]
pub struct KeyMetadata {
    /// Headers to forward to upstream (field_name -> value).
    pub headers: HashMap<String, String>,

    /// Group this key originated from. Always set by controller in current
    /// versions. `#[serde(default)]` lets Gateway-new / Controller-old
    /// rolling upgrades deserialize old payloads (fallback `"default"`).
    #[serde(default = "default_group_name")]
    pub group: String,
}

fn default_key_sources() -> Vec<KeyGet> {
    vec![
        KeyGet::Header {
            name: "apikey".to_string(),
        },
        KeyGet::Query {
            name: "apikey".to_string(),
        },
    ]
}

fn default_realm() -> String {
    "API Gateway".to_string()
}

fn default_key_field() -> String {
    "key".to_string()
}

impl Default for KeyAuthConfig {
    fn default() -> Self {
        Self {
            key_sources: default_key_sources(),
            hide_credentials: false,
            auth_failure_delay_ms: 0,
            anonymous: None,
            realm: default_realm(),
            key_field: default_key_field(),
            secret_groups: None,
            upstream_header_fields: vec![],
            resolved_keys: None,
        }
    }
}

impl KeyAuthConfig {
    /// Whether the anonymous username is written as `X-Consumer-Username`
    /// (exact-case whitelist entry). The Gateway and admission share this rule.
    pub fn forwards_anonymous_username(&self) -> bool {
        self.upstream_header_fields
            .iter()
            .any(|field| field.eq_ignore_ascii_case("X-Consumer-Username"))
    }

    /// Validate configuration. Returns `Err(message)` if invalid.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_and_check()
    }

    /// Internal validation logic
    fn validate_and_check(&self) -> Result<(), String> {
        // Check that key_sources is not empty
        if self.key_sources.is_empty() {
            return Err("key_sources cannot be empty - at least one key source must be specified".to_string());
        }

        // Validate each key source
        for (i, source) in self.key_sources.iter().enumerate() {
            match source {
                KeyGet::Header { name } | KeyGet::Query { name } | KeyGet::Cookie { name } | KeyGet::Ctx { name } => {
                    if name.is_empty() {
                        return Err(format!("key_sources[{}]: name cannot be empty", i));
                    }
                }
                // Unsupported sources for API key extraction
                KeyGet::RemoteIp
                | KeyGet::DirectPeerIp
                | KeyGet::Host
                | KeyGet::TraceId
                | KeyGet::Path
                | KeyGet::Method
                | KeyGet::RemoteIpAndPath
                | KeyGet::RequestPath => {
                    return Err(format!(
                        "key_sources[{}]: unsupported source type '{}' for API key extraction. Use header, query, cookie, or ctx",
                        i,
                        source.source_type()
                    ));
                }
            }
        }

        // Validate key_field is not empty
        if self.key_field.is_empty() {
            return Err("key_field cannot be empty".to_string());
        }

        // realm must not be empty
        validate_auth_realm(&self.realm).map_err(str::to_owned)?;
        if self.realm.is_empty() {
            return Err("realm cannot be empty".to_string());
        }

        // Whitelisted metadata names are cleared and written on every continuation.
        validate_owned_header_names("upstreamHeaderFields", &self.upstream_header_fields)?;

        // The anonymous username is only written when it is forwarded.
        if self.forwards_anonymous_username() {
            validate_anonymous_header_text(self.anonymous.as_deref()).map_err(str::to_string)?;
        }

        // Validate secret_groups structure (cross-group raw-key uniqueness
        // is checked at controller resolve time, not here).
        if let Some(groups) = &self.secret_groups {
            if groups.is_empty() {
                return Err("secret_groups cannot be empty when present".to_string());
            }
            let mut seen: HashSet<&str> = HashSet::new();
            for g in groups {
                crate::resources::common::group_name::validate_group_name(&g.name)
                    .map_err(|e| format!("secret_groups: {}", e))?;
                if !seen.insert(g.name.as_str()) {
                    return Err(format!("duplicate group name '{}'", g.name));
                }
                if g.secret_refs.is_empty() {
                    return Err(format!("group '{}' has no secretRefs", g.name));
                }
            }
        }

        Ok(())
    }
}

impl PluginValidation for KeyAuthConfig {
    fn validation_error(&self) -> Option<String> {
        self.validate_and_check().err()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/key_auth_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/key_auth_test/validation_trait_tests.rs"]
mod validation_trait_tests;
