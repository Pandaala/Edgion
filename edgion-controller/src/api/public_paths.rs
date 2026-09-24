//! Canonical source of truth for "public" (unauthenticated) HTTP paths.
//!
//! Referenced by the controller's authorization layer (`authz_middleware`) and
//! the token-auth 503 gate. Centralizing them here prevents drift that could let
//! a fail-closed authz change 403 the public `/health` probe or block the
//! `/api/v1/auth/status` readiness endpoint.
//!
//! # Security boundary (fr-cauth-01)
//!
//! The authorization skip-set is the **static base ONLY** — probes plus the
//! single public auth endpoint (`/api/v1/auth/status`). There is no
//! operator-configurable skip list, so a business route can never be made
//! public by configuration.

/// Public liveness/readiness/metrics probes. Unauthenticated by convention
/// (K8s controller-manager style). Exact-match.
pub const PUBLIC_PROBE_PATHS: &[&str] = &["/health", "/ready", "/metrics"];

/// Public auth endpoint reachable regardless of token readiness:
/// `status` surfaces whether auth is ready. Exact-match.
pub const PUBLIC_AUTH_ENDPOINTS: &[&str] = &["/api/v1/auth/status"];

/// True if `path` is a public probe (`/health`, `/ready`, `/metrics`).
pub fn is_public_probe(path: &str) -> bool {
    PUBLIC_PROBE_PATHS.contains(&path)
}

/// True if `path` is the public auth endpoint (`/api/v1/auth/status`, exact-match).
pub fn is_public_auth_endpoint(path: &str) -> bool {
    PUBLIC_AUTH_ENDPOINTS.contains(&path)
}

/// The **static** public set used by the authorization layer's skip check and
/// the token-auth 503 gate. Probes + the public auth endpoint.
pub fn is_static_public(path: &str) -> bool {
    is_public_probe(path) || is_public_auth_endpoint(path)
}

#[cfg(test)]
#[path = "../../tests/unit/api/public_paths_test.rs"]
mod tests;
