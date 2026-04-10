//! Axum middleware for JWT Bearer authentication on Admin API endpoints.
//!
//! Extracts the `Authorization: Bearer <token>` header, validates the JWT
//! against the JWKS obtained from the configured OIDC provider, and injects
//! the verified claims into the request extensions for downstream handlers.

use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

use super::jwks_provider::JwksProvider;
use crate::core::gateway::plugins::http::common::jwt_common::{
    map_jwt_decode_error, resolve_algorithm_policy, select_jwk, validate_token_alg, JwkSelectError,
};

/// Verified JWT claims injected into request extensions.
///
/// Downstream handlers can extract this via `Extension<AuthClaims>`.
#[derive(Debug, Clone)]
pub struct AuthClaims {
    /// The subject (`sub` claim).
    pub sub: Option<String>,
    /// The issuer (`iss` claim).
    pub iss: Option<String>,
    /// Full claims as JSON (for scope/role checking).
    pub claims: Value,
}

/// Error response body for authentication failures.
#[derive(serde::Serialize)]
struct AuthError {
    error: String,
}

fn auth_error_response(status: StatusCode, message: &str) -> Response {
    let body = Json(AuthError {
        error: message.to_string(),
    });
    (status, body).into_response()
}

/// Axum middleware function for JWT Bearer authentication.
///
/// Usage:
/// ```rust,ignore
/// use axum::middleware;
///
/// let auth_layer = middleware::from_fn_with_state(
///     auth_state.clone(),
///     jwt_auth_middleware,
/// );
/// router.layer(auth_layer)
/// ```
pub async fn jwt_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AuthMiddlewareState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // Skip authentication for configured paths (health/ready probes).
    if state.skip_paths.contains(&path) {
        return next.run(request).await;
    }

    // Extract Bearer token.
    let token = match extract_bearer_token(&request) {
        Some(t) => t,
        None => {
            return auth_error_response(
                StatusCode::UNAUTHORIZED,
                "Missing or invalid Authorization header",
            );
        }
    };

    // Validate JWT.
    match validate_jwt(&state, &token).await {
        Ok(claims) => {
            let mut request = request;
            request.extensions_mut().insert(claims);
            next.run(request).await
        }
        Err((status, message)) => auth_error_response(status, &message),
    }
}

/// State shared by the JWT auth middleware.
pub struct AuthMiddlewareState {
    pub jwks_provider: Arc<JwksProvider>,
    pub audiences: Vec<String>,
    pub issuers: Vec<String>,
    pub allowed_algorithms: Vec<String>,
    pub clock_skew_seconds: u64,
    pub skip_paths: HashSet<String>,
}

