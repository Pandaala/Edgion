use super::*;
use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};
use axum::{body::Body, routing::get, Router};
use http::Request;
use serde_json::json;
use tower::ServiceExt;

async fn state() -> (Arc<AdminState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = FileSystemConfig::new(dir.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller");
    let conf_mgr = Arc::new(ConfMgr::create(ConfCenterConfig::FileSystem(config)).await.unwrap());
    (
        Arc::new(AdminState {
            conf_mgr,
            schema_validator: Arc::new(SchemaValidator::empty()),
        }),
        dir,
    )
}
fn setting(name: &str) -> Value {
    json!({"apiVersion":"edgion.io/v1","kind":"EdgionConfigData","metadata":{"namespace":"apps","name":name,"uid":"uid","resourceVersion":"7","annotations":{"credential":"synthetic-secret"}},"spec":{"enable":false,"data":{"type":"ClusterRegionSetting","config":{"clusterName":"cluster","regionName":"local","regions":"invalid-table","unknownField":"preserve"}}}})
}
#[test]
fn key_projection_keeps_readable_identity_without_table_or_credentials() {
    let value = serde_json::to_value(project(setting("setting")).unwrap().unwrap()).unwrap();
    assert_eq!(value["clusterName"], "cluster");
    assert_eq!(value["regionName"], "local");
    let text = value.to_string();
    for forbidden in [
        "synthetic-secret",
        "regions",
        "invalid-table",
        "unknownField",
        "annotations",
        "enable",
    ] {
        assert!(!text.contains(forbidden));
    }
    for identity in [Value::Null, json!(42), json!("x".repeat(64))] {
        let mut source = setting("bad");
        source["spec"]["data"]["config"]["regionName"] = identity;
        let row = serde_json::to_value(project(source).unwrap().unwrap()).unwrap();
        assert_eq!(row["identityDiagnostic"], "invalid-identity");
        assert!(row["regionName"].is_null());
        assert!(row["clusterName"].is_null());
    }
}
#[tokio::test]
async fn source_keys_advance_empty_pages_and_detail_reads_fresh_unknown_fields() {
    let (state, _dir) = state().await;
    let mut other = setting("a-other");
    other["spec"]["data"]["type"] = json!("Misc");
    for (name, body) in [("a-other", other), ("b-setting", setting("b-setting"))] {
        state
            .conf_center()
            .set_one("EdgionConfigData", Some("apps"), name, body.to_string())
            .await
            .unwrap();
    }
    let first = keys(
        State(state.clone()),
        Query(PaginationQuery {
            limit: Some(1),
            continue_token: None,
        }),
    )
    .await
    .unwrap();
    let first = serde_json::to_value(first.0).unwrap();
    assert_eq!(first["count"], 0);
    let token = first["continue_token"].as_str().unwrap().to_owned();
    let second = keys(
        State(state.clone()),
        Query(PaginationQuery {
            limit: Some(1),
            continue_token: Some(token),
        }),
    )
    .await
    .unwrap();
    let second = serde_json::to_value(second.0).unwrap();
    assert_eq!(second["data"][0]["regionName"], "local");
    let detail1 = detail(State(state.clone()), Path(("apps".into(), "b-setting".into())))
        .await
        .unwrap();
    assert_eq!(detail1.0["spec"]["data"]["config"]["unknownField"], "preserve");
    let mut changed = setting("b-setting");
    changed["spec"]["data"]["config"]["regionName"] = json!("new-region");
    state
        .conf_center()
        .set_one("EdgionConfigData", Some("apps"), "b-setting", changed.to_string())
        .await
        .unwrap();
    assert_eq!(
        detail(State(state.clone()), Path(("apps".into(), "b-setting".into())))
            .await
            .unwrap()
            .0["spec"]["data"]["config"]["regionName"],
        "new-region"
    );
    assert_eq!(
        detail(State(state), Path(("apps".into(), "a-other".into())))
            .await
            .unwrap_err(),
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn setting_routes_enforce_resource_list_and_get_before_handlers() {
    use crate::authz::{Authorizer, CenterPolicy};
    use crate::config::{FedAllowRule, FedRbacConfig};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (state, _dir) = state().await;
    state
        .conf_center()
        .set_one(
            "EdgionConfigData",
            Some("apps"),
            "setting",
            setting("setting").to_string(),
        )
        .await
        .unwrap();
    for (grant, path, expected) in [
        (
            "list-keys",
            "/api/v1/cluster-region-settings/keys",
            StatusCode::FORBIDDEN,
        ),
        ("list", "/api/v1/cluster-region-settings/keys", StatusCode::OK),
        (
            "list",
            "/api/v1/cluster-region-settings/apps/setting",
            StatusCode::FORBIDDEN,
        ),
        ("get", "/api/v1/cluster-region-settings/apps/setting", StatusCode::OK),
    ] {
        let az = Arc::new(Authorizer::new(CenterPolicy::from_config(&FedRbacConfig {
            allow: vec![FedAllowRule {
                verbs: vec![grant.into()],
                kinds: vec!["EdgionConfigData".into()],
            }],
        })));
        let role = az.center_role();
        let hits = Arc::new(AtomicUsize::new(0));
        let entered = hits.clone();
        let router = Router::new()
            .route("/api/v1/cluster-region-settings/keys", get(keys))
            .route("/api/v1/cluster-region-settings/{namespace}/{name}", get(detail))
            .route_layer(axum::middleware::from_fn(
                move |request: Request<Body>, next: axum::middleware::Next| {
                    let entered = entered.clone();
                    async move {
                        entered.fetch_add(1, Ordering::Relaxed);
                        next.run(request).await
                    }
                },
            ))
            .route_layer(axum::middleware::from_fn_with_state(
                az,
                crate::api::authz_middleware::authz_layer,
            ))
            .layer(axum::middleware::from_fn(
                move |mut request: Request<Body>, next: axum::middleware::Next| {
                    let role = role.clone();
                    async move {
                        request.extensions_mut().insert(role);
                        next.run(request).await
                    }
                },
            ))
            .with_state(state.clone());
        let response = router
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(hits.load(Ordering::Relaxed), usize::from(expected == StatusCode::OK));
    }
}

#[tokio::test]
async fn source_page_output_overflow_is_a_fixed_non_success() {
    let (state, dir) = state().await;
    state
        .conf_center()
        .set_one("EdgionConfigData", Some("apps"), "large", setting("large").to_string())
        .await
        .unwrap();
    // Literal YAML tabs fit the source/page budget; JSON must escape each tab.
    let source = format!("apiVersion: edgion.io/v1\nkind: EdgionConfigData\nmetadata:\n  namespace: apps\n  name: large\nspec:\n  extension: |-\n    x{}\n", "\t".repeat(6 * 1024 * 1024));
    assert!(source.len() < 8 * 1024 * 1024);
    std::fs::write(dir.path().join("apps/EdgionConfigData_apps_large.yaml"), source).unwrap();
    let router = Router::new()
        .route(
            "/resources/{kind}",
            get(super::super::namespaced_handlers::list_all_namespaces),
        )
        .with_state(state);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/resources/EdgionConfigData")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = axum::body::to_bytes(response.into_body(), 100).await.unwrap();
    assert_eq!(body.as_ref(), b"JSON response exceeds limit");
}
