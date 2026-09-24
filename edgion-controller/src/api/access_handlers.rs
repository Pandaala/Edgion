use axum::extract::Extension;
use axum::response::{IntoResponse, Json, Response};
use http::StatusCode;
use serde::Serialize;

use super::ApiResponse;
use crate::authz::Role;

pub async fn get_access(Extension(role): Extension<Role>) -> Response {
    match role.access_snapshot() {
        Ok(snapshot) => Json(ApiResponse::ok_body(snapshot)).into_response(),
        Err(error) => {
            tracing::error!(component = "admin_api", error = %error, "Failed to build effective access snapshot");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(AccessErrorResponse {
                    success: false,
                    error: AccessError {
                        code: "ACCESS_SNAPSHOT_FAILED",
                        message: error.to_string(),
                    },
                }),
            )
                .into_response()
        }
    }
}

#[derive(Serialize)]
struct AccessErrorResponse {
    success: bool,
    error: AccessError,
}

#[derive(Serialize)]
struct AccessError {
    code: &'static str,
    message: String,
}

#[cfg(test)]
#[path = "../../tests/unit/api/access_handlers_test.rs"]
mod tests;
