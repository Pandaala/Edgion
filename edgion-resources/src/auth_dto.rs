//! Client-facing authentication wire DTOs.
//!
//! These are the pure serde request/response types shared between the
//! controller's local-auth HTTP API and its clients (edgion-cli, dashboard).
//! They carry no server-side machinery (no bcrypt hashing, JWT signing, or axum
//! state) — that lives in `edgion_common::local_auth`. Placing the wire types in
//! the schema leaf crate lets any client deserialize the controller's auth
//! responses without linking the server-side auth stack.

use serde::{Deserialize, Serialize};

/// Login request body.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response body.
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    /// Signed JWT to use as `Authorization: Bearer <token>`.
    pub token: String,
    /// Token lifetime in seconds.
    pub expires_in: u64,
}

/// Me response body.
#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub username: String,
}

/// High-level auth configuration state of the controller.
///
/// Consumed by CLI / dashboard to decide how to authenticate without guessing
/// from a free-form `auth_method` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    /// At least one auth provider is ready to validate requests.
    Ready,
    /// Operator requires auth but no provider has been installed yet
    /// (typically the K8s Secret is still being fetched). Admin API
    /// requests currently return 503 Service Unavailable.
    Initializing,
    /// Operator did not configure authentication and does not expect it.
    Disabled,
}

/// Auth status response — tells clients what authentication the controller
/// expects and which providers are currently ready.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatusResponse {
    /// Whether the operator expects authentication to be enforced.
    pub auth_required: bool,
    /// Providers currently ready to validate requests. Possible values:
    /// `"oidc"`, `"local"`, `"cli"`. Empty when `status = initializing` or `disabled`.
    /// The `"cli"` provider is added Controller-side when at least one cli token is
    /// registered (see `Authorizer::cli_tokens_present`). It is not reported by the
    /// shared `unified_auth::build_status` because `unified_auth` must not import authz.
    pub active_providers: Vec<String>,
    /// High-level readiness status. CLIs should dispatch on this, not on
    /// `active_providers` alone.
    pub status: AuthStatus,
}
