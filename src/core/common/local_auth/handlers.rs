//! HTTP handlers for local authentication endpoints.
//!
//! Provides:
//! - `POST /api/v1/auth/login`  — validate credentials, return JWT
//! - `POST /api/v1/auth/logout` — clear the httpOnly auth cookie
//! - `GET /api/v1/auth/me`      — return authenticated user info from JWT claims

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::common::api::ApiResponse;
use crate::core::common::unified_auth::UnifiedAuthState;

/// JWT claims used by local auth tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAuthClaims {
    /// Subject (username).
    pub sub: String,
    /// Issued-at timestamp (Unix seconds).
    pub iat: usize,
    /// Expiry timestamp (Unix seconds).
    pub exp: usize,
}

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

/// POST /api/v1/auth/login
///
/// Validates username + password against the stored config and returns a signed JWT.
pub async fn login_handler(State(state): State<Arc<UnifiedAuthState>>, Json(req): Json<LoginRequest>) -> Response {
    // Return 503 when the local auth provider is not yet populated.
    let Some(local_state) = state.local.load_full() else {
        use axum::http::HeaderValue;
        let body: Json<ApiResponse<()>> = Json(ApiResponse::err_body(
            "authentication subsystem initializing, retry later".to_string(),
        ));
        let mut resp = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
        resp.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("10"));
        return resp;
    };

    // Validate username. Run dummy bcrypt regardless to equalize timing and
    // prevent side-channel enumeration of whether the username exists.
    if req.username != local_state.username {
        let password = req.password.clone();
        let dummy = local_state.dummy_hash.clone();
        let _ = tokio::task::spawn_blocking(move || bcrypt::verify(&password, &dummy)).await;
        tracing::debug!(component = "local_auth", "Login failed: unknown username");
        let body: Json<ApiResponse<LoginResponse>> =
            Json(ApiResponse::err_body("Invalid username or password".to_string()));
        return (StatusCode::UNAUTHORIZED, body).into_response();
    }

    // Validate password using bcrypt
    let password = req.password.clone();
    let hash = local_state.password_hash.clone();
    let verify_result = match tokio::task::spawn_blocking(move || bcrypt::verify(&password, &hash)).await {
        Ok(r) => r,
        Err(_) => {
            tracing::error!(component = "local_auth", "bcrypt task panicked");
            let body: Json<ApiResponse<LoginResponse>> =
                Json(ApiResponse::err_body("Internal authentication error".to_string()));
            return (StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
        }
    };
    match verify_result {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(component = "local_auth", "Login failed: wrong password");
            let body: Json<ApiResponse<LoginResponse>> =
                Json(ApiResponse::err_body("Invalid username or password".to_string()));
            return (StatusCode::UNAUTHORIZED, body).into_response();
        }
        Err(e) => {
            tracing::error!(component = "local_auth", error = %e, "bcrypt verify error");
            let body: Json<ApiResponse<LoginResponse>> =
                Json(ApiResponse::err_body("Internal authentication error".to_string()));
            return (StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
        }
    }

    // Build JWT claims
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as usize;
    let expiry_secs = local_state.jwt_expiry_hours * 3600;
    let claims = LocalAuthClaims {
        sub: local_state.username.clone(),
        iat: now,
        exp: now + expiry_secs as usize,
    };

    // Encode JWT
    let encoding_key = EncodingKey::from_secret(local_state.jwt_secret.as_bytes());
    match encode(&Header::new(jsonwebtoken::Algorithm::HS256), &claims, &encoding_key) {
        Ok(token) => {
            tracing::info!(
                component = "local_auth",
                username = %local_state.username,
                "Login successful"
            );
            // `Secure` keeps this bearer-equivalent JWT cookie off plaintext http://.
            // Operator-configurable (default true) for non-TLS dev; see LocalAuthConfig.
            let secure_attr = if local_state.cookie_secure { "; Secure" } else { "" };
            let cookie_value = format!(
                "edgion_token={}; HttpOnly; SameSite=Strict; Path=/{}; Max-Age={}",
                token, secure_attr, expiry_secs
            );
            let response = LoginResponse {
                token,
                expires_in: expiry_secs,
            };
            let mut resp = Json(ApiResponse::ok_body(response)).into_response();
            resp.headers_mut()
                .insert(header::SET_COOKIE, cookie_value.parse().unwrap());
            resp
        }
        Err(e) => {
            tracing::error!(component = "local_auth", error = %e, "JWT encode error");
            let body: Json<ApiResponse<LoginResponse>> =
                Json(ApiResponse::err_body("Failed to generate token".to_string()));
            (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
        }
    }
}

/// GET /api/v1/auth/me
///
/// Returns the authenticated user's info, extracted from the JWT claims inserted
/// by the unified auth middleware.
///
/// This handler does NOT guard on `state.local` readiness: readiness is enforced
/// by the middleware (which returns 503 before an unauthenticated request ever
/// reaches here). A valid OIDC token may legitimately reach this handler even
/// when local auth is still `None` — guarding here would spuriously 503 those
/// requests.
pub async fn me_handler(
    axum::Extension(claims): axum::Extension<crate::core::common::unified_auth::UnifiedAuthClaims>,
) -> Response {
    Json(ApiResponse::ok_body(MeResponse {
        username: claims.sub.unwrap_or_default(),
    }))
    .into_response()
}

/// POST /api/v1/auth/logout
///
/// Clears the httpOnly auth cookie by setting Max-Age=0. Reads `cookie_secure`
/// from `LocalAuthState` so the clearing cookie carries the same `Secure`
/// attribute as the login cookie — a `Secure` clearing cookie must not be sent
/// when the login cookie was non-`Secure` (and vice versa), otherwise the
/// browser would refuse the clear over plain HTTP. No readiness guard: logout
/// must work regardless of provider state, so it falls back to secure-by-default
/// when the local provider is not yet populated.
pub async fn logout_handler(State(state): State<Arc<UnifiedAuthState>>) -> Response {
    let cookie_secure = state.local.load_full().map(|s| s.cookie_secure).unwrap_or(true);
    let secure_attr = if cookie_secure { "; Secure" } else { "" };
    let cookie_value = format!(
        "edgion_token=; HttpOnly; SameSite=Strict; Path=/{}; Max-Age=0",
        secure_attr
    );
    let mut resp = Json(ApiResponse::<()>::ok_body(())).into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, cookie_value.parse().unwrap());
    resp
}

