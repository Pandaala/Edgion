//! Local authentication configuration.
//!
//! Provides built-in username/password authentication with JWT for the Admin API.
//! When no OIDC (`auth`) section is present and no `[local_auth]` is configured,
//! credentials are auto-generated and stored as a K8s Secret via `CenterApi`.
//!
//! Example TOML:
//! ```toml
//! [local_auth]
//! enabled = true
//! username = "admin"
//! password = "my-secure-password"
//! jwt_secret = "change-me-in-production"
//! jwt_expiry_hours = 24
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Local authentication configuration.
///
/// When absent from the config file and no OIDC `[auth]` section is configured,
/// local auth is automatically enabled with auto-generated credentials stored
/// as a K8s Secret via `CenterApi`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalAuthConfig {
    /// Enable local authentication. Default: true.
    pub enabled: bool,

    /// Username for local auth login. Default: "admin".
    pub username: String,

    /// Plaintext password (bcrypt hash is computed at startup).
    /// Empty string means credentials need to be generated.
    pub password: String,

    /// JWT signing secret. Empty string means credentials need to be generated.
    pub jwt_secret: String,

    /// JWT token expiry in hours. Default: 24.
    pub jwt_expiry_hours: u64,

    /// Paths that skip authentication (e.g., health/ready probes).
    /// `/api/v1/auth/` prefix paths are always skipped regardless of this list.
    #[serde(default = "default_skip_paths")]
    pub skip_paths: Vec<String>,
}

impl Default for LocalAuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            username: "admin".to_string(),
            password: String::new(),   // empty = needs generation
            jwt_secret: String::new(), // empty = needs generation
            jwt_expiry_hours: 24,
            skip_paths: default_skip_paths(),
        }
    }
}

impl LocalAuthConfig {
    /// Check if credentials need to be generated (password or jwt_secret is empty).
    pub fn needs_generation(&self) -> bool {
        self.password.is_empty() || self.jwt_secret.is_empty()
    }

    /// Validate the configuration. Returns None if valid, Some(error) if invalid.
    pub fn validate(&self) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        if self.username.is_empty() {
            return Some("local_auth.username must not be empty");
        }
        if self.password.is_empty() {
            return Some("local_auth.password must not be empty");
        }
        if self.jwt_secret.is_empty() {
            return Some("local_auth.jwt_secret must not be empty");
        }
        if self.jwt_expiry_hours == 0 {
            return Some("local_auth.jwt_expiry_hours must be greater than 0");
        }
        None
    }

    /// Build a HashSet of skip paths for O(1) lookup in middleware.
    pub fn skip_paths_set(&self) -> HashSet<String> {
        self.skip_paths.iter().cloned().collect()
    }
}

fn default_skip_paths() -> Vec<String> {
    vec!["/health".to_string(), "/ready".to_string()]
}
