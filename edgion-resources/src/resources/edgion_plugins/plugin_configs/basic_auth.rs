//! Basic Auth plugin configuration

use std::collections::{HashMap, HashSet};

crate::redact::impl_redacted_debug!(BasicAuthConfig);

use crate::resources::common::group_name::default_group_name;
use crate::resources::edgion_plugins::auth_realm::{validate_anonymous_header_text, validate_auth_realm};
use crate::resources::gateway::SecretObjectReference;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A named group of Secret references whose users all carry this group's
/// name in `ResolvedBasicAuthUser.group`. Used for audit / tenant
/// attribution in PluginLog.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BasicAuthSecretGroup {
    /// Group name; charset = Kubernetes label-value
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

    /// Secret references for this group. Must be non-empty at preparse.
    /// Each Secret is `type: kubernetes.io/basic-auth` with `username` +
    /// `password` fields (1 Secret = 1 user).
    pub secret_refs: Vec<SecretObjectReference>,
}

/// Resolved BasicAuth user (controller-populated). The `password` field
/// holds either plaintext or htpasswd-format hash as stored in the Secret;
/// gateway's `load_users()` pre-hashes plaintext to `{SHA}<base64>` at
/// plugin construction.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedBasicAuthUser {
    /// Password value as stored in Secret (plaintext or pre-hashed).
    pub password: String,

    /// Group this user originated from. Always set by controller in
    /// current versions; `#[serde(default)]` provides a fallback so that
    /// Gateway-new / Controller-old rolling upgrades don't break
    /// deserialization.
    #[serde(default = "default_group_name")]
    pub group: String,
}

impl std::fmt::Debug for ResolvedBasicAuthUser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ResolvedBasicAuthUser([redacted])")
    }
}

/// Basic Auth plugin configuration
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BasicAuthConfig {
    /// Named groups of Secret references. Optional so anonymous-only
    /// deployments remain valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_groups: Option<Vec<BasicAuthSecretGroup>>,

    /// Hide Authorization header from upstream
    #[serde(default)]
    pub hide_credentials: bool,

    /// Delay in milliseconds before returning an authentication failure response.
    /// Increases the time cost for brute-force / credential-stuffing attacks.
    /// Default: 0 (no delay).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,

    /// Realm for WWW-Authenticate header
    #[serde(default = "default_realm")]
    pub realm: String,

    /// Anonymous consumer username (allows unauthenticated access)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<String>,

    /// Resolved users from Secret refs (controller populated).
    /// Key: username, value: ResolvedBasicAuthUser (password + group).
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_users: Option<HashMap<String, ResolvedBasicAuthUser>>,
}

fn default_realm() -> String {
    "API Gateway".to_string()
}

impl Default for BasicAuthConfig {
    fn default() -> Self {
        Self {
            secret_groups: None,
            hide_credentials: false,
            auth_failure_delay_ms: 0,
            realm: default_realm(),
            anonymous: None,
            resolved_users: None,
        }
    }
}

impl BasicAuthConfig {
    pub fn detect_validation_error(&self) -> Option<String> {
        if let Err(error) = validate_auth_realm(&self.realm) {
            return Some(error.to_string());
        }
        if self.realm.trim().is_empty() {
            return Some("realm cannot be empty".to_string());
        }

        // The anonymous username is always written as X-Consumer-Username.
        if let Err(error) = validate_anonymous_header_text(self.anonymous.as_deref()) {
            return Some(error.to_string());
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

        // Final gate: anonymous or groups required
        let has_secret_groups = self.secret_groups.as_ref().is_some_and(|v| !v.is_empty());
        let has_resolved = self.resolved_users.as_ref().is_some_and(|v| !v.is_empty());
        if self.anonymous.is_none() && !has_secret_groups && !has_resolved {
            return Some("secretGroups (or resolvedUsers) is required when anonymous is not set".to_string());
        }

        None
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for BasicAuthConfig {
    fn validation_error(&self) -> Option<String> {
        self.detect_validation_error()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/basic_auth_test.rs"]
mod tests;
