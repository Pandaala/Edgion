use super::*;
use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

/// Build a minimal FileSystem-backed AdminState (non-K8s mode, the mode in
/// which the URL-vs-body name desync is observable). The returned TempDir
/// must be kept alive for the storage path to stay valid.
pub(super) async fn make_fs_state() -> (Arc<AdminState>, tempfile::TempDir) {
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

/// A minimal HTTPRoute body; HTTPRouteSpec fields are all optional so `spec:{}` parses.
fn http_route_body(name: &str, namespace: &str) -> Bytes {
    Bytes::from(format!(
        r#"{{"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{{"name":"{name}","namespace":"{namespace}"}},"spec":{{}}}}"#
    ))
}

fn backend_tls_policy_body(name: &str, api_version: &str) -> Bytes {
    Bytes::from(format!(
        r#"{{
                "apiVersion":"{api_version}",
                "kind":"BackendTLSPolicy",
                "metadata":{{"name":"{name}","namespace":"default"}},
                "spec":{{
                    "targetRefs":[{{"group":"","kind":"Service","name":"backend"}}],
                    "validation":{{"hostname":"backend.default.svc","wellKnownCACertificates":"System"}}
                }}
            }}"#
    ))
}

#[tokio::test]
async fn update_rejects_mismatched_name() {
    let (state, _tmp) = make_fs_state().await;
    let result = update_namespaced(
        State(state),
        Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
        HeaderMap::new(),
        http_route_body("route-b", "default"),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
}

#[tokio::test]
async fn update_rejects_mismatched_namespace() {
    let (state, _tmp) = make_fs_state().await;
    let result = update_namespaced(
        State(state),
        Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
        HeaderMap::new(),
        http_route_body("route-a", "other-ns"),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
}

#[tokio::test]
async fn update_matching_identity_passes_guard() {
    // Identity matches, so the guard must NOT reject; the request reaches the
    // writer, which returns NotFound (404) for the absent resource -- proving
    // the guard does not false-positive on a valid identity.
    let (state, _tmp) = make_fs_state().await;
    let result = update_namespaced(
        State(state),
        Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
        HeaderMap::new(),
        http_route_body("route-a", "default"),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::NOT_FOUND));
}

#[tokio::test]
async fn stale_if_match_returns_conflict_and_keeps_current_resource() {
    let (state, _tmp) = make_fs_state().await;
    let current = r#"{"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"name":"route-a","namespace":"default","resourceVersion":"42"},"spec":{}}"#;
    state
        .conf_center()
        .create_one("HTTPRoute", Some("default"), "route-a", current.to_string())
        .await
        .expect("seed resource must be created");
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::IF_MATCH, r#""41""#.parse().unwrap());

    let result = update_namespaced(
        State(Arc::clone(&state)),
        Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
        headers,
        http_route_body("route-a", "default"),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::CONFLICT));
    let stored = state
        .conf_center()
        .get_one("HTTPRoute", Some("default"), "route-a")
        .await
        .expect("current resource must remain readable");
    let stored: serde_yaml::Value = serde_yaml::from_str(&stored).unwrap();
    assert_eq!(stored["metadata"]["resourceVersion"], "42");
}

#[tokio::test]
async fn stale_delete_if_match_returns_conflict_and_keeps_current_resource() {
    let (state, _tmp) = make_fs_state().await;
    let current = r#"{"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"name":"route-a","namespace":"default","resourceVersion":"42"},"spec":{}}"#;
    state
        .conf_center()
        .create_one("HTTPRoute", Some("default"), "route-a", current.to_string())
        .await
        .expect("seed resource must be created");
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::IF_MATCH, r#""41""#.parse().unwrap());

    let result = delete_namespaced(
        State(Arc::clone(&state)),
        Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
        headers,
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::CONFLICT));
    assert!(state
        .conf_center()
        .get_one("HTTPRoute", Some("default"), "route-a")
        .await
        .is_ok());
}

