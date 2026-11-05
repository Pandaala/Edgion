use super::*;
use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

async fn make_fs_state() -> (Arc<AdminState>, tempfile::TempDir) {
    let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
    let fs_config =
        FileSystemConfig::new(tmp_data.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
            .await
            .expect("ConfMgr::create"),
    );
    let state = Arc::new(AdminState {
        conf_mgr,
        schema_validator: Arc::new(SchemaValidator::empty()),
    });
    use crate::conf_mgr::sync_runtime::resource_processor::{EdgionPluginsHandler, ProcessorObj, ResourceProcessor};
    let runtime = state.runtime();
    let processor = Arc::new(ResourceProcessor::<edgion_resources::EdgionPlugins>::new(
        "EdgionPlugins",
        10,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    processor.set_ready();
    // Handler tests bypass startup; emulate the persisted-version floor that
    // the production write-readiness gate requires before allocating versions.
    crate::conf_mgr::sync_runtime::resource_version::bump_resource_version_floor(42);
    (state, tmp_data)
}

fn override_body(resource_version: &str) -> String {
    serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionConfigData",
        "metadata": {
            "name": "override",
            "namespace": "default",
            "resourceVersion": resource_version
        },
        "spec": {
            "enable": true,
            "visibility": "Namespace",
            "data": {
                "type": "ClusterRegionSetting",
                "config": { "clusterName":"prod", "regionName":"east", "regions": [
                    { "name": "east", "hashRange": [0, 499], "backendEndpoint": "east.example.test" },
                    { "name": "west", "hashRange": [500, 999], "backendEndpoint": "west.example.test" }
                ] }
            }
        }
    })
    .to_string()
}

