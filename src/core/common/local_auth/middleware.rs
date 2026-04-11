//! Axum middleware for local JWT Bearer authentication on Admin API endpoints.
//!
//! Extracts the `Authorization: Bearer <token>` header, validates the JWT
//! using the configured HS256 secret, and injects `LocalAuthClaims` into the
//! request extensions for downstream handlers.

use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;

use super::config::LocalAuthConfig;
use super::handlers::LocalAuthClaims;

/// Error response body for authentication failures.
#[derive(Serialize)]
struct AuthError {
    error: String,
}

fn auth_error_response(status: StatusCode, message: &str) -> Response {
    let body = Json(AuthError {
        error: message.to_string(),
    });
    (status, body).into_response()
}

/// State shared by the local auth middleware.
pub struct LocalAuthState {
    /// Bcrypt hash of the configured password.
    pub password_hash: String,
    /// Expected username.
    pub username: String,
    /// JWT signing secret.
    pub jwt_secret: String,
    /// Token expiry in hours.
    pub jwt_expiry_hours: u64,
    /// Paths that bypass auth checks.
    pub skip_paths: HashSet<String>,
}

impl LocalAuthState {
    /// Build `LocalAuthState` from a `LocalAuthConfig`, computing the bcrypt hash.
    pub fn from_config(config: &LocalAuthConfig) -> Arc<Self> {
        let password_hash = bcrypt::hash(&config.password, bcrypt::DEFAULT_COST)
            .expect("bcrypt::hash should not fail with a valid password");

        Arc::new(Self {
            password_hash,
            username: config.username.clone(),
            jwt_secret: config.jwt_secret.clone(),
            jwt_expiry_hours: config.jwt_expiry_hours,
            skip_paths: config.skip_paths_set(),
        })
    }
}

/// Axum middleware function for local JWT Bearer authentication.
///
/// - Paths in `skip_paths` (default: `/health`, `/ready`) are passed through unchanged.
/// - Paths starting with `/api/v1/auth/` (the login endpoint) are passed through unchanged.
/// - All other paths require a valid `Authorization: Bearer <HS256-JWT>` header.
pub async fn local_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<LocalAuthState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // Skip authentication for configured paths (health/ready probes).
    if state.skip_paths.contains(&path) {
        return next.run(request).await;
    }

    // Skip authentication for public auth endpoints only.
    if path == "/api/v1/auth/login"
        || path == "/api/v1/auth/logout"
        || path == "/api/v1/auth/status"
    {
        return next.run(request).await;
    }

    // Extract Bearer token, falling back to httpOnly cookie (browser auth).
    let token = match extract_bearer_token(&request) {
        Some(t) => t,
        None => {
            match extract_cookie_token(&request) {
                Some(t) => t,
                None => {
                    return auth_error_response(
                        StatusCode::UNAUTHORIZED,
                        "Missing or invalid Authorization header",
                    );
                }
            }
        }
    };

    // Decode and validate JWT.
    let decoding_key = DecodingKey::from_secret(state.jwt_secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    // Disable audience validation — local tokens don't have an `aud` claim.
    validation.validate_aud = false;

    match decode::<LocalAuthClaims>(&token, &decoding_key, &validation) {
        Ok(token_data) => {
            let mut request = request;
            request.extensions_mut().insert(token_data.claims);
            next.run(request).await
        }
        Err(e) => {
            tracing::debug!(component = "local_auth", error = %e, "JWT validation failed");
            auth_error_response(StatusCode::UNAUTHORIZED, "Invalid or expired token")
        }
    }
}

/// Extract the Bearer token from the Authorization header.
fn extract_bearer_token(request: &Request<Body>) -> Option<String> {
    let auth_header = request.headers().get(header::AUTHORIZATION)?;
    let auth_str = auth_header.to_str().ok()?;

    if auth_str.len() > 7 && auth_str[..7].eq_ignore_ascii_case("bearer ") {
        Some(auth_str[7..].to_string())
    } else {
        None
    }
}

/// Extract the JWT token from the `edgion_token` httpOnly cookie.
fn extract_cookie_token(request: &Request<Body>) -> Option<String> {
    let cookie_header = request.headers().get(header::COOKIE)?;
    let cookie_str = cookie_header.to_str().ok()?;
    for part in cookie_str.split(';') {
        let trimmed = part.trim();
        if let Some(value) = trimmed.strip_prefix("edgion_token=") {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}