#[tokio::test]
async fn create_rejects_mismatched_namespace() {
    // Body metadata.namespace ("other-ns") disagrees with the URL namespace
    // ("default"); the create-path guard must reject before persisting,
    // mirroring update_namespaced.
    let (state, _tmp) = make_fs_state().await;
    let result = create_namespaced(
        State(state),
        Path(("HTTPRoute".to_string(), "default".to_string())),
        http_route_body("route-a", "other-ns"),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
}

#[tokio::test]
async fn create_matching_namespace_passes_guard() {
    // Body metadata.namespace matches the URL namespace, so the guard must NOT
    // reject; the request reaches the writer and the resource is created (201),
    // proving the guard does not false-positive on a valid identity.
    let (state, _tmp) = make_fs_state().await;
    let result = create_namespaced(
        State(state),
        Path(("HTTPRoute".to_string(), "default".to_string())),
        http_route_body("route-a", "default"),
    )
    .await;
    let (status, _headers, _body) = result.expect("create should succeed on matching identity");
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn create_rejects_unregistered_backend_tls_policy_version_before_persistence() {
    let (state, _tmp) = make_fs_state().await;
    let result = create_namespaced(
        State(Arc::clone(&state)),
        Path(("BackendTLSPolicy".to_string(), "default".to_string())),
        backend_tls_policy_body("backend-tls", "gateway.networking.k8s.io/v1alpha2"),
    )
    .await;

    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    assert!(state
        .conf_center()
        .get_one("BackendTLSPolicy", Some("default"), "backend-tls")
        .await
        .is_err());
}

#[tokio::test]
async fn tls_route_create_and_update_reject_invalid_versions_before_persistence() {
    let (state, _tmp) = make_fs_state().await;
    let canonical = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "TLSRoute",
        "metadata": {"name": "tls-route", "namespace": "default"},
        "spec": {"hostnames": ["example.com"]}
    });
    for version in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!(42)),
        Some(serde_json::json!("gateway.networking.k8s.io/v1alpha2")),
    ] {
        let mut invalid = canonical.clone();
        match version {
            Some(value) => invalid["apiVersion"] = value,
            None => {
                invalid.as_object_mut().unwrap().remove("apiVersion");
            }
        }
        let result = create_namespaced(
            State(state.clone()),
            Path(("TLSRoute".to_string(), "default".to_string())),
            Bytes::from(invalid.to_string()),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    }
    assert!(state
        .conf_center()
        .get_one("TLSRoute", Some("default"), "tls-route")
        .await
        .is_err());
    let (status, _, _) = create_namespaced(
        State(state.clone()),
        Path(("TLSRoute".to_string(), "default".to_string())),
        Bytes::from(canonical.to_string()),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::CREATED);
    let mut invalid = canonical;
    invalid["apiVersion"] = serde_json::json!("gateway.networking.k8s.io/v1alpha2");
    let result = update_namespaced(
        State(state.clone()),
        Path(("TLSRoute".to_string(), "default".to_string(), "tls-route".to_string())),
        HeaderMap::new(),
        Bytes::from(invalid.to_string()),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    let stored = state
        .conf_center()
        .get_one("TLSRoute", Some("default"), "tls-route")
        .await
        .unwrap();
    let stored: serde_yaml::Value = serde_yaml::from_str(&stored).unwrap();
    assert_eq!(stored["apiVersion"], "gateway.networking.k8s.io/v1");
}

#[tokio::test]
async fn update_rejects_unregistered_backend_tls_policy_version_and_keeps_canonical_object() {
    let (state, _tmp) = make_fs_state().await;
    let (status, _headers, _response) = create_namespaced(
        State(Arc::clone(&state)),
        Path(("BackendTLSPolicy".to_string(), "default".to_string())),
        backend_tls_policy_body("backend-tls", "gateway.networking.k8s.io/v1"),
    )
    .await
    .expect("canonical BackendTLSPolicy create must succeed");
    assert_eq!(status, StatusCode::CREATED);

    let result = update_namespaced(
        State(Arc::clone(&state)),
        Path((
            "BackendTLSPolicy".to_string(),
            "default".to_string(),
            "backend-tls".to_string(),
        )),
        HeaderMap::new(),
        backend_tls_policy_body("backend-tls", "gateway.networking.k8s.io/v1alpha2"),
    )
    .await;

    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    let stored = state
        .conf_center()
        .get_one("BackendTLSPolicy", Some("default"), "backend-tls")
        .await
        .expect("canonical object must remain stored");
    let stored: serde_yaml::Value = serde_yaml::from_str(&stored).unwrap();
    assert_eq!(stored["apiVersion"], "gateway.networking.k8s.io/v1");
}

#[tokio::test]
async fn malformed_tls_update_rejects_before_persistence() {
    let (state, _tmp) = make_fs_state().await;
    let current = serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionTls",
        "metadata": {"name": "tls", "namespace": "default", "resourceVersion": "42"},
        "spec": {"hosts": ["example.com"], "secretRef": {"name": "tls"},
                 "clientAuth": {"mode": "Terminate"}}
    });
    state
        .conf_center()
        .create_one("EdgionTls", Some("default"), "tls", current.to_string())
        .await
        .unwrap();
    let before = state
        .conf_center()
        .get_one("EdgionTls", Some("default"), "tls")
        .await
        .unwrap();
    let mut invalid = current;
    invalid["spec"]["clientAuth"]["mode"] = serde_json::json!("private-marker");
    let result = update_namespaced(
        State(Arc::clone(&state)),
        Path(("EdgionTls".to_string(), "default".to_string(), "tls".to_string())),
        HeaderMap::new(),
        Bytes::from(invalid.to_string()),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    let after = state
        .conf_center()
        .get_one("EdgionTls", Some("default"), "tls")
        .await
        .unwrap();
    assert_eq!(after, before);
}
