use super::*;

#[tokio::test]
async fn etcd_policy_validation_is_read_only_and_uses_shared_rules() {
    let (state, tmp) = make_state().await;
    for (policy, expected) in [
        (serde_json::json!({"timeout": {"request": 0}}), Some("timeout.request")),
        (serde_json::json!({"keepAlive": {"time": 1}}), Some("keepAlive")),
        (serde_json::json!({"keepAlive": {"time": 1, "timeout": 1}}), None),
    ] {
        let mut config = policy;
        config["endpoints"] = serde_json::json!(["http://etcd.example:2379"]);
        let document = serde_json::json!({
            "apiVersion": "edgion.io/v1", "kind": "LinkSys",
            "metadata": {"name": "etcd-policy", "namespace": "default", "resourceVersion": "17"},
            "spec": {"type": "etcd", "config": config}
        });
        let response = validate_kind(
            State(state.clone()),
            Path("LinkSys".into()),
            Bytes::from(document.to_string()),
        )
        .await
        .unwrap()
        .0
        .data
        .unwrap();
        assert_eq!(response.ok, expected.is_none());
        assert!(response.schema_errors.is_empty());
        assert!(response.unknown_fields.is_empty());
        if let Some(expected) = expected {
            assert!(
                response.validation_errors.iter().any(|error| error.contains(expected)),
                "{:?}",
                response.validation_errors
            );
        }
        let parsed: LinkSys = parse_resource_and_update_version(&document.to_string(), false).unwrap();
        assert_eq!(parsed.metadata.resource_version.as_deref(), Some("17"));
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }
}
use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

async fn make_state() -> (Arc<AdminState>, tempfile::TempDir) {
    let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
    let fs_config =
        FileSystemConfig::new(tmp_data.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
            .await
            .expect("ConfMgr::create"),
    );
    let schema_validator = Arc::new(SchemaValidator::empty());
    let state = Arc::new(AdminState {
        conf_mgr,
        schema_validator,
    });
    (state, tmp_data)
}

fn clean_http_route_body() -> Bytes {
    Bytes::from(
        r#"{
                "apiVersion": "gateway.networking.k8s.io/v1",
                "kind": "HTTPRoute",
                "metadata": {"name": "r1", "namespace": "default"},
                "spec": {}
            }"#,
    )
}

#[tokio::test]
async fn original_gateway_class_field_reports_schema_error() {
    let (mut state, _tmp) = make_state().await;
    let crd_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    Arc::get_mut(&mut state).unwrap().schema_validator = Arc::new(SchemaValidator::from_crd_dir(&crd_dir).unwrap());
    let body = Bytes::from(
        r#"{"apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{"name":"gc-a"},"spec":{"controllerName":"invalid"}}"#,
    );
    let result = validate_kind(State(state), Path("GatewayClass".to_string()), body)
        .await
        .unwrap()
        .0
        .data
        .unwrap();
    assert!(!result.ok);
    assert!(!result.schema_errors.is_empty());
}

#[tokio::test]
async fn gateway_schema_error_precedes_typed_deserialization_error() {
    let (mut state, _tmp) = make_state().await;
    let crd_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    Arc::get_mut(&mut state).unwrap().schema_validator = Arc::new(SchemaValidator::from_crd_dir(&crd_dir).unwrap());
    let body = Bytes::from(
        r#"{"apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{"name":"gc-a"},"spec":{"controllerName":42}}"#,
    );
    let result = validate_kind(State(state), Path("GatewayClass".to_string()), body)
        .await
        .expect("schema verdict must be returned before typed parsing")
        .0
        .data
        .unwrap();
    assert!(!result.ok);
    assert!(result
        .schema_errors
        .iter()
        .any(|error| error.contains("/spec/controllerName")));
}

#[tokio::test]
async fn last_pre_v1_validate_uses_source_schema_and_canonical_rule_limit() {
    let (mut state, _tmp) = make_state().await;
    let crd_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    Arc::get_mut(&mut state).unwrap().schema_validator = Arc::new(SchemaValidator::from_crd_dir(&crd_dir).unwrap());
    let grpc = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1alpha2",
        "kind": "GRPCRoute",
        "metadata": {"name": "grpc", "namespace": "default"},
        "spec": {"rules": [{"matches": vec![serde_json::json!({"method": {"service": "example.Echo"}}); 9]}]}
    });
    let response = validate_kind(
        State(state.clone()),
        Path("GRPCRoute".to_string()),
        Bytes::from(grpc.to_string()),
    )
    .await
    .unwrap()
    .0
    .data
    .unwrap();
    assert!(!response.ok);
    assert!(response.schema_errors.iter().any(|error| error.contains("matches")));

    for kind in ["TCPRoute", "UDPRoute"] {
        let route = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1alpha2",
            "kind": kind,
            "metadata": {"name": "stream", "namespace": "default"},
            "spec": {"rules": [
                {"backendRefs": [{"name": "one", "port": 8080}]},
                {"backendRefs": [{"name": "two", "port": 8080}]}
            ]}
        });
        let response = validate_kind(
            State(state.clone()),
            Path(kind.to_string()),
            Bytes::from(route.to_string()),
        )
        .await
        .unwrap()
        .0
        .data
        .unwrap();
        assert!(!response.ok);
        assert!(response
            .schema_errors
            .iter()
            .any(|error| error.contains("at most one rule")));
    }
}

