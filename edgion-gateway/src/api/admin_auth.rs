//! Gateway admin API authentication: a single shared bearer token, checked with
//! a constant-time digest comparison. See
//! `skills/04-review/admin-api/regression-guards/gateway-admin-bearer-boundary.md`.
//!
//! The trust unit is one operator + one gateway pod. There is no login, no JWT,
//! no cookie, no user model. The token is resolved once at startup and stored
//! here only as a SHA-256 digest (never the plaintext).

/// Minimum recommended token length. Shorter tokens are accepted but logged at
/// WARN — we do not reject startup (avoids CrashLoop on a borderline value).
const MIN_TOKEN_LEN: usize = 32;

/// Outcome of evaluating a request against the configured token.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AdminAuthOutcome {
    /// A correct token was presented — let the request through.
    Proceed,
    /// A token is configured but the request's token is missing or wrong (401).
    Unauthorized,
    /// No token is configured server-side — fail closed (503).
    Unconfigured,
}

use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Auth middleware state: the SHA-256 digest of the configured token, or `None`
/// when no token is configured (fail-close). Cheap to clone (32-byte `Copy`
/// array); built once at startup. We keep only the digest, never the plaintext
/// token.
#[derive(Clone)]
pub(super) struct AdminAuthState {
    token_digest: Option<[u8; 32]>,
}

impl AdminAuthState {
    /// Build the state from the resolved token. Trims; empty/whitespace-only is
    /// treated as unconfigured. Logs a WARN (not a hard error) when a configured
    /// token is shorter than the recommended minimum.
    pub(super) fn from_token(token: Option<String>) -> Self {
        let token = token.as_deref().map(str::trim).filter(|t| !t.is_empty());
        match token {
            None => Self { token_digest: None },
            Some(t) => {
                if t.len() < MIN_TOKEN_LEN {
                    tracing::warn!(
                        component = "admin_api_gateway",
                        event = "weak_admin_token",
                        min_len = MIN_TOKEN_LEN,
                        "Gateway admin token is shorter than the recommended minimum length"
                    );
                }
                let mut digest = [0u8; 32];
                digest.copy_from_slice(&Sha256::digest(t.as_bytes()));
                Self {
                    token_digest: Some(digest),
                }
            }
        }
    }

    /// Borrow the configured digest, if any.
    fn digest(&self) -> Option<&[u8; 32]> {
        self.token_digest.as_ref()
    }
}

/// Extract the bearer token from an `Authorization` header value. The scheme is
/// matched case-insensitively and MUST be separated from the token by
/// whitespace; surrounding whitespace is trimmed. Returns `None` for a missing
/// header, a non-Bearer scheme, a missing separator (e.g. `"Bearerxyz"`), or an
/// empty token.
fn parse_bearer_token(header: Option<&str>) -> Option<&str> {
    let header = header?.trim();
    // Require a whitespace separator between scheme and token so a malformed
    // value like "Bearerxyz" (no delimiter) is rejected rather than parsed as
    // the token "xyz".
    let (scheme, rest) = header.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Decide the outcome for a request. Pure and side-effect-free so it is fully
/// unit-testable. `token_digest` is `None` when no token is configured.
pub(super) fn evaluate(token_digest: Option<&[u8; 32]>, auth_header: Option<&str>) -> AdminAuthOutcome {
    let Some(expected) = token_digest else {
        return AdminAuthOutcome::Unconfigured;
    };
    let Some(presented) = parse_bearer_token(auth_header) else {
        return AdminAuthOutcome::Unauthorized;
    };
    let presented_digest = Sha256::digest(presented.as_bytes());
    // Constant-time over fixed-size digests: neither length nor content of the
    // configured token varies the timing.
    if bool::from(presented_digest.as_slice().ct_eq(expected.as_slice())) {
        AdminAuthOutcome::Proceed
    } else {
        AdminAuthOutcome::Unauthorized
    }
}

/// Axum middleware guarding every admin route. Reads `Authorization`, evaluates
/// it against the configured token digest, and either proceeds or rejects.
///
/// Never logs the `Authorization` header value — only presence and the result.
pub(super) async fn admin_auth_middleware(
    State(state): State<AdminAuthState>,
    request: Request,
    next: Next,
) -> Response {
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match evaluate(state.digest(), auth_header) {
        AdminAuthOutcome::Proceed => next.run(request).await,
        AdminAuthOutcome::Unauthorized => {
            let mut resp = (
                StatusCode::UNAUTHORIZED,
                Json(super::ApiResponse::<()>::err_body(
                    "Unauthorized: a valid bearer token is required".to_string(),
                )),
            )
                .into_response();
            resp.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(r#"Bearer realm="edgion-gateway-admin""#),
            );
            resp
        }
        AdminAuthOutcome::Unconfigured => {
            let mut resp = (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(super::ApiResponse::<()>::err_body(
                    "Admin API unavailable: no admin token configured".to_string(),
                )),
            )
                .into_response();
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("10"));
            resp
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/api/admin_auth_test.rs"]
mod tests;
