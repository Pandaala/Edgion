//! Controller token authentication middleware — the sole Admin API authn layer.
//!
//! On a recognized pre-shared bearer token (Bearer-only, case-insensitive) it
//! injects the token's `Restricted` role and `CliTokenIdentity` (for deny-audit).
//! Both the local admin token (allow-all wildcard policy) and the scoped Center
//! cli tokens live in the same `Authorizer` map, so this one middleware
//! authenticates both. An unrecognized / absent token injects nothing; the
//! downstream `authz_layer` then 403s the (role-less) request — except on the
//! static public paths, which it skips.
//!
//! # Bearer-only
//!
//! This middleware reads only the `Authorization` header (never a cookie). The
//! token-only model has no cookie/session concept.

use std::sync::Arc;

use axum::{body::Body, extract::State, middleware::Next, response::Response};
use http::Request;

use crate::authz::Authorizer;

/// Audit identity for a token-authenticated request.
///
/// Carries the human-readable token name (the local admin token's reserved
/// `"admin"`, or a `cli_tokens[].name` from config) for deny-log structured
/// fields. Inserted alongside `Role::Restricted` by `cli_token_pre_authn`.
#[derive(Clone)]
pub struct CliTokenIdentity {
    pub name: String,
}

/// Extract the Bearer token value from the `Authorization` header.
///
/// Matches `Bearer ` case-insensitively. Returns `None` when the header is
/// absent, is not a valid UTF-8 string, or does not start with `bearer `.
fn extract_bearer(req: &Request<Body>) -> Option<&str> {
    let val = req.headers().get(http::header::AUTHORIZATION)?.to_str().ok()?;
    if val.len() > 7 && val[..7].eq_ignore_ascii_case("bearer ") {
        Some(&val[7..])
    } else {
        None
    }
}

/// Controller token authentication middleware.
///
/// Layer order: 503 gate (outer) → `cli_token_pre_authn` → `authz_layer`
/// (route_layer) → handler.
///
/// A `Bearer` token present in the `Authorization` header is looked up via
/// `Authorizer::cli_lookup` (SHA-256 hash comparison). On a hit it injects
/// `Role::Restricted` and `CliTokenIdentity`. On a miss or no bearer it injects
/// nothing — `authz_layer` then denies any non-public route.
pub async fn cli_token_pre_authn(
    State(authorizer): State<Arc<Authorizer>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(tok) = extract_bearer(&req) {
        if let Some((role, name)) = authorizer.cli_lookup(tok) {
            req.extensions_mut().insert(role);
            req.extensions_mut().insert(CliTokenIdentity { name });
        }
    }

    next.run(req).await
}

#[cfg(test)]
#[path = "../../tests/unit/api/cli_token_middleware_test.rs"]
mod tests;
