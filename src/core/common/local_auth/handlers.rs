//! HTTP handlers for local authentication endpoints.
//!
//! Provides:
//! - `POST /api/v1/auth/login` — validate credentials, return JWT
//! - `GET /api/v1/auth/me`    — return authenticated user info from JWT claims

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::common::api::ApiResponse;

use super::middleware::LocalAuthState;

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

/// Auth status response — tells clients what authentication is required.
#[derive(Debug, Clone, Serialize)]
pub struct AuthStatusResponse {
    /// Whether authentication is required for API access.
    pub auth_required: bool,
    /// Authentication method: "local", "oidc", or "none".
    pub auth_method: String,
}

/// POST /api/v1/auth/login
///
/// Validates username + password against the stored config and returns a signed JWT.
pub async fn login_handler(
    State(state): State<Arc<LocalAuthState>>,
    Json(req): Json<LoginRequest>,
) -> Response {
    // Validate username
    if req.username != state.username {
        tracing::debug!(
            component = "local_auth",
            "Login failed: unknown username"
        );
        let body: Json<ApiResponse<LoginResponse>> =
            Json(ApiResponse::error("Invalid username or password".to_string()));
        return (StatusCode::UNAUTHORIZED, body).into_response();
    }

    // Validate password using bcrypt
    match bcrypt::verify(&req.password, &state.password_hash) {
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
    let expiry_secs = state.jwt_expiry_hours * 3600;
    let claims = LocalAuthClaims {
        sub: state.username.clone(),
        iat: now,
        exp: now + expiry_secs as usize,
    };

    // Encode JWT
    let encoding_key = EncodingKey::from_secret(state.jwt_secret.as_bytes());
    match encode(&Header::new(jsonwebtoken::Algorithm::HS256), &claims, &encoding_key) {
        Ok(token) => {
            tracing::info!(
                component = "local_auth",
                username = %state.username,
                "Login successful"
            );
            let response = LoginResponse {
                token,
                expires_in: expiry_secs,
            };
            Json(ApiResponse::success(response)).into_response()
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
/// by [`crate::core::common::local_auth::middleware::local_auth_middleware`].
pub async fn me_handler(
    axum::Extension(claims): axum::Extension<LocalAuthClaims>,
) -> Json<ApiResponse<MeResponse>> {
    Json(ApiResponse::success(MeResponse {
        username: claims.sub,
    }))
}
