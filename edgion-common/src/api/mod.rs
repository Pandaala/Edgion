pub mod ip_acceptor;
pub mod ip_allowlist;
mod types;

pub use types::{ApiResponse, ListResponse};

use axum::{
    http::{header, HeaderValue, Response},
    middleware::Next,
};

/// Inject `Cache-Control: no-store` on every admin API response.
///
/// Private caches (browsers, local proxies) are NOT bound by RFC 9111 §3.5's
/// prohibition on storing responses with an `Authorization` header, so the
/// server must make its intent explicit. Applied by the controller as the
/// outermost layer of the Admin API so it fires unconditionally on every
/// response path.
pub async fn cache_control_no_store_middleware(
    request: axum::extract::Request,
    next: Next,
) -> Response<axum::body::Body> {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
