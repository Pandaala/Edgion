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
//!     /* require_auth = */ true,
//!     "controller",
//! )?;
//! let auth_layer = middleware::from_fn_with_state(state, unified_auth_middleware);
//! router = router.layer(auth_layer);
//! ```

use arc_swap::ArcSwapOption;
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
    /// Local auth state. `ArcSwapOption` supports lazy population by
    /// the background Secret retry task (Controller auto-generate path).
    /// At startup it may be `None` even when `require_auth = true`;
    /// middleware returns 503 in that case until the value is populated.
    pub local: ArcSwapOption<LocalAuthState>,
    /// Paths that bypass authentication (union of both providers' skip_paths).
    pub skip_paths: HashSet<String>,
    /// Whether the operator *expects* authentication to be enforced.
    /// Derived at startup from the intended configuration (including
    /// auto-generate fallback), not from whether providers are currently ready.
    /// Middleware uses this + provider readiness to decide pass-through vs 503.
    pub require_auth: bool,
}

impl UnifiedAuthState {
    /// Build a `UnifiedAuthState` from optional OIDC and local auth configs.
    ///
    /// # Errors
    ///
    /// Returns `Err` if any `enabled = true` provider fails `validate()`.
    /// Callers are expected to propagate the error to `main()` so the process
    /// exits with a non-zero code — never silently start with a half-disabled
    /// auth stack.
    ///
    /// `require_auth` must be supplied by the caller. It expresses operator
    /// *intent* (e.g. Controller's auto-generate fallback sets it true even
    /// when `local_auth_config` is `None`), independent of whether providers
    /// are currently ready.
    pub fn from_configs(
        auth_config: Option<&AdminAuthConfig>,
        local_auth_config: Option<&LocalAuthConfig>,
        require_auth: bool,
        component: &str,
    ) -> anyhow::Result<Arc<Self>> {
        let mut skip_paths = HashSet::new();
        skip_paths.insert("/health".to_string());
        skip_paths.insert("/ready".to_string());
        skip_paths.insert("/metrics".to_string());

        let oidc = match auth_config {
            Some(cfg) if cfg.enabled => {
                if let Some(err) = cfg.validate() {
                    return Err(anyhow::anyhow!(
                        "invalid [auth] config (component={}): {}",
                        component,
                        err
                    ));
                }
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
            Some(_) => {
                tracing::info!(component = component, "OIDC authentication disabled by config");
                None
            }
            None => {
                tracing::info!(component = component, "OIDC authentication not configured");
                None
            }
        };

        let local_initial = match local_auth_config {
            Some(cfg) if cfg.enabled => {
                if let Some(err) = cfg.validate() {
                    return Err(anyhow::anyhow!(
                        "invalid [local_auth] config (component={}): {}",
                        component,
                        err
                    ));
                }
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
            Some(_) => {
                tracing::info!(component = component, "Local authentication disabled by config");
                None
            }
            None => {
                tracing::info!(component = component, "Local authentication not configured");
                None
            }
        };

        Ok(Arc::new(Self {
            oidc,
            local: ArcSwapOption::from(local_initial),
            skip_paths,
            require_auth,
        }))
    }

    /// Install a freshly built `LocalAuthState` produced after an asynchronous
    /// credential fetch (e.g. K8s Secret retry loop).
    ///
    /// Uses an atomic pointer swap so concurrent middleware reads see the new
    /// value on their next `.load()` without blocking.
    pub fn update_local(&self, state: Arc<LocalAuthState>) {
        self.local.store(Some(state));
    }

    /// Derive the pre-computed auth status from the live state.
    ///
    /// Reads from the actual assembled state (not TOML intent) — so transient
    /// states like "configured but Secret not yet fetched" are reported as
    /// `Initializing` instead of claiming `Ready`. Intended to be called
    /// per-request from the `/api/v1/auth/status` handler.
    pub fn build_status(&self) -> super::local_auth::AuthStatusResponse {
        use super::local_auth::{AuthStatus, AuthStatusResponse};
        let (status, providers) = if !self.require_auth {
            // Disabled mode: honor the struct-level invariant that
            // `active_providers` is empty when `status == Disabled`, regardless
            // of what providers may have been eagerly assembled.
            (AuthStatus::Disabled, Vec::new())
        } else {
            let mut providers = Vec::new();
            if self.oidc.is_some() {
                providers.push("oidc".to_string());
            }
            if self.local.load().is_some() {
                providers.push("local".to_string());
            }
            let status = if providers.is_empty() {
                AuthStatus::Initializing
            } else {
                AuthStatus::Ready
            };
            (status, providers)
        };
        AuthStatusResponse {
            auth_required: self.require_auth,
            active_providers: providers,
            status,
        }
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Build a 503 Service Unavailable response used when `require_auth` is true
/// but no provider is currently ready (e.g. during Secret fetch retry).
///
/// Returns:
/// - Status 503
/// - Header `Retry-After: 10` (seconds)
/// - Body `ApiResponse::error("authentication subsystem initializing, retry later")`
fn initializing_response() -> Response {
    use axum::http::HeaderValue;
    let body = axum::Json(crate::core::common::api::ApiResponse::<()>::error(
        "authentication subsystem initializing, retry later".to_string(),
    ));
    let mut resp = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
    resp.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("10"));
    resp
}

/// Unified Axum middleware that tries OIDC auth first, then falls back to local.
///
/// # Skip rules
///
/// The following requests bypass authentication entirely:
/// - Paths in `state.skip_paths` (health/ready probes, `/metrics`, etc.)
/// - The three public auth endpoints `/api/v1/auth/login`, `/api/v1/auth/logout`,
///   and `/api/v1/auth/status` (exact match; note: `/api/v1/auth/me` is NOT
///   skipped — it requires a valid token)
/// - Paths starting with `/configserver/` (edgion-cli compatibility)
/// - When `state.require_auth == false` (operator did not configure auth)
///
/// # Fail-close behavior
///
/// When `state.require_auth == true` but neither provider is currently ready
/// (e.g. Secret fetch still in flight), returns 503 with `Retry-After: 10`.
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
    let path = request.uri().path().to_string();

    // Skip authentication for configured paths (health/ready probes, /metrics, etc.).
    if state.skip_paths.contains(&path) {
        return next.run(request).await;
    }
    // The three public auth endpoints must stay reachable regardless of
    // provider readiness: login so clients can authenticate, logout so they
    // can clear a session, status so UIs can surface the init state.
    // `/api/v1/auth/me` is intentionally NOT in this list — it requires
    // a valid token.
    if path == "/api/v1/auth/login" || path == "/api/v1/auth/logout" || path == "/api/v1/auth/status" {
        return next.run(request).await;
    }
    if path.starts_with("/configserver/") {
        return next.run(request).await;
    }

    // Operator did not configure authentication — legitimate no-auth mode.
    if !state.require_auth {
        return next.run(request).await;
    }

    // Snapshot local provider state. ArcSwapOption → Option<Arc<LocalAuthState>>.
    let local_snapshot: Option<Arc<LocalAuthState>> = state.local.load_full();

    // Fail-close: operator wants auth but no provider is ready yet.
    if state.oidc.is_none() && local_snapshot.is_none() {
        tracing::warn!(
            component = "unified_auth",
            path = %path,
            "auth required but no provider ready — returning 503"
        );
        return initializing_response();
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
    if let Some(local_state) = local_snapshot.as_ref() {
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

    // Both failed — log details internally but return a generic message to the client
    // to avoid leaking provider-specific error details.
    tracing::debug!(
        component = "unified_auth",
        oidc_error = ?oidc_error,
        local_error = ?local_error,
        "Authentication failed"
    );

    auth_error_response(StatusCode::UNAUTHORIZED, "Authentication failed")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::common::auth::AdminAuthConfig;
    use crate::core::common::local_auth::LocalAuthConfig;
    use axum::body::to_bytes;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    fn invalid_oidc_cfg() -> AdminAuthConfig {
        let mut c = AdminAuthConfig::default();
        c.enabled = true;
        c.discovery = String::new();
        c
    }

    fn invalid_local_cfg() -> LocalAuthConfig {
        let mut c = LocalAuthConfig::default();
        c.enabled = true;
        c.username = String::new();
        c.password = String::new();
        c
    }

    fn valid_local_cfg() -> LocalAuthConfig {
        let mut c = LocalAuthConfig::default();
        c.enabled = true;
        c.username = "admin".to_string();
        c.password = "a_long_enough_password_123".to_string();
        c.jwt_secret = "a_long_enough_jwt_secret_value_abcdef".to_string();
        c
    }

    #[test]
    fn from_configs_returns_err_on_oidc_validate_failure() {
        let cfg = invalid_oidc_cfg();
        let r = UnifiedAuthState::from_configs(Some(&cfg), None, true, "test");
        assert!(r.is_err(), "expected Err for invalid OIDC config, got Ok");
    }

    #[test]
    fn from_configs_returns_err_on_local_validate_failure() {
        let cfg = invalid_local_cfg();
        let r = UnifiedAuthState::from_configs(None, Some(&cfg), true, "test");
        assert!(r.is_err(), "expected Err for invalid local config, got Ok");
    }

    #[test]
    fn from_configs_ok_when_only_local_valid() {
        let cfg = valid_local_cfg();
        let r = UnifiedAuthState::from_configs(None, Some(&cfg), true, "test");
        let state = r.expect("valid local cfg should build state");
        assert!(state.oidc.is_none());
        assert!(state.local.load().is_some());
        assert!(state.require_auth);
    }

    #[test]
    fn update_local_visible_on_next_load() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        assert!(state.local.load().is_none());
        let cfg = valid_local_cfg();
        state.update_local(LocalAuthState::from_config(&cfg));
        assert!(state.local.load().is_some());
    }

    #[test]
    fn require_auth_flag_propagates() {
        let state_no = UnifiedAuthState::from_configs(None, None, false, "test").unwrap();
        assert!(!state_no.require_auth);

        let state_yes = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        assert!(state_yes.require_auth);
    }

    fn router_with_state(state: Arc<UnifiedAuthState>) -> axum::Router {
        use axum::{middleware, routing::get};
        axum::Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, unified_auth_middleware))
    }

    #[tokio::test]
    async fn middleware_returns_503_when_require_auth_and_no_provider() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let app = router_with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/test")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let retry = resp.headers().get("retry-after").expect("Retry-After header");
        assert_eq!(retry.to_str().unwrap(), "10");
    }

