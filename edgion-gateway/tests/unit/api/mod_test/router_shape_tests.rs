use super::*;
use crate::conf_sync::ConfigClient;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for `oneshot`

/// Token used by the admin-router shape tests. The admin router is now
/// auth-guarded, so these routing-presence tests configure a token and send
/// it on every request — letting the request reach (or miss) the route so we
/// assert routing, not auth. The probe router ignores the header.
const TEST_ADMIN_TOKEN: &str = "router-shape-test-admin-token-0123456789";

fn test_client() -> Arc<ConfigClient> {
    Arc::new(ConfigClient::new("test".to_string(), "test".to_string()))
}

async fn status_of(router: Router, path: &str) -> StatusCode {
    router
        .oneshot(
            Request::builder()
                .uri(path)
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn probe_router_serves_health_and_ready() {
    let r = create_probe_router(test_client());
    assert_eq!(status_of(r.clone(), "/health").await, StatusCode::OK);
    let ready = status_of(r, "/ready").await;
    assert!(ready == StatusCode::OK || ready == StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn policy_fatal_overrides_ready_config_cache() {
    assert_eq!(readiness_status(true, false, false), StatusCode::OK);
    assert_eq!(readiness_status(true, true, false), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(readiness_status(false, false, false), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(readiness_status(true, false, true), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn admin_router_drops_health_and_ready_but_keeps_configclient() {
    let r = create_admin_router(test_client(), Some(TEST_ADMIN_TOKEN.to_string()));
    assert_eq!(status_of(r.clone(), "/health").await, StatusCode::NOT_FOUND);
    assert_eq!(status_of(r.clone(), "/ready").await, StatusCode::NOT_FOUND);
    assert_eq!(status_of(r, "/configclient/httproute/list").await, StatusCode::OK);
}

#[tokio::test]
async fn admin_router_drops_server_info_keeps_configclient() {
    // Production admin router (integration testing mode off in unit tests):
    // server-info is testing-only and must be absent; /configclient stays mounted.
    let r = create_admin_router(test_client(), Some(TEST_ADMIN_TOKEN.to_string()));
    assert_eq!(status_of(r.clone(), "/api/v1/server-info").await, StatusCode::NOT_FOUND);
    assert_eq!(status_of(r, "/configclient/httproute/list").await, StatusCode::OK);
}
