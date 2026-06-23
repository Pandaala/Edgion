//! Client-facing authentication wire DTOs.
//!
//! These are the pure serde types shared between the controller's Admin API and
//! its clients (edgion-cli). The controller authenticates with a single
//! pre-shared bearer token (Istio-style); the only auth endpoint is the public
//! `GET /api/v1/auth/status` probe, whose response is described here. Placing the
//! wire types in the schema leaf crate lets any client deserialize the status
//! response without linking the server-side auth stack.

use serde::{Deserialize, Serialize};

/// High-level auth configuration state of the controller.
///
/// Consumed by the CLI to decide whether a token is required and whether the
/// server is ready to validate one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    /// At least one token (admin or Center) is registered and ready.
    Ready,
    /// Auth is required but no token has been registered yet.
    Initializing,
    /// Auth is not enforced. (Currently unreachable: auth is mandatory.)
    Disabled,
}

/// Auth status response — tells clients what authentication the controller
/// expects and whether it is ready.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatusResponse {
    /// Whether the operator expects authentication to be enforced.
    pub auth_required: bool,
    /// Authentication mode. Currently always `"token"` (a pre-shared bearer
    /// token) when auth is required, or `"disabled"` when it is not.
    pub mode: String,
    /// High-level readiness status. CLIs should dispatch on this.
    pub status: AuthStatus,
}
