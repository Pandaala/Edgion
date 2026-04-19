//! Unified authentication middleware for the Admin API.
//!
//! Combines OIDC-based JWT authentication and local (HS256) JWT authentication
//! into a single Axum middleware. When both providers are configured, the
//! middleware tries OIDC validation first and falls back to local validation,
//! allowing tokens from either provider to coexist on the same endpoint.
//!
//! ## Motivation
//!
//! Previously, OIDC and local auth were separate middleware layers. When both
//! were applied (OIDC outer, local inner), the OIDC layer would reject local
//! tokens before the local layer ever had a chance to validate them. This
//! module solves that by performing a sequential try-OIDC-then-try-local
//! strategy in a single middleware function.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use axum::middleware;
//! use crate::core::common::unified_auth::{UnifiedAuthState, unified_auth_middleware};
//!
//! let state = UnifiedAuthState::from_configs(
//!     config.auth.as_ref(),
//!     config.local_auth.as_ref(),
//!     "controller",
//! );
//! let auth_layer = middleware::from_fn_with_state(state, unified_auth_middleware);
//! router = router.layer(auth_layer);
//! ```

use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

use super::auth::jwt_middleware::AuthMiddlewareState;
use super::auth::AdminAuthConfig;
use super::local_auth::config::LocalAuthConfig;
use super::local_auth::handlers::LocalAuthClaims;
use super::local_auth::middleware::LocalAuthState;
use crate::core::gateway::plugins::http::common::jwt_common::{
    map_jwt_decode_error, resolve_algorithm_policy, select_jwk, validate_token_alg, JwkSelectError,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Indicates which authentication provider successfully validated the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthProvider {
    /// Token validated via OIDC (external identity provider).
    Oidc,
    /// Token validated via local HS256 secret.
    Local,
}

/// Claims extracted from a successfully validated token, regardless of provider.
///
/// Downstream handlers can extract this via `Extension<UnifiedAuthClaims>`.
#[derive(Debug, Clone)]
pub struct UnifiedAuthClaims {
    /// Which provider validated this token.
    pub provider: AuthProvider,
    /// The subject (`sub` claim), if present.
    pub sub: Option<String>,
    /// The issuer (`iss` claim), if present.
    pub iss: Option<String>,
    /// Full claims as a JSON value (for scope/role inspection).
    pub claims: serde_json::Value,
}

/// Shared state for the unified authentication middleware.
///
/// Holds optional references to both the OIDC and local auth states, plus the
/// merged set of paths that should bypass authentication entirely.
pub struct UnifiedAuthState {
    /// OIDC auth state (None when OIDC is not configured or disabled).
    pub oidc: Option<Arc<AuthMiddlewareState>>,
    /// Local auth state (None when local auth is not configured or disabled).
    pub local: Option<Arc<LocalAuthState>>,
    /// Paths that bypass authentication (union of both providers' skip_paths).
    pub skip_paths: HashSet<String>,
}

impl UnifiedAuthState {
    /// Build a `UnifiedAuthState` from optional OIDC and local auth configs.
    ///
    /// Each provider is independently validated and enabled/disabled. The
    /// `skip_paths` sets from both configs are merged together with the
    /// mandatory `/health` and `/ready` paths.
    pub fn from_configs(
        auth_config: Option<&AdminAuthConfig>,
        local_auth_config: Option<&LocalAuthConfig>,
        component: &str,
    ) -> Arc<Self> {
        let mut skip_paths = HashSet::new();
        // Always skip health/ready probes and the Prometheus scrape endpoint.
        // `/metrics` follows the K8s controller-manager convention of being
        // unauthenticated: scrapers usually cannot send bearer tokens.
        skip_paths.insert("/health".to_string());
        skip_paths.insert("/ready".to_string());
        skip_paths.insert("/metrics".to_string());

        let oidc = match auth_config {
            Some(cfg) if cfg.enabled => {
                if let Some(err) = cfg.validate() {
                    tracing::error!(
                        component = component,
                        error = err,
                        "Invalid auth config, skipping OIDC auth"
                    );
                    None
                } else {
                    for p in &cfg.skip_paths {
                        skip_paths.insert(p.clone());
                    }
                    tracing::info!(
                        component = component,
                        discovery = %cfg.discovery,
                        "OIDC authentication enabled"
                    );
                    Some(AuthMiddlewareState::from_config(cfg))
                }
            }
            Some(_) => {
                tracing::info!(component = component, "OIDC authentication disabled by config");
                None
            }
            None => {
                tracing::info!(component = component, "OIDC authentication not configured");
                None
            }
        };

        let local = match local_auth_config {
            Some(cfg) if cfg.enabled => {
                if let Some(err) = cfg.validate() {
                    tracing::error!(
                        component = component,
                        error = err,
                        "Invalid local_auth config, skipping local auth"
                    );
                    None
                } else {
                    for p in &cfg.skip_paths {
                        skip_paths.insert(p.clone());
                    }
                    tracing::info!(
                        component = component,
                        username = %cfg.username,
                        "Local authentication enabled"
                    );
                    Some(LocalAuthState::from_config(cfg))
                }
            }
            Some(_) => {
                tracing::info!(component = component, "Local authentication disabled by config");
                None
            }
            None => {
                tracing::info!(component = component, "Local authentication not configured");
                None
            }
        };

        Arc::new(Self {
            oidc,
            local,
            skip_paths,
        })
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Unified Axum middleware that tries OIDC auth first, then falls back to local.
///
/// # Skip rules
///
/// The following requests bypass authentication entirely:
/// - Paths in `state.skip_paths` (health/ready probes, etc.)
/// - Paths starting with `/api/v1/auth/` (login endpoint)
/// - Paths starting with `/configserver/` (edgion-cli compatibility)
/// - When neither OIDC nor local auth is configured (no-auth mode)
///
/// # Validation order
///
/// 1. Try OIDC validation (if configured). On success → inject `UnifiedAuthClaims`
///    with `provider = Oidc`.
/// 2. If OIDC fails or is not configured, try local validation (if configured).
///    On success → inject `UnifiedAuthClaims` with `provider = Local`.
/// 3. If both fail → 401 Unauthorized.
pub async fn unified_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<UnifiedAuthState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // If neither provider is configured, pass through (no-auth mode).
    if state.oidc.is_none() && state.local.is_none() {
        return next.run(request).await;
    }

    let path = request.uri().path().to_string();

    // Skip authentication for configured paths (health/ready probes, etc.).
    if state.skip_paths.contains(&path) {
        return next.run(request).await;
    }

    // Skip authentication for public auth endpoints only.
    if path == "/api/v1/auth/login" || path == "/api/v1/auth/logout" || path == "/api/v1/auth/status" {
        return next.run(request).await;
    }

    // Extract Bearer token, falling back to httpOnly cookie (browser auth).
    let token = match extract_bearer_token(&request) {
        Some(t) => t,
        None => match extract_cookie_token(&request) {
            Some(t) => t,
            None => {
                return auth_error_response(StatusCode::UNAUTHORIZED, "Missing or invalid Authorization header");
            }
        },
    };

    // Try OIDC validation first.
    let mut oidc_error: Option<String> = None;
    if let Some(ref oidc_state) = state.oidc {
        match try_oidc_validation(oidc_state, &token).await {
            Ok(claims) => {
                tracing::debug!(
                    component = "unified_auth",
                    provider = "oidc",
                    sub = ?claims.sub,
                    "Token validated via OIDC"
                );
                let mut request = request;
                request.extensions_mut().insert(claims);
                return next.run(request).await;
            }
            Err(err) => {
                tracing::debug!(
                    component = "unified_auth",
                    provider = "oidc",
                    error = %err,
                    "OIDC validation failed, trying local"
                );
                oidc_error = Some(err);
            }
        }
    }

    // Fall back to local validation.
    let mut local_error: Option<String> = None;
    if let Some(ref local_state) = state.local {
        match try_local_validation(local_state, &token) {
            Ok(claims) => {
                tracing::debug!(
                    component = "unified_auth",
                    provider = "local",
                    sub = ?claims.sub,
                    "Token validated via local auth"
                );
                let mut request = request;
                request.extensions_mut().insert(claims);
                return next.run(request).await;
            }
            Err(err) => {
                tracing::debug!(
                    component = "unified_auth",
                    provider = "local",
                    error = %err,
                    "Local validation also failed"
                );
                local_error = Some(err);
            }
        }
    }

    // Both failed — build an informative error message.
    let message = match (oidc_error, local_error) {
        (Some(oidc_err), Some(local_err)) => {
            format!("Authentication failed: OIDC: {}; Local: {}", oidc_err, local_err)
        }
        (Some(oidc_err), None) => {
            format!("Authentication failed (OIDC): {}", oidc_err)
        }
        (None, Some(local_err)) => {
            format!("Authentication failed (Local): {}", local_err)
        }
        (None, None) => "Authentication failed: no auth provider available".to_string(),
    };

    auth_error_response(StatusCode::UNAUTHORIZED, &message)
}

// ---------------------------------------------------------------------------
// OIDC validation helper
// ---------------------------------------------------------------------------

/// Attempt to validate a JWT token against the OIDC provider's JWKS.
async fn try_oidc_validation(state: &AuthMiddlewareState, token: &str) -> Result<UnifiedAuthClaims, String> {
    // Decode header to get algorithm + kid.
    let header = decode_header(token).map_err(|e| format!("Invalid token header: {}", e))?;

    // Resolve algorithm policy.
    let allowed_algs_strs: Option<Vec<String>> = if state.allowed_algorithms.is_empty() {
        None
    } else {
        Some(state.allowed_algorithms.clone())
    };
    let (expected_alg, allowed_algs) =
        resolve_algorithm_policy(None, allowed_algs_strs.as_deref().map(|v| v as &[String])).map_err(|(_, msg)| msg)?;

    // Validate token algorithm against policy.
    validate_token_alg(header.alg, expected_alg, &allowed_algs).map_err(|(_, msg)| msg)?;

    // Fetch JWKS from the provider (with caching).
    let jwks = state
        .jwks_provider
        .get_jwks()
        .await
        .map_err(|e| format!("JWKS fetch failed: {}", e))?;

    // Select the matching JWK by kid / algorithm.
    let jwk = select_jwk(&header, &jwks, header.alg).map_err(|e| match e {
        JwkSelectError::KidNotFound => "Unknown signing key".to_string(),
        JwkSelectError::NoSuitableKey => "No suitable signing key".to_string(),
    })?;

    // Build decoding key from the selected JWK.
    let decoding_key = DecodingKey::from_jwk(&jwk).map_err(|e| format!("Decoding key error: {}", e))?;

    // Build validation parameters.
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
        // Validate against the discovery document's issuer if available.
        if let Some(issuer) = state.jwks_provider.issuer().await {
            validation.set_issuer(&[issuer]);
        }
    }

    // Decode and verify the token.
    let token_data = decode::<Value>(token, &decoding_key, &validation).map_err(|e| {
        let (_, msg) = map_jwt_decode_error(e);
        msg
    })?;

    let sub = token_data
        .claims
        .get("sub")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let iss = token_data
        .claims
        .get("iss")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(UnifiedAuthClaims {
        provider: AuthProvider::Oidc,
        sub,
        iss,
        claims: token_data.claims,
    })
}

// ---------------------------------------------------------------------------
// Local validation helper
// ---------------------------------------------------------------------------

/// Attempt to validate a JWT token using the local HS256 secret.
fn try_local_validation(state: &LocalAuthState, token: &str) -> Result<UnifiedAuthClaims, String> {
    let decoding_key = DecodingKey::from_secret(state.jwt_secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    // Disable audience validation — local tokens don't carry an `aud` claim.
    validation.validate_aud = false;

    let token_data = decode::<LocalAuthClaims>(token, &decoding_key, &validation)
        .map_err(|e| format!("Local JWT validation failed: {}", e))?;

    let local_claims = token_data.claims;
    let claims_json = serde_json::json!({
        "sub": &local_claims.sub,
        "iat": local_claims.iat,
        "exp": local_claims.exp,
    });

    Ok(UnifiedAuthClaims {
        provider: AuthProvider::Local,
        sub: Some(local_claims.sub),
        iss: None,
        claims: claims_json,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Error response body for authentication failures.
#[derive(Serialize)]
struct AuthError {
    error: String,
}

/// Build a JSON error response with the given status code and message.
fn auth_error_response(status: StatusCode, message: &str) -> Response {
    let body = Json(AuthError {
        error: message.to_string(),
    });
    (status, body).into_response()
}

/// Extract the Bearer token from the `Authorization` header.
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