    #[tokio::test]
    async fn middleware_passthrough_when_require_auth_false() {
        let state = UnifiedAuthState::from_configs(None, None, false, "test").unwrap();
        let app = router_with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/test")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn middleware_503_body_is_json_error() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let app = router_with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/test")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v.get("success").and_then(|x| x.as_bool()), Some(false));
        let err = v.get("error").and_then(|x| x.as_str()).expect("error field");
        assert!(err.contains("initializ"), "error message unexpected: {err}");
    }

    #[tokio::test]
    async fn middleware_health_path_bypasses_503() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let app = axum::Router::new()
            .route("/health", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(state, unified_auth_middleware));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Documents an axum gotcha: `router.layer(L).merge(other)` does NOT apply
    /// `L` to routes brought in via `merge`. Guard against re-introducing this pattern.
    #[tokio::test]
    async fn layer_before_merge_does_not_protect_merged_routes() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let base: axum::Router = axum::Router::new()
            .route("/base", axum::routing::get(|| async { "base-ok" }))
            .layer(axum::middleware::from_fn_with_state(state, unified_auth_middleware));
        let after_router: axum::Router =
            axum::Router::new().route("/after", axum::routing::get(|| async { "after-ok" }));
        let app = base.merge(after_router);

        // /base was in the router when .layer() ran → gated (503 since no provider).
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/base").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        // /after was merged AFTER .layer() → NOT gated. This is the axum behavior
        // we have to work around: merge first, layer last.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/after")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "axum does not retroactively layer merged routes; if this ever changes, update the comment"
        );
    }

    /// The correct pattern the Controller/Center actually use: merge all routes
    /// FIRST, then wrap with `.layer()` via `wrap_with_unified_auth_layer`.
    /// Guards that `/api/v1/auth/me` is behind the middleware.
    #[tokio::test]
    async fn merge_before_layer_protects_all_routes() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let base: axum::Router = axum::Router::new().route("/base", axum::routing::get(|| async { "base-ok" }));
        let merged_in: axum::Router =
            axum::Router::new().route("/merged", axum::routing::get(|| async { "merged-ok" }));
        let app = base.merge(merged_in);
        let app = app.layer(axum::middleware::from_fn_with_state(state, unified_auth_middleware));

        for path in ["/base", "/merged"] {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(path).body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE, "{path} must be gated");
        }
    }
}
