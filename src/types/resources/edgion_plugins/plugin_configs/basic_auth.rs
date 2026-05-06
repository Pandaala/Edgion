//! Basic Auth plugin configuration

use super::lru_cache_config::LruCacheConfig;
use crate::types::resources::gateway::SecretObjectReference;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Basic Auth plugin configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BasicAuthConfig {
    /// References to Kubernetes Secrets (type: kubernetes.io/basic-auth)
    /// Each Secret should contain username and password fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_refs: Option<Vec<SecretObjectReference>>,

    /// Hide Authorization header from upstream
    #[serde(default)]
    pub hide_credentials: bool,

    /// Delay in milliseconds before returning an authentication failure response.
    /// Increases the time cost for brute-force / credential-stuffing attacks.
    /// Default: 0 (no delay).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,

    /// LRU cache TTL in seconds for successful authentications.
    /// Default: 300 (5 minutes). Set to 0 to disable caching entirely.
    #[serde(default = "default_basic_auth_cache_ttl")]
    pub cache_ttl: u64,

    /// LRU cache configuration (capacity and sharding).
    /// Omit or set fields to 0 to use global defaults.
    #[serde(default)]
    pub lru_cache: LruCacheConfig,

    /// Enable singleflight deduplication for concurrent bcrypt verifications.
    /// Only applies to bcrypt hashes (passwords starting with `$2`).
    /// Default: true.
    #[serde(default = "default_basic_auth_singleflight")]
    pub singleflight: bool,

    /// Realm for WWW-Authenticate header
    #[serde(default = "default_realm")]
    pub realm: String,

    /// Anonymous consumer username (allows unauthenticated access)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<String>,

    /// Resolved users from Secret refs (controller populated).
    /// Key: username, value: password/hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_users: Option<HashMap<String, String>>,

    /// Validation error cache.
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

fn default_realm() -> String {
    "API Gateway".to_string()
}

fn default_basic_auth_cache_ttl() -> u64 {
    300
}

fn default_basic_auth_singleflight() -> bool {
    true
}

impl Default for BasicAuthConfig {
    fn default() -> Self {
        Self {
            secret_refs: None,
            hide_credentials: false,
            auth_failure_delay_ms: 0,
            cache_ttl: default_basic_auth_cache_ttl(),
            lru_cache: LruCacheConfig::default(),
            singleflight: default_basic_auth_singleflight(),
            realm: default_realm(),
            anonymous: None,
            resolved_users: None,
            validation_error: None,
        }
    }
}

impl BasicAuthConfig {
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub fn detect_validation_error(&self) -> Option<String> {
        if self.realm.trim().is_empty() {
            return Some("realm cannot be empty".to_string());
        }
        if self.secret_refs.as_ref().is_some_and(|refs| refs.is_empty()) {
            return Some("secretRefs cannot be empty when provided".to_string());
        }
        let has_secret_refs = self.secret_refs.as_ref().is_some_and(|v| !v.is_empty());
        let has_resolved = self.resolved_users.as_ref().is_some_and(|v| !v.is_empty());
        if self.anonymous.is_none() && !has_secret_refs && !has_resolved {
            return Some("secretRefs (or resolvedUsers) is required when anonymous is not set".to_string());
        }
        None
    }
}

use crate::types::resources::edgion_plugins::PluginValidation;

impl PluginValidation for BasicAuthConfig {
    fn validation_error(&self) -> Option<String> {
        self.detect_validation_error()
    }
}

#[cfg(test)]
mod tests {
    use super::BasicAuthConfig;
    use std::collections::HashMap;

    #[test]
    fn test_detect_validation_error_requires_creds_or_anonymous() {
        let cfg = BasicAuthConfig::default();
        assert!(cfg.detect_validation_error().is_some_and(|e| e.contains("secretRefs")));
    }

    #[test]
    fn test_default_auth_failure_delay_ms_is_zero() {
        let cfg = BasicAuthConfig::default();
        assert_eq!(cfg.auth_failure_delay_ms, 0);
    }

    #[test]
    fn test_detect_validation_error_allows_resolved_users() {
        let cfg = BasicAuthConfig {
            resolved_users: Some(HashMap::from([("alice".to_string(), "pass".to_string())])),
            ..Default::default()
        };
        assert!(cfg.detect_validation_error().is_none());
    }

    #[test]
    fn test_basic_auth_cache_config_defaults() {
        let cfg = BasicAuthConfig::default();
        assert_eq!(cfg.cache_ttl, 300);
        assert_eq!(cfg.lru_cache.capacity, 0);
        assert!(cfg.singleflight);
    }

    #[test]
    fn test_basic_auth_cache_disabled_when_ttl_zero() {
        let cfg = BasicAuthConfig {
            cache_ttl: 0,
            ..Default::default()
        };
        assert_eq!(cfg.cache_ttl, 0);
    }
}
