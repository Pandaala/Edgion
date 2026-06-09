//! Admin API authentication configuration.
//!
//! When the `auth` section is present in the Controller or Center config file,
//! the Admin API endpoints (except health/ready probes) require a valid JWT
//! Bearer token from the configured OIDC provider.
//!
//! Example YAML:
//! ```yaml
//! auth:
//!   discovery: "https://keycloak.example.com/realms/edgion/.well-known/openid-configuration"
//!   audiences: ["edgion-admin"]
//! ```

use serde::{Deserialize, Serialize};

/// Admin API authentication configuration.
///
/// Omit the entire `auth` section to disable authentication (backward compatible).
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
    /// Default: ["/health", "/ready", "/metrics"].
    #[serde(default = "default_skip_paths")]
    pub skip_paths: Vec<String>,

    /// Whether to verify TLS certificates when fetching discovery/JWKS.
    /// Default: true. Set to false only for development.
    #[serde(default = "default_true")]
    pub ssl_verify: bool,

    /// Max response body size accepted for the OIDC discovery document.
    /// Default: 65 536 bytes (64 KB). Raise only if your IdP returns an
    /// unusually large discovery document — the typical document is < 4 KB.
    /// A misconfigured or malicious endpoint returning a body larger than this
    /// is cut off mid-stream, so peak memory is bounded.
    #[serde(default = "default_discovery_max_response_bytes")]
    pub discovery_max_response_bytes: usize,

    /// Max response body size accepted for the JWKS document. Default:
    /// 1 048 576 bytes (1 MB). Typical JWKS is < 10 KB; raise only if your
    /// IdP publishes an unusually large key set.
    #[serde(default = "default_jwks_max_response_bytes")]
    pub jwks_max_response_bytes: usize,
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
            discovery_max_response_bytes: default_discovery_max_response_bytes(),
            jwks_max_response_bytes: default_jwks_max_response_bytes(),
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
        if self.discovery_max_response_bytes == 0 {
            return Some("auth.discovery_max_response_bytes must be greater than 0");
        }
        if self.jwks_max_response_bytes == 0 {
            return Some("auth.jwks_max_response_bytes must be greater than 0");
        }
        // Guard against fr-cauth-01: an operator must not be able to add a business
        // route to the auth skip-set. A skipped business route slips past unified_auth
        // and (on the Controller) gets Superuser via the assign_superuser fallback, or
        // (on the Center, which has no authz layer) becomes fully unauthenticated.
        for p in &self.skip_paths {
            if !skip_path_is_allowlisted(p) {
                return Some(SKIP_PATHS_ALLOWLIST_ERR);
            }
        }
        None
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
    crate::core::common::auth::public_paths::PUBLIC_PROBE_PATHS
        .iter()
        .map(|p| (*p).to_string())
        .collect()
}

/// Error returned when a `skip_paths` entry is outside the allowed set.
pub(crate) const SKIP_PATHS_ALLOWLIST_ERR: &str =
    "skip_paths entries must be exactly /health, /ready, /metrics, or start with /api/v1/auth/";

/// Whether an operator-supplied `skip_paths` entry is safe to bypass authentication.
///
/// Only the public probe endpoints (`/health`, `/ready`, `/metrics`) and the public
/// auth endpoints (`/api/v1/auth/...`, which are unauthenticated by design) may be
/// skipped. Any other path — in particular a business route like
/// `/api/v1/cluster/httproute` — must NOT be skippable, because a skipped route is
/// unauthenticated and (on the Controller) is granted Superuser by the
/// `assign_superuser_if_no_role` fallback. See finding fr-cauth-01.
pub(crate) fn skip_path_is_allowlisted(path: &str) -> bool {
    crate::core::common::auth::public_paths::is_skip_allowlisted(path)
}

fn default_discovery_max_response_bytes() -> usize {
    65_536
}

fn default_jwks_max_response_bytes() -> usize {
    1_048_576
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
        assert!(config.skip_paths.iter().any(|p| p == "/health"));
        assert!(config.skip_paths.iter().any(|p| p == "/ready"));
        assert!(config.skip_paths.iter().any(|p| p == "/metrics"));
    }

    #[test]
    fn test_skip_paths_business_route_rejected() {
        // fr-cauth-01: a business route in skip_paths must be rejected at validation.
        let config = AdminAuthConfig {
            enabled: true,
            discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
            skip_paths: vec!["/health".to_string(), "/api/v1/cluster/httproute".to_string()],
            ..Default::default()
        };
        assert_eq!(config.validate(), Some(SKIP_PATHS_ALLOWLIST_ERR));
    }

    #[test]
    fn test_skip_paths_allowlist_accepted() {
        // The allowed set (probe + public auth endpoints) must pass validation.
        let config = AdminAuthConfig {
            enabled: true,
            discovery: "https://idp.example.com/.well-known/openid-configuration".to_string(),
            skip_paths: vec![
                "/health".to_string(),
                "/ready".to_string(),
                "/metrics".to_string(),
                "/api/v1/auth/login".to_string(),
            ],
            ..Default::default()
        };
        assert!(config.validate().is_none());
    }

    #[test]
    fn test_deserialize_from_yaml() {
        let yaml = r#"
discovery: "https://idp.example.com/.well-known/openid-configuration"
audiences: [edgion-admin]
clock_skew_seconds: 60
"#;
        let config: AdminAuthConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.discovery,
            "https://idp.example.com/.well-known/openid-configuration"
        );
        assert_eq!(config.audiences, vec!["edgion-admin"]);
        assert_eq!(config.clock_skew_seconds, 60);
        assert_eq!(config.jwks_cache_ttl, 300); // default
    }

    #[test]
    fn body_limit_defaults_and_override() {
        // Defaults when fields are omitted — backward compatible with existing configs.
        let default = AdminAuthConfig::default();
        assert_eq!(default.discovery_max_response_bytes, 65_536);
        assert_eq!(default.jwks_max_response_bytes, 1_048_576);

        // Overrides from YAML.
        let yaml = r#"
discovery: "https://idp.example.com/.well-known/openid-configuration"
discovery_max_response_bytes: 16384
jwks_max_response_bytes: 524288
"#;
        let config: AdminAuthConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.discovery_max_response_bytes, 16_384);
        assert_eq!(config.jwks_max_response_bytes, 524_288);
        assert!(config.validate().is_none());

        // Zero is rejected.
        let bad = AdminAuthConfig {
            discovery: "https://idp.example.com/.well-known/openid-configuration".into(),
            jwks_max_response_bytes: 0,
            ..Default::default()
        };
        assert_eq!(
            bad.validate(),
            Some("auth.jwks_max_response_bytes must be greater than 0")
        );
    }
}