#[cfg(test)]
mod handler_tests {
    use super::*;
    use crate::core::common::local_auth::middleware::LocalAuthState;
    use crate::core::common::unified_auth::UnifiedAuthState;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn valid_local() -> crate::core::common::local_auth::LocalAuthConfig {
        let mut c = crate::core::common::local_auth::LocalAuthConfig::default();
        c.enabled = true;
        c.username = "admin".to_string();
        c.password = "a_long_enough_password_123".to_string();
        c.jwt_secret = "a_long_enough_jwt_secret_value_abcdef".to_string();
        c
    }

    fn app_with_unified(state: Arc<UnifiedAuthState>) -> axum::Router {
        use axum::routing::{get, post};
        axum::Router::new()
            .route("/api/v1/auth/login", post(login_handler))
            .route("/api/v1/auth/logout", post(logout_handler))
            .route("/api/v1/auth/me", get(me_handler))
            .with_state(state)
    }

    #[tokio::test]
    async fn login_returns_503_when_local_not_ready() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        let app = app_with_unified(state);
        let body = serde_json::json!({"username":"admin","password":"x"}).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let retry = resp.headers().get("retry-after").expect("Retry-After header");
        assert_eq!(retry.to_str().unwrap(), "10");
    }

    #[tokio::test]
    async fn login_succeeds_after_update_local() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        state.update_local(LocalAuthState::from_config(&valid_local()));
        let app = app_with_unified(state);
        let body = serde_json::json!({
            "username":"admin",
            "password":"a_long_enough_password_123"
        })
        .to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Pin the wire format of `AuthStatus`. CLI bails on serde parse failure,
    /// so silent casing drift (e.g. accidentally dropping `rename_all` or
    /// changing it to `lowercase`) would break every CLI command end-to-end.
    #[test]
    fn auth_status_serde_wire_format_snake_case() {
        assert_eq!(serde_json::to_string(&AuthStatus::Ready).unwrap(), "\"ready\"");
        assert_eq!(
            serde_json::to_string(&AuthStatus::Initializing).unwrap(),
            "\"initializing\""
        );
        assert_eq!(serde_json::to_string(&AuthStatus::Disabled).unwrap(), "\"disabled\"");
        // And round-trip from wire:
        let r: AuthStatus = serde_json::from_str("\"ready\"").unwrap();
        assert_eq!(r, AuthStatus::Ready);
        let i: AuthStatus = serde_json::from_str("\"initializing\"").unwrap();
        assert_eq!(i, AuthStatus::Initializing);
        let d: AuthStatus = serde_json::from_str("\"disabled\"").unwrap();
        assert_eq!(d, AuthStatus::Disabled);
    }

    #[tokio::test]
    async fn login_rejects_wrong_username() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        state.update_local(LocalAuthState::from_config(&valid_local()));
        let app = app_with_unified(state);
        let body = serde_json::json!({"username":"wrong_user","password":"a_long_enough_password_123"}).to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Run a successful login against `state` and return the `Set-Cookie` value.
    async fn login_cookie(state: Arc<UnifiedAuthState>) -> String {
        let app = app_with_unified(state);
        let body = serde_json::json!({
            "username":"admin",
            "password":"a_long_enough_password_123"
        })
        .to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        resp.headers()
            .get(axum::http::header::SET_COOKIE)
            .expect("login must set a cookie")
            .to_str()
            .unwrap()
            .to_string()
    }

    /// Run logout against `state` and return the `Set-Cookie` value.
    async fn logout_cookie(state: Arc<UnifiedAuthState>) -> String {
        let app = app_with_unified(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/logout")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        resp.headers()
            .get(axum::http::header::SET_COOKIE)
            .expect("logout must set a cookie")
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn logout_sets_cookie_max_age_zero() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        state.update_local(LocalAuthState::from_config(&valid_local()));
        let cookie = logout_cookie(state).await;
        assert!(
            cookie.contains("Max-Age=0"),
            "logout cookie should carry Max-Age=0 to clear client state: {cookie}"
        );
    }

    #[tokio::test]
    async fn login_cookie_has_secure_by_default() {
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        state.update_local(LocalAuthState::from_config(&valid_local()));
        let cookie = login_cookie(state).await;
        assert!(
            cookie.contains("; Secure"),
            "login cookie must carry Secure by default: {cookie}"
        );
    }

    #[tokio::test]
    async fn login_cookie_omits_secure_when_disabled() {
        let mut cfg = valid_local();
        cfg.cookie_secure = false;
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        state.update_local(LocalAuthState::from_config(&cfg));
        let cookie = login_cookie(state).await;
        // Match the exact attribute segment: the embedded JWT body could in
        // principle contain the bare substring "Secure".
        assert!(
            !cookie.contains("; Secure"),
            "login cookie must omit Secure when cookie_secure=false: {cookie}"
        );
    }

    #[tokio::test]
    async fn logout_cookie_secure_matches_config() {
        // Secure by default.
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        state.update_local(LocalAuthState::from_config(&valid_local()));
        let cookie = logout_cookie(state).await;
        assert!(
            cookie.contains("; Secure"),
            "logout clearing cookie must carry Secure: {cookie}"
        );

        // Omitted when disabled, so the browser accepts the clear over plain HTTP.
        let mut cfg = valid_local();
        cfg.cookie_secure = false;
        let state = UnifiedAuthState::from_configs(None, None, true, "test").unwrap();
        state.update_local(LocalAuthState::from_config(&cfg));
        let cookie = logout_cookie(state).await;
        assert!(
            !cookie.contains("; Secure"),
            "logout cookie must omit Secure when disabled: {cookie}"
        );
    }
}
