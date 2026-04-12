//! Admin API authentication module.
//!
//! Provides JWT Bearer authentication for Controller and Center Admin APIs.
//! Uses OIDC discovery to fetch JWKS and validate tokens.
//!
//! ## Architecture
//!
//! - **config**: `AdminAuthConfig` — TOML-based configuration.
//! - **jwks_provider**: Background JWKS fetch + cache with singleflight refresh.
//! - **jwt_middleware**: Axum middleware layer for per-request JWT validation.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use crate::core::common::auth::{AdminAuthConfig, AuthMiddlewareState, jwt_auth_middleware};
//! use axum::middleware;
//!
//! // In controller/center config:
//! // [auth]
//! // discovery = "https://idp.example.com/.well-known/openid-configuration"
//! // audiences = ["edgion-admin"]
//!
//! if let Some(ref auth_config) = config.auth {
//!     if auth_config.enabled {
//!         let auth_state = AuthMiddlewareState::from_config(auth_config);
//!         let auth_layer = middleware::from_fn_with_state(auth_state, jwt_auth_middleware);
//!         router = router.layer(auth_layer);
//!     }
//! }
//! ```

pub mod config;
pub mod jwks_provider;
pub mod jwt_middleware;

pub use config::AdminAuthConfig;
pub use jwt_middleware::{jwt_auth_middleware, AuthClaims, AuthMiddlewareState};
