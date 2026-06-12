//! Local authentication module.
//!
//! Provides built-in username/password authentication with JWT for the Admin API.
//! Credentials come only from configuration: the `[local_auth]` section in the
//! controller config file, the `EDGION_ADMIN_*` environment variables, or `[auth]`
//! OIDC. They are never auto-generated. When nothing valid is configured, no auth
//! provider loads and the Admin API returns 503 fail-close (mirrors Center).
//!
//! ## Sub-modules
//!
//! - **config**: `LocalAuthConfig` — YAML-based configuration and defaults.
//! - **handlers**: Login and me HTTP handlers.
//! - **middleware**: `LocalAuthState` shared with `unified_auth` for local
//!   validation fallback. The actual middleware lives in `unified_auth`.

pub mod config;
pub mod handlers;
pub mod middleware;

pub use config::LocalAuthConfig;
pub use handlers::{
    login_handler, logout_handler, me_handler, AuthStatus, AuthStatusResponse, LoginRequest, LoginResponse,
};
pub use middleware::LocalAuthState;
