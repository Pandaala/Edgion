use super::*;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "prepared-policy-observation-admin-token";
const PATH: &str = "/api/v1/debug/backend-traffic-policy?group=&kind=Service&namespace=observation-api&name=absent";

fn router(mode: bool, token: Option<&str>) -> Router {
    create_admin_router_with_testing_mode(
        Arc::new(ConfigClient::new("observation-api-test".into(), "test".into())),
        token.map(str::to_string),
        mode,
    )
}

async fn request(router: Router, path: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(path);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = router.oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn prepared_conflict_route_inherits_bearer_auth_and_testing_mount() {
    let (status, body) = request(router(true, Some(TOKEN)), PATH, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["complete"], true);
    assert_eq!(body["data"]["candidates"], json!([]));
    assert!(body["data"]["effective"].is_null());
    assert!(body["data"]["generation"].is_u64());
    for token in [None, Some("wrong-token")] {
        assert_eq!(
            request(router(true, Some(TOKEN)), PATH, token).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        request(router(true, None), PATH, Some(TOKEN)).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        request(router(false, Some(TOKEN)), PATH, Some(TOKEN)).await.0,
        StatusCode::NOT_FOUND
    );
    let probes = create_probe_router(Arc::new(ConfigClient::new(
        "observation-probe-test".into(),
        "test".into(),
    )));
    assert_eq!(request(probes, PATH, Some(TOKEN)).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn prepared_conflict_query_rejects_missing_malformed_and_wrong_typed_targets() {
    let route = router(true, Some(TOKEN));
    for path in [
        "/api/v1/debug/backend-traffic-policy",
        "/api/v1/debug/backend-traffic-policy?group=&kind=Service&namespace=observation-api",
    ] {
        assert_eq!(
            request(route.clone(), path, Some(TOKEN)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    for path in [
        "/api/v1/debug/backend-traffic-policy?group=edgion.io&kind=Service&namespace=ns&name=svc",
        "/api/v1/debug/backend-traffic-policy?group=&kind=EdgionBackend&namespace=ns&name=svc",
        "/api/v1/debug/backend-traffic-policy?group=&kind=Secret&namespace=ns&name=svc",
        "/api/v1/debug/backend-traffic-policy?group=&kind=Service&namespace=&name=svc",
    ] {
        let (status, body) = request(route.clone(), path, Some(TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false);
        assert!(body["data"].is_null());
        assert!(body["error"].is_string());
    }
    let too_long = format!("{PATH}{}", "x".repeat(OBSERVATION_MAX_TARGET_BYTES));
    assert_eq!(request(route.clone(), &too_long, Some(TOKEN)).await.1["success"], false);
    let ai =
        "/api/v1/debug/backend-traffic-policy?group=edgion.io&kind=EdgionBackend&namespace=observation-api&name=absent";
    let (_, body) = request(route, ai, Some(TOKEN)).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["target"]["group"], "edgion.io");
    assert_eq!(body["data"]["target"]["kind"], "EdgionBackend");
}

#[test]
fn prepared_conflict_projection_overflow_is_an_error_body_without_partial_data() {
    use crate::backends::policy::backend_traffic::BackendTrafficPolicyStore;
    // The response helper consumes the same projection result as the live handler.
    let store = BackendTrafficPolicyStore::new();
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: "ns".into(),
        name: "x".repeat(OBSERVATION_MAX_TARGET_BYTES + 1),
    };
    let body = serde_json::to_value(backend_traffic_policy_response(&store.snapshot(), &target)).unwrap();
    assert_eq!(body["success"], false);
    assert!(body.get("data").is_none());
    assert_eq!(body["error"], "Policy observation target exceeds metadata limit");
}