impl AuthMiddlewareState {
    /// Build from AdminAuthConfig.
    pub fn from_config(config: &super::config::AdminAuthConfig) -> Arc<Self> {
        let jwks_provider = JwksProvider::new(config);
        Arc::new(Self {
            jwks_provider,
            audiences: config.audiences.clone(),
            issuers: config.issuers.clone(),
            allowed_algorithms: config.allowed_algorithms.clone(),
            clock_skew_seconds: config.clock_skew_seconds,
            skip_paths: config.skip_paths_set(),
        })
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

/// Validate a JWT token and return the claims.
async fn validate_jwt(
    state: &AuthMiddlewareState,
    token: &str,
) -> Result<AuthClaims, (StatusCode, String)> {
    // Decode header to get algorithm + kid.
    let header = decode_header(token).map_err(|e| {
        tracing::debug!(component = "admin_auth", error = %e, "Failed to decode JWT header");
        (StatusCode::UNAUTHORIZED, "Invalid token header".to_string())
    })?;

    // Resolve algorithm policy.
    let allowed_algs_strs: Option<Vec<String>> = if state.allowed_algorithms.is_empty() {
        None
    } else {
        Some(state.allowed_algorithms.clone())
    };
    let (expected_alg, allowed_algs) = resolve_algorithm_policy(
        None, // No single forced algorithm for admin API
        allowed_algs_strs.as_deref().map(|v| v as &[String]),
    )
    .map_err(|(_, msg)| (StatusCode::UNAUTHORIZED, msg))?;

    // Validate token algorithm.
    validate_token_alg(header.alg, expected_alg, &allowed_algs)
        .map_err(|(_, msg)| (StatusCode::UNAUTHORIZED, msg))?;

    // Fetch JWKS.
    let jwks = state
        .jwks_provider
        .get_jwks()
        .await
        .map_err(|e| {
            tracing::error!(component = "admin_auth", error = %e, "Failed to fetch JWKS");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Authentication service unavailable".to_string(),
            )
        })?;

    // Select matching JWK.
    let jwk = select_jwk(&header, &jwks, header.alg).map_err(|e| match e {
        JwkSelectError::KidNotFound => {
            tracing::debug!(component = "admin_auth", kid = ?header.kid, "JWK kid not found in JWKS");
            (StatusCode::UNAUTHORIZED, "Unknown signing key".to_string())
        }
        JwkSelectError::NoSuitableKey => {
            tracing::debug!(component = "admin_auth", alg = ?header.alg, "No suitable JWK for algorithm");
            (
                StatusCode::UNAUTHORIZED,
                "No suitable signing key".to_string(),
            )
        }
    })?;

    // Build decoding key from JWK.
    let decoding_key = DecodingKey::from_jwk(&jwk).map_err(|e| {
        tracing::error!(component = "admin_auth", error = %e, "Failed to build decoding key from JWK");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Authentication configuration error".to_string(),
        )
    })?;

    // Build validation.
    let mut validation = Validation::new(header.alg);
    validation.leeway = state.clock_skew_seconds;
    validation.validate_exp = true;
    validation.validate_nbf = true;

    if !state.audiences.is_empty() {
        validation.set_audience(&state.audiences);
    } else {
        validation.validate_aud = false;
    }

    if !state.issuers.is_empty() {
        validation.set_issuer(&state.issuers);
    } else {
        // Validate against discovery issuer if available.
        if let Some(issuer) = state.jwks_provider.issuer().await {
            validation.set_issuer(&[issuer]);
        }
    }

    // Decode and verify.
    let token_data = decode::<Value>(token, &decoding_key, &validation)
        .map_err(|e| {
            let (_, msg) = map_jwt_decode_error(e);
            (StatusCode::UNAUTHORIZED, msg)
        })?;

    let sub = token_data.claims.get("sub").and_then(|v| v.as_str()).map(|s| s.to_string());
    let iss = token_data.claims.get("iss").and_then(|v| v.as_str()).map(|s| s.to_string());

    Ok(AuthClaims {
        sub,
        iss,
        claims: token_data.claims,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    #[test]
    fn test_extract_bearer_token_valid() {
        let request = Request::builder()
            .header("Authorization", "Bearer eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9")
            .body(Body::empty())
            .unwrap();
        let token = extract_bearer_token(&request).unwrap();
        assert_eq!(token, "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9");
    }

    #[test]
    fn test_extract_bearer_token_case_insensitive() {
        let request = Request::builder()
            .header("Authorization", "BEARER my_token")
            .body(Body::empty())
            .unwrap();
        let token = extract_bearer_token(&request).unwrap();
        assert_eq!(token, "my_token");
    }

    #[test]
    fn test_extract_bearer_token_missing() {
        let request = Request::builder().body(Body::empty()).unwrap();
        assert!(extract_bearer_token(&request).is_none());
    }

    #[test]
    fn test_extract_bearer_token_basic_auth() {
        let request = Request::builder()
            .header("Authorization", "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();
        assert!(extract_bearer_token(&request).is_none());
    }

    #[test]
    fn test_extract_bearer_token_empty_token() {
        let request = Request::builder()
            .header("Authorization", "Bearer ")
            .body(Body::empty())
            .unwrap();
        // "Bearer " has length 7, token part is empty string
        let token = extract_bearer_token(&request);
        assert_eq!(token, Some(String::new()));
    }
}
