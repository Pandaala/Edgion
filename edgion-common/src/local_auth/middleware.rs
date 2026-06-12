//! Shared state for local username/password authentication.
//!
//! The actual Bearer/cookie extraction and JWT validation lives in
//! `common::unified_auth`, which is the only middleware wired into the Admin
//! API. This module exposes `LocalAuthState` so `unified_auth` can perform the
//! local-validation fallback without depending on full middleware plumbing.

use std::sync::Arc;

use super::config::LocalAuthConfig;

/// State shared by the local auth middleware.
pub struct LocalAuthState {
    /// Bcrypt hash of the configured password.
    pub password_hash: String,
    /// Dummy bcrypt hash used to equalize timing when the username is wrong.
    /// Prevents timing side-channels that reveal whether a username exists.
    pub dummy_hash: String,
    /// Expected username.
    pub username: String,
    /// JWT signing secret.
    pub jwt_secret: String,
    /// Token expiry in hours.
    pub jwt_expiry_hours: u64,
    /// Emit the `Secure` attribute on the issued/cleared auth cookie.
    pub cookie_secure: bool,
}

impl LocalAuthState {
    /// Build `LocalAuthState` from a `LocalAuthConfig`, computing the bcrypt hash.
    pub fn from_config(config: &LocalAuthConfig) -> Arc<Self> {
        let password_hash = bcrypt::hash(&config.password, bcrypt::DEFAULT_COST)
            .expect("bcrypt::hash should not fail with a valid password");
        let dummy_hash = bcrypt::hash("__dummy__", bcrypt::DEFAULT_COST).expect("bcrypt::hash should not fail");

        Arc::new(Self {
            password_hash,
            dummy_hash,
            username: config.username.clone(),
            jwt_secret: config.jwt_secret.clone(),
            jwt_expiry_hours: config.jwt_expiry_hours,
            cookie_secure: config.cookie_secure,
        })
    }
}
