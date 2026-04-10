//! Admin API authentication configuration.
//!
//! When `[auth]` section is present in the Controller or Center config file,
//! the Admin API endpoints (except health/ready probes) require a valid JWT
//! Bearer token from the configured OIDC provider.
//!
//! Example TOML:
//! ```toml
//! [auth]
//! discovery = "https://keycloak.example.com/realms/edgion/.well-known/openid-configuration"
//! audiences = ["edgion-admin"]
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Admin API authentication configuration.
///
/// Omit the entire `[auth]` section to disable authentication (backward compatible).
/// When present, all Admin API endpoints (except `skip_paths`) require a valid Bearer JWT.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdminAuthConfig {
    /// Enable authentication. Default: true (when the section is present).
    pub enabled: bool,

    /// OIDC discovery URL (required when enabled).
    /// Example: `https://keycloak.example.com/realms/edgion/.well-known/openid-configuration`
    pub discovery: String,

    /// Expected audiences in the JWT `aud` claim.
    /// If empty, audience validation is skipped.
    #[serde(default)]
    pub audiences: Vec<String>,

    /// Expected issuers in the JWT `iss` claim.
    /// If empty, issuer is validated against the discovery document's `issuer` field.
    #[serde(default)]
    pub issuers: Vec<String>,

    /// Allowed signing algorithms. Default: RS256, RS384, RS512, ES256, ES384.
    #[serde(default)]
    pub allowed_algorithms: Vec<String>,

    /// Clock skew tolerance in seconds. Default: 120.
    #[serde(default = "default_clock_skew")]
    pub clock_skew_seconds: u64,

    /// JWKS cache TTL in seconds. Default: 300.
    #[serde(default = "default_jwks_cache_ttl")]
    pub jwks_cache_ttl: u64,

    /// Minimum interval between JWKS refresh attempts in seconds. Default: 10.
    #[serde(default = "default_jwks_min_refresh_interval")]
    pub jwks_min_refresh_interval: u64,

    /// Paths that skip authentication (e.g., health/ready probes).
    /// Default: ["/health", "/ready"].
    #[serde(default = "default_skip_paths")]
    pub skip_paths: Vec<String>,

    /// Whether to verify TLS certificates when fetching discovery/JWKS.
    /// Default: true. Set to false only for development.
    #[serde(default = "default_true")]
    pub ssl_verify: bool,
}

impl Default for AdminAuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            discovery: String::new(),
            audiences: Vec::new(),
            issuers: Vec::new(),
            allowed_algorithms: Vec::new(),
            clock_skew_seconds: default_clock_skew(),
            jwks_cache_ttl: default_jwks_cache_ttl(),
            jwks_min_refresh_interval: default_jwks_min_refresh_interval(),
            skip_paths: default_skip_paths(),
            ssl_verify: true,
        }
    }
}

impl AdminAuthConfig {
    /// Validate the configuration. Returns None if valid, Some(error) if invalid.
    pub fn validate(&self) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        if self.discovery.is_empty() {
            return Some("auth.discovery is required when auth is enabled");
        }
        if !self.discovery.starts_with("https://") && !self.discovery.starts_with("http://") {
            return Some("auth.discovery must start with http:// or https://");
        }
        if self.jwks_cache_ttl == 0 {
            return Some("auth.jwks_cache_ttl must be greater than 0");
        }
        if self.jwks_min_refresh_interval == 0 {
            return Some("auth.jwks_min_refresh_interval must be greater than 0");
        }
        None
    }

    /// Build a HashSet of skip paths for O(1) lookup in middleware.
    pub fn skip_paths_set(&self) -> HashSet<String> {
        self.skip_paths.iter().cloned().collect()
    }
}

fn default_clock_skew() -> u64 {
    120
}

fn default_jwks_cache_ttl() -> u64 {
    300
}

fn default_jwks_min_refresh_interval() -> u64 {
    10
}

fn default_skip_paths() -> Vec<String> {
    vec!["/health".to_string(), "/ready".to_string()]
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_validates_as_missing_discovery() {
        let config = AdminAuthConfig::default();
        assert_eq!(
            config.validate(),
            Some("auth.discovery is required when auth is enabled")
        );
    }

    #[test]
    fn test_disabled_config_always_valid() {
        let config = AdminAuthConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(config.validate().is_none());
    }

    #[test]
    fn test_valid_config() {
        let config = AdminAuthConfig {
            enabled: true,
            discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
            audiences: vec!["edgion-admin".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_none());
    }

    #[test]
    fn test_invalid_discovery_scheme() {
        let config = AdminAuthConfig {
            discovery: "ftp://bad.example.com".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.validate(),
            Some("auth.discovery must start with http:// or https://")
        );
    }

    #[test]
    fn test_skip_paths_default() {
        let config = AdminAuthConfig::default();
        let set = config.skip_paths_set();
        assert!(set.contains("/health"));
        assert!(set.contains("/ready"));
    }

    #[test]
    fn test_deserialize_from_toml() {
        let toml = r#"
            discovery = "https://idp.example.com/.well-known/openid-configuration"
            audiences = ["edgion-admin"]
            clock_skew_seconds = 60
        "#;
        let config: AdminAuthConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            config.discovery,
            "https://idp.example.com/.well-known/openid-configuration"
        );
        assert_eq!(config.audiences, vec!["edgion-admin"]);
        assert_eq!(config.clock_skew_seconds, 60);
        assert_eq!(config.jwks_cache_ttl, 300); // default
    }
}
