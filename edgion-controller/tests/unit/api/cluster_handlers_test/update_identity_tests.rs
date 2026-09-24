use super::*;
use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

/// Build a minimal FileSystem-backed AdminState (non-K8s mode, the mode in
/// which the URL-vs-body name desync is observable). The returned TempDir
/// must be kept alive for the storage path to stay valid.
async fn make_fs_state() -> (Arc<AdminState>, tempfile::TempDir) {
    let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
    let fs_config =
        FileSystemConfig::new(tmp_data.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
            .await
            .expect("ConfMgr::create"),
    );
    let schema_validator = Arc::new(SchemaValidator::empty());
    let (tx, _rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
    let authorizer = Arc::new(crate::authz::Authorizer::new(crate::authz::CenterPolicy::deny_all()));
    let state = Arc::new(AdminState {
        conf_mgr,
        schema_validator,
        config_file: None,
        authorizer,
        center_reload_tx: tx,
    });
    (state, tmp_data)
}

fn gateway_class_body(name: &str) -> Bytes {
    Bytes::from(format!(
        r#"{{"apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{{"name":"{name}"}},"spec":{{"controllerName":"edgion.io/gateway"}}}}"#
    ))
}

#[tokio::test]
async fn update_cluster_rejects_mismatched_name() {
    let (state, _tmp) = make_fs_state().await;
    let result = update_cluster(
        State(state),
        Path(("GatewayClass".to_string(), "gc-a".to_string())),
        HeaderMap::new(),
        gateway_class_body("gc-b"),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
}

#[tokio::test]
async fn update_cluster_matching_name_passes_guard() {
    // Identity matches, so the guard must NOT reject; the request reaches the
    // writer, which returns NotFound (404) for the absent resource -- proving
    // the guard does not false-positive on a valid identity.
    let (state, _tmp) = make_fs_state().await;
    let result = update_cluster(
        State(state),
        Path(("GatewayClass".to_string(), "gc-a".to_string())),
        HeaderMap::new(),
        gateway_class_body("gc-a"),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::NOT_FOUND));
}

#[tokio::test]
async fn create_rejects_original_schema_invalid_gateway_class() {
    let (mut state, _tmp) = make_fs_state().await;
    let crd_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    Arc::get_mut(&mut state).unwrap().schema_validator = Arc::new(SchemaValidator::from_crd_dir(&crd_dir).unwrap());
    let content = Bytes::from(
        r#"{"apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{"name":"gc-a"},"spec":{"controllerName":"invalid"}}"#,
    );
    assert!(parse_resource_and_update_version::<GatewayClass>(std::str::from_utf8(&content).unwrap(), false).is_ok());
    let result = create_cluster(State(Arc::clone(&state)), Path("GatewayClass".to_string()), content).await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    assert!(state.conf_center().get_one("GatewayClass", None, "gc-a").await.is_err());
}