#[tokio::test]
async fn clean_document_reports_ok_and_unsupported_walk() {
    let (state, _tmp) = make_state().await;
    let resp = validate_kind(State(state), Path("HTTPRoute".to_string()), clean_http_route_body())
        .await
        .expect("validate_kind must succeed");
    let body = resp.0.data.expect("ok_body always carries data");
    assert!(body.ok);
    assert_eq!(body.unknown_field_check, "unsupported");
    assert!(body.unknown_fields.is_empty());
    assert!(body.schema_errors.is_empty());
    assert!(body.validation_errors.is_empty());
}

#[tokio::test]
async fn http_route_transform_contract_is_reported_without_persistence() {
    let (state, tmp) = make_state().await;
    let body = Bytes::from(
        r#"{
                "apiVersion": "gateway.networking.k8s.io/v1",
                "kind": "HTTPRoute",
                "metadata": {"name": "invalid-transform", "namespace": "default"},
                "spec": {"rules": [{"backendRefs": [{
                    "name": "backend",
                    "filters": [{"type": "URLRewrite", "urlRewrite": {
                        "path": {"type": "ReplaceFullPath", "replacePrefixMatch": "/v2"}
                    }}]
                }]}]}
            }"#,
    );

    let payload = validate_kind(State(state), Path("HTTPRoute".to_string()), body)
        .await
        .expect("HTTPRoute validation succeeds")
        .0
        .data
        .expect("response data");

    assert!(!payload.ok);
    assert!(payload
        .validation_errors
        .iter()
        .any(|error| error.contains("rules[0]") && error.contains("backendRefs/0/filters/0")));
    assert!(std::fs::read_dir(tmp.path()).expect("read conf dir").next().is_none());
}

#[tokio::test]
async fn typo_in_edgion_plugins_config_is_reported_and_not_persisted() {
    let (state, tmp) = make_state().await;
    let body = Bytes::from(
        r#"{
                "apiVersion": "edgion.io/v1",
                "kind": "EdgionPlugins",
                "metadata": {"name": "auth-chain", "namespace": "default"},
                "spec": {
                    "requestPlugins": [
                        {"type": "BasicAuth", "config": {"denys": ["x"]}}
                    ]
                }
            }"#,
    );
    let resp = validate_kind(State(state.clone()), Path("EdgionPlugins".to_string()), body)
        .await
        .expect("validate_kind must succeed");
    let payload = resp.0.data.expect("ok_body always carries data");
    assert!(!payload.ok);
    assert_eq!(payload.unknown_field_check, "performed");
    assert_eq!(
        payload.unknown_fields,
        vec!["requestPlugins[0].config.denys".to_string()]
    );

    // Nothing persisted: the FileSystem conf dir must remain empty.
    let mut entries = std::fs::read_dir(tmp.path()).expect("read conf dir");
    assert!(
        entries.next().is_none(),
        "validate must not write anything to the backing store"
    );
}

#[tokio::test]
async fn typed_config_data_typo_is_reported_while_misc_remains_open() {
    let (state, _tmp) = make_state().await;
    let typed = Bytes::from(
        r#"{
                "apiVersion":"edgion.io/v1","kind":"EdgionConfigData",
                "metadata":{"name":"cidrs","namespace":"default"},
                "spec":{"data":{"type":"IpList","config":{"items":[{"name":"office","cidrs":[],"cidrz":[]}]}}}
            }"#,
    );
    let typed = validate_kind(State(state.clone()), Path("EdgionConfigData".to_string()), typed)
        .await
        .expect("typed ConfigData validation succeeds")
        .0
        .data
        .expect("response data");
    assert!(!typed.ok);
    assert_eq!(typed.unknown_field_check, "performed");
    assert_eq!(typed.unknown_fields, ["data.config.items[0].cidrz"]);

    let misc = Bytes::from(
        r#"{
                "apiVersion":"edgion.io/v1","kind":"EdgionConfigData",
                "metadata":{"name":"vendor","namespace":"default"},
                "spec":{"data":{"type":"Misc","config":{"vendor":{"arbitrary":[1,2,3]}}}}
            }"#,
    );
    let misc = validate_kind(State(state), Path("EdgionConfigData".to_string()), misc)
        .await
        .expect("Misc ConfigData validation succeeds")
        .0
        .data
        .expect("response data");
    assert!(misc.ok);
    assert_eq!(misc.unknown_field_check, "performed");
    assert!(misc.unknown_fields.is_empty());
}

