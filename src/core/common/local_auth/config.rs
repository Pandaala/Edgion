//! Local authentication configuration.
//!
//! Provides built-in username/password authentication with JWT for the Admin API.
//! Credentials come only from configuration (this `[local_auth]` section, the
//! `EDGION_ADMIN_*` environment variables, or `[auth]` OIDC). They are never
//! auto-generated. When nothing valid is configured the Admin API returns 503
//! fail-close.
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

/// Local authentication configuration.
///
/// Credentials come only from configuration (this section, the `EDGION_ADMIN_*`
/// environment variables, or `[auth]` OIDC); they are never auto-generated. When
/// nothing valid is configured, no auth provider loads and the Admin API returns
/// 503 fail-close.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalAuthConfig {
    /// Enable local authentication. Default: true.
    pub enabled: bool,

    /// Username for local auth login. Default: "admin".
    pub username: String,

    /// Plaintext password (bcrypt hash is computed at startup).
    /// Empty string means unconfigured (no auth provider loads → 503 fail-close).
    pub password: String,

    /// JWT signing secret. Empty string means unconfigured
    /// (no auth provider loads → 503 fail-close).
    pub jwt_secret: String,

    /// JWT token expiry in hours. Default: 24.
    ///
    /// **Security note**: This value is the **maximum exposure window** for a
    /// leaked / stolen token. Local-auth JWT is stateless — there is no
    /// server-side revocation list; `POST /api/v1/auth/logout` only clears
    /// the browser cookie and does not invalidate already-issued tokens.
    ///
    /// For production deployments that need true token revocation, configure
    /// OIDC federation via `[auth]` instead (the IdP handles session
    /// revocation). When `[local_auth]` must be used, shorten this value
    /// (recommended: 1–4 hours) and rotate `jwt_secret` as a "revoke all"
    /// mechanism. See `skills/02-features/02-config/03-auth-bootstrap.md`.
    pub jwt_expiry_hours: u64,

    /// Paths that skip authentication (e.g., health/ready probes).
    /// `/api/v1/auth/` prefix paths are always skipped regardless of this list.
    #[serde(default = "default_skip_paths")]
    pub skip_paths: Vec<String>,

    /// Emit the `Secure` attribute on the `edgion_token` auth cookie. Default: true.
    ///
    /// The cookie is a bearer-equivalent JWT credential; with `Secure` the browser
    /// never replays it over plaintext `http://`. Keep it `true` for any TLS-fronted
    /// deployment. Set it to `false` only for an explicit non-TLS dev/internal setup
    /// where the Admin API is served over plain HTTP (the browser would otherwise
    /// refuse to store a `Secure` cookie). Mirrors the CSRF `cookieSecure` and OIDC
    /// `sessionCookieSecure` plugin defaults.
    #[serde(default = "default_cookie_secure")]
    pub cookie_secure: bool,
}

impl Default for LocalAuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            username: "admin".to_string(),
            password: String::new(),   // empty = unconfigured (503 fail-close)
            jwt_secret: String::new(), // empty = unconfigured (503 fail-close)
            jwt_expiry_hours: 24,
            skip_paths: default_skip_paths(),
            cookie_secure: default_cookie_secure(),
        }
    }
}

impl LocalAuthConfig {
    /// Apply EDGION_ADMIN_* environment-variable overrides onto this config.
    /// A non-empty env value overrides the corresponding field. This is the
    /// recommended K8s path: inject from a Secret via valueFrom.secretKeyRef.
    pub fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("EDGION_ADMIN_USERNAME") {
            if !v.is_empty() {
                self.username = v;
            }
        }
        if let Ok(v) = std::env::var("EDGION_ADMIN_PASSWORD") {
            if !v.is_empty() {
                self.password = v;
            }
        }
        if let Ok(v) = std::env::var("EDGION_ADMIN_JWT_SECRET") {
            if !v.is_empty() {
                self.jwt_secret = v;
            }
        }
    }

    /// Whether any EDGION_ADMIN_* override is set (non-empty).
    pub fn env_overrides_present() -> bool {
        [
            "EDGION_ADMIN_USERNAME",
            "EDGION_ADMIN_PASSWORD",
            "EDGION_ADMIN_JWT_SECRET",
        ]
        .iter()
        .any(|k| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false))
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
        // Guard against fr-cauth-01: see the same check in `AdminAuthConfig::validate`.
        // A business route added here would slip past unified_auth and become an
        // unauthenticated admin (Controller) or fully unauthenticated (Center) route.
        for p in &self.skip_paths {
            if !crate::core::common::auth::config::skip_path_is_allowlisted(p) {
                return Some(crate::core::common::auth::config::SKIP_PATHS_ALLOWLIST_ERR);
            }
        }
        None
    }
}

