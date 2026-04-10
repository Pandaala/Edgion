mod types;

pub use types::{ApiResponse, ListResponse};

use axum::Router;
use tower_http::cors::{Any, CorsLayer};

/// Apply a permissive CORS layer to an Axum router.
///
/// Allows any origin, method, and header. Suitable for development and
/// scenarios where the Admin API is behind a gateway that enforces stricter policies.
pub fn apply_cors_layer(router: Router) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    router.layer(cors)
}