#[tokio::test]
async fn url_allow_list_nested_typo_reaches_advisory_validation_result() {
    let (state, _tmp) = make_state().await;
    let body = Bytes::from(
        r#"{
                "apiVersion":"edgion.io/v1","kind":"EdgionConfigData",
                "metadata":{"name":"urls","namespace":"default"},
                "spec":{"data":{"type":"RequestAccessUrlAllowList","config":{"items":[{
                    "name":"health","hosts":["api.example.com"],
                    "paths":[{"type":"Exact","value":"/health","valuez":"/ignored"}]
                }]}}}
            }"#,
    );

    let payload = validate_kind(State(state.clone()), Path("EdgionConfigData".to_string()), body)
        .await
        .expect("typed ConfigData typo must reach the advisory validator")
        .0
        .data
        .expect("response data");

    assert!(!payload.ok);
    assert_eq!(payload.unknown_field_check, "performed");
    assert_eq!(payload.unknown_fields, ["data.config.items[0].paths[0].valuez"]);

    let clean = Bytes::from(
        r#"{
                "apiVersion":"edgion.io/v1","kind":"EdgionConfigData",
                "metadata":{"name":"urls","namespace":"default"},
                "spec":{"data":{"type":"RequestAccessUrlAllowList","config":{"items":[{
                    "name":"health","hosts":["api.example.com"],
                    "paths":[{"type":"Exact","value":"/health"}]
                }]}}}
            }"#,
    );
    let clean = validate_kind(State(state), Path("EdgionConfigData".to_string()), clean)
        .await
        .expect("clean typed ConfigData must validate")
        .0
        .data
        .expect("response data");
    assert!(clean.ok);
    assert_eq!(clean.unknown_field_check, "performed");
    assert!(clean.unknown_fields.is_empty());
}

#[tokio::test]
async fn link_sys_typo_is_reported_without_persistence() {
    let (state, tmp) = make_state().await;
    let body = Bytes::from(
        r#"{
                "apiVersion":"edgion.io/v1","kind":"LinkSys",
                "metadata":{"name":"redis","namespace":"default"},
                "spec":{"type":"redis","config":{"endpoints":[],"timeout":{"conect":10}}}
            }"#,
    );
    let payload = validate_kind(State(state), Path("LinkSys".to_string()), body)
        .await
        .expect("LinkSys validation succeeds")
        .0
        .data
        .expect("response data");
    assert!(!payload.ok);
    assert_eq!(payload.unknown_field_check, "performed");
    assert_eq!(payload.unknown_fields, ["config.timeout.conect"]);
    assert!(std::fs::read_dir(tmp.path()).expect("read conf dir").next().is_none());
}

#[tokio::test]
async fn unparseable_body_is_bad_request() {
    let (state, _tmp) = make_state().await;
    let result = validate_kind(
        State(state),
        Path("HTTPRoute".to_string()),
        Bytes::from("not yaml or json content"),
    )
    .await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
}

#[tokio::test]
async fn backend_tls_policy_validation_rejects_unregistered_version_before_typed_parse() {
    let (state, tmp) = make_state().await;
    let body = Bytes::from(
        r#"{
                "apiVersion":"gateway.networking.k8s.io/v1alpha2",
                "kind":"BackendTLSPolicy",
                "metadata":{"name":"backend-tls","namespace":"default"},
                "spec":{
                    "targetRefs":[{"group":"","kind":"Service","name":"backend"}],
                    "validation":{"hostname":"backend.default.svc","wellKnownCACertificates":"System"}
                }
            }"#,
    );

    let result = validate_kind(State(state), Path("BackendTLSPolicy".to_string()), body).await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    assert!(std::fs::read_dir(tmp.path()).expect("read conf dir").next().is_none());
}

#[tokio::test]
async fn unknown_kind_is_bad_request() {
    let (state, _tmp) = make_state().await;
    let result = validate_kind(State(state), Path("NoSuchKind".to_string()), clean_http_route_body()).await;
    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
}

#[tokio::test]
async fn tls_route_validation_rejects_invalid_source_versions() {
    let (state, tmp) = make_state().await;
    for version in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!(7)),
        Some(serde_json::json!("gateway.networking.k8s.io/v1alpha2")),
    ] {
        let mut document = serde_json::json!({
            "kind": "TLSRoute", "metadata": {"name": "tls-route", "namespace": "default"},
            "spec": {"hostnames": ["example.com"]}
        });
        if let Some(value) = version {
            document["apiVersion"] = value;
        }
        let result = validate_kind(
            State(state.clone()),
            Path("TLSRoute".to_string()),
            Bytes::from(document.to_string()),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    }
    assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn yaml_and_json_bodies_behave_identically() {
    let (state, _tmp) = make_state().await;
    let yaml_body = Bytes::from(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: r1\n  namespace: default\nspec: {}\n",
        );
    let resp = validate_kind(State(state), Path("HTTPRoute".to_string()), yaml_body)
        .await
        .expect("validate_kind must succeed on YAML");
    let body = resp.0.data.expect("ok_body always carries data");
    assert!(body.ok);
}