async fn preview(state: Arc<AdminState>, target: &str) -> Result<Value, StatusCode> {
    preview_setting_failover(
        State(state),
        axum::extract::Path(("default".into(), "override".into())),
        Json(SettingFailoverRequest {
            region_name: "east".into(),
            failover_to: target.into(),
        }),
    )
    .await
    .map(|v| v.0)
}
async fn apply(state: Arc<AdminState>, target: &str, receipt: Value) -> Result<Value, StatusCode> {
    apply_setting_failover(
        State(state),
        axum::extract::Path(("default".into(), "override".into())),
        Json(SettingFailoverApplyRequest {
            region_name: "east".into(),
            failover_to: target.into(),
            precondition: serde_json::from_value(receipt["data"]["precondition"].clone()).unwrap(),
        }),
    )
    .await
    .map(|v| v.0)
}
async fn seed(state: &AdminState, body: Value) {
    state
        .conf_center()
        .set_one("EdgionConfigData", Some("default"), "override", body.to_string())
        .await
        .unwrap();
    // Direct storage fixtures bypass processor initialization, which restores
    // the persisted version floor before the real admin API accepts writes.
    if let Some(version) = body["metadata"]["resourceVersion"]
        .as_str()
        .and_then(|version| version.parse().ok())
    {
        crate::conf_mgr::sync_runtime::resource_version::bump_resource_version_floor(version);
    }
}
async fn stored(state: &AdminState) -> Value {
    yaml_serde::from_str(
        &state
            .conf_center()
            .get_source_one("EdgionConfigData", Some("default"), "override")
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn failover_advances_stored_resource_version_and_rejects_replayed_preview() {
    let (state, _dir) = make_fs_state().await;
    seed(&state, serde_json::from_str(&override_body("42")).unwrap()).await;
    let receipt = preview(state.clone(), "west").await.unwrap();
    assert!(receipt["data"]["modified"].as_bool().unwrap());
    let response = apply(state.clone(), "west", receipt.clone()).await.unwrap();
    assert_eq!(response["persistence"], "persisted");
    assert_eq!(response["controllerObservation"], "not-confirmed");
    assert_eq!(response["gatewayApplication"], "not-confirmed");
    let current = stored(&state).await;
    assert_ne!(current["metadata"]["resourceVersion"], "42");
    assert_eq!(current["spec"]["data"]["config"]["regions"][0]["failoverTo"], "west");
    assert_eq!(
        apply(state.clone(), "west", receipt).await.unwrap_err(),
        StatusCode::CONFLICT
    );
    let receipt = preview(state.clone(), "west").await.unwrap();
    let response = apply(state.clone(), "west", receipt).await.unwrap();
    assert_eq!(response["modified"], false);
    assert_eq!(response["persistence"], "unchanged");
    assert_eq!(stored(&state).await, current);
}
#[tokio::test]
async fn failover_precondition_covers_unversioned_unknown_fields_and_recreation() {
    for change in ["unknown", "uid", "identity"] {
        let (state, _dir) = make_fs_state().await;
        let mut original: Value = serde_json::from_str(&override_body("42")).unwrap();
        original["metadata"].as_object_mut().unwrap().remove("resourceVersion");
        original["metadata"]["uid"] = serde_json::json!("old-uid");
        original["spec"]["unknown"] = serde_json::json!("original");
        seed(&state, original.clone()).await;
        let receipt = preview(state.clone(), "west").await.unwrap();
        let mut changed = original;
        match change {
            "unknown" => changed["spec"]["unknown"] = serde_json::json!("changed"),
            "uid" => changed["metadata"]["uid"] = serde_json::json!("recreated"),
            _ => changed["spec"]["data"]["config"]["clusterName"] = serde_json::json!("other"),
        }
        seed(&state, changed.clone()).await;
        assert_eq!(
            apply(state.clone(), "west", receipt).await.unwrap_err(),
            StatusCode::CONFLICT
        );
        assert_eq!(stored(&state).await, changed);
        let receipt = preview(state.clone(), "west").await.unwrap();
        apply(state.clone(), "west", receipt).await.unwrap();
        assert_eq!(stored(&state).await["spec"]["unknown"], changed["spec"]["unknown"]);
    }
}
#[tokio::test]
async fn failover_rejects_invalid_disabled_and_unobserved_settings_without_writing() {
    for case in ["disabled", "self", "missing", "cycle", "invalid", "unobserved"] {
        let (state, _dir) = make_fs_state().await;
        let mut source: Value = serde_json::from_str(&override_body("42")).unwrap();
        let target = match case {
            "disabled" => {
                source["spec"]["enable"] = serde_json::json!(false);
                "west"
            }
            "self" => "east",
            "missing" => "unknown",
            "cycle" => {
                source["spec"]["data"]["config"]["regions"][1]["failoverTo"] = serde_json::json!("east");
                "west"
            }
            "invalid" => {
                source["spec"]["data"]["config"]["regions"][0]["backendEndpoint"] = serde_json::json!("bad:port");
                ""
            }
            _ => {
                state
                    .runtime()
                    .processor_registry
                    .get("EdgionPlugins")
                    .unwrap()
                    .set_not_ready();
                "west"
            }
        };
        seed(&state, source.clone()).await;
        assert_eq!(
            preview(state.clone(), target).await.unwrap_err(),
            if case == "unobserved" {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            }
        );
        assert_eq!(stored(&state).await, source);
    }
}
#[tokio::test]
async fn failover_apply_route_requires_controller_readiness() {
    use axum::{body::Body, routing::post, Router};
    use tower::ServiceExt;
    let (state, _dir) = make_fs_state().await;
    let source: Value = serde_json::from_str(&override_body("42")).unwrap();
    seed(&state, source.clone()).await;
    let receipt = preview(state.clone(), "west").await.unwrap();
    let app = Router::new()
        .route(
            "/api/v1/cluster-region-settings/{namespace}/{name}/failover",
            post(apply_setting_failover),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::api::require_ready_middleware,
        ))
        .with_state(state.clone());
    let body =
        serde_json::json!({"regionName":"east","failoverTo":"west","precondition":receipt["data"]["precondition"]});
    let response = app
        .oneshot(
            http::Request::post("/api/v1/cluster-region-settings/default/override/failover")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(stored(&state).await, source);
}

#[tokio::test]
async fn consumer_view_labels_disabled_source_as_source_not_gateway_availability() {
    let (state, _dir) = make_fs_state().await;
    let mut source: Value = serde_json::from_str(&override_body("42")).unwrap();
    source["spec"]["enable"] = serde_json::json!(false);
    seed(&state, source).await;
    let value = list_setting_consumers(State(state), axum::extract::Path(("default".into(), "override".into())))
        .await
        .unwrap()
        .0;
    assert_eq!(value["settingEnabled"], false);
    assert_eq!(value["tableObservation"], "source");
    assert_eq!(value["consumerObservation"], "controller");
    assert_eq!(value["gatewayApplication"], "not-confirmed");
}
