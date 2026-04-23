//! Local authentication module.
//!
//! Provides built-in username/password authentication with JWT for the Admin API.
//! When no OIDC (`[auth]`) section is present in the controller config, this is
//! automatically enabled with auto-generated credentials persisted to a K8s
//! Secret (`edgion-admin-credentials` in namespace `edgion-system`).
//!
//! ## Sub-modules
//!
//! - **config**: `LocalAuthConfig` — TOML-based configuration and defaults.
//! - **handlers**: Login and me HTTP handlers.
//! - **middleware**: Axum middleware that validates JWT Bearer tokens.

pub mod config;
pub mod handlers;
pub mod middleware;
pub mod retry;
pub mod secret;

pub use config::LocalAuthConfig;
pub use handlers::{
    login_handler, logout_handler, me_handler, AuthStatus, AuthStatusResponse, LoginRequest, LoginResponse,
};
pub use middleware::{local_auth_middleware, LocalAuthState};
