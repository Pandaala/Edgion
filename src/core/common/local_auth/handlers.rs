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
    /// `"oidc"`, `"local"`. Empty when `status = initializing` or `disabled`.
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
        let body: Json<ApiResponse<()>> = Json(ApiResponse::error(
            "authentication subsystem initializing, retry later".to_string(),
        ));
        let mut resp = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
        resp.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("10"));
        return resp;
    };

    // Validate username
    if req.username != local_state.username {
        tracing::debug!(component = "local_auth", "Login failed: unknown username");
        let body: Json<ApiResponse<LoginResponse>> =
            Json(ApiResponse::error("Invalid username or password".to_string()));
        return (StatusCode::UNAUTHORIZED, body).into_response();
    }

    // Validate password using bcrypt
    match bcrypt::verify(&req.password, &local_state.password_hash) {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(component = "local_auth", "Login failed: wrong password");
            let body: Json<ApiResponse<LoginResponse>> =
                Json(ApiResponse::error("Invalid username or password".to_string()));
            return (StatusCode::UNAUTHORIZED, body).into_response();
        }
        Err(e) => {
            tracing::error!(component = "local_auth", error = %e, "bcrypt verify error");
            let body: Json<ApiResponse<LoginResponse>> =
                Json(ApiResponse::error("Internal authentication error".to_string()));
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
            let cookie_value = format!(
                "edgion_token={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
                token, expiry_secs
            );
            let response = LoginResponse {
                token,
                expires_in: expiry_secs,
            };
            let mut resp = Json(ApiResponse::success(response)).into_response();
            resp.headers_mut()
                .insert(header::SET_COOKIE, cookie_value.parse().unwrap());
            resp
        }
        Err(e) => {
            tracing::error!(component = "local_auth", error = %e, "JWT encode error");
            let body: Json<ApiResponse<LoginResponse>> =
                Json(ApiResponse::error("Failed to generate token".to_string()));
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
    Json(ApiResponse::success(MeResponse {
        username: claims.sub.unwrap_or_default(),
    }))
    .into_response()
}

/// POST /api/v1/auth/logout
///
/// Clears the httpOnly auth cookie by setting Max-Age=0. Stateless — does not
/// consult `LocalAuthState`, so no readiness guard is needed.
pub async fn logout_handler() -> Response {
    let cookie_value = "edgion_token=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0";
    let mut resp = Json(ApiResponse::<()>::success(())).into_response();
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
}