fn default_skip_paths() -> Vec<String> {
    crate::core::common::auth::public_paths::PUBLIC_PROBE_PATHS
        .iter()
        .map(|p| (*p).to_string())
        .collect()
}

fn default_cookie_secure() -> bool {
    true
}

/// Shared lock serializing every test that mutates the process-global
/// `EDGION_ADMIN_*` env vars. All such tests live in the single `edgion`
/// lib test binary and run multi-threaded; per-module locks would not
/// serialize across modules. Lock this in EVERY env-mutating test.
#[cfg(test)]
pub(crate) static ADMIN_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_base() -> LocalAuthConfig {
        LocalAuthConfig {
            password: "secret".to_string(),
            jwt_secret: "signing-key".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn skip_paths_business_route_rejected() {
        // fr-cauth-01: a business route in skip_paths must be rejected at validation.
        let mut config = valid_base();
        config.skip_paths = vec!["/health".to_string(), "/api/v1/cluster/httproute".to_string()];
        assert_eq!(
            config.validate(),
            Some(crate::core::common::auth::config::SKIP_PATHS_ALLOWLIST_ERR)
        );
    }

    #[test]
    fn skip_paths_allowlist_accepted() {
        let mut config = valid_base();
        config.skip_paths = vec![
            "/health".to_string(),
            "/ready".to_string(),
            "/metrics".to_string(),
            "/api/v1/auth/status".to_string(),
        ];
        assert!(config.validate().is_none());
    }

    const ENV_VARS: [&str; 3] = [
        "EDGION_ADMIN_USERNAME",
        "EDGION_ADMIN_PASSWORD",
        "EDGION_ADMIN_JWT_SECRET",
    ];

    fn clear_env() {
        for k in ENV_VARS {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn apply_env_overrides_applies_set_and_leaves_unset() {
        let _guard = crate::core::common::local_auth::config::ADMIN_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();

        // Only password + jwt_secret are set; username is left unset.
        std::env::set_var("EDGION_ADMIN_PASSWORD", "env-pass");
        std::env::set_var("EDGION_ADMIN_JWT_SECRET", "env-jwt");

        let mut cfg = LocalAuthConfig {
            username: "original-user".to_string(),
            password: "original-pass".to_string(),
            jwt_secret: "original-jwt".to_string(),
            ..Default::default()
        };
        assert!(LocalAuthConfig::env_overrides_present());
        cfg.apply_env_overrides();

        // Unset var leaves the field unchanged.
        assert_eq!(cfg.username, "original-user");
        // Set vars override.
        assert_eq!(cfg.password, "env-pass");
        assert_eq!(cfg.jwt_secret, "env-jwt");

        clear_env();
    }

    #[test]
    fn apply_env_overrides_empty_value_does_not_override() {
        let _guard = crate::core::common::local_auth::config::ADMIN_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();

        // An explicitly-empty env value must NOT clobber a configured field.
        std::env::set_var("EDGION_ADMIN_PASSWORD", "");

        let mut cfg = LocalAuthConfig {
            password: "original-pass".to_string(),
            ..Default::default()
        };
        // Empty value => not "present".
        assert!(!LocalAuthConfig::env_overrides_present());
        cfg.apply_env_overrides();
        assert_eq!(cfg.password, "original-pass");

        clear_env();
    }

    #[test]
    fn env_overrides_present_false_when_unset() {
        let _guard = crate::core::common::local_auth::config::ADMIN_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        clear_env();
        assert!(!LocalAuthConfig::env_overrides_present());
    }
}
