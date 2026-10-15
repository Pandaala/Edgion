use super::update_identity_tests::make_fs_state;
use super::*;
use crate::conf_mgr::conf_center::file_system::{
    FileSystemEvent, FileSystemResourceController, PathRegistry, ResourceKey,
};
use crate::conf_mgr::sync_runtime::resource_processor::{ProcessorHandler, ResourceProcessor};
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use crate::conf_mgr::ControllerRuntime;
use serde_json::{json, Value};

fn document(kind: &str) -> Value {
    match kind {
        "HTTPRoute" => json!({"apiVersion":"gateway.networking.k8s.io/v1", "kind":kind,
            "metadata":{"name":"sample", "uid":"original-uid"}, "spec":{"hostnames":["before.example.com"]}}),
        "ConfigMap" => json!({"apiVersion":"v1", "kind":kind,
            "metadata":{"name":"sample", "uid":"original-uid"}, "data":{"value":"before"}}),
        "EdgionPlugins" => json!({"apiVersion":"edgion.io/v1", "kind":kind,
            "metadata":{"name":"sample", "uid":"original-uid"}, "spec":{}}),
        _ => panic!("unsupported test kind"),
    }
}

fn encode(value: &Value, yaml: bool) -> String {
    if yaml {
        serde_yaml::to_string(value).unwrap()
    } else {
        value.to_string()
    }
}

#[test]
fn namespace_admission_changes_only_omitted_or_null_namespace() {
    for yaml in [false, true] {
        for ns in ["default", "tenant-a"] {
            for supplied in [None, Some(Value::Null), Some(json!(ns))] {
                let mut input = document("HTTPRoute");
                input["spec"]["futureField"] = json!({"nested":[1, null, "kept"]});
                input["metadata"]["resourceVersion"] = json!("42");
                if let Some(value) = supplied {
                    input["metadata"]["namespace"] = value;
                }
                let admitted = admit_namespace(encode(&input, yaml), ns).unwrap();
                input["metadata"]["namespace"] = json!(ns);
                let actual: Value = parse_resource(&admitted).unwrap();
                assert_eq!(actual, input);
            }
        }
    }
}

#[test]
fn namespace_admission_rejects_invalid_metadata_and_explicit_namespaces() {
    for yaml in [false, true] {
        for metadata in [
            None,
            Some(Value::Null),
            Some(json!([])),
            Some(json!("invalid")),
            Some(json!({"namespace":""})),
            Some(json!({"namespace":"other"})),
            Some(json!({"namespace":42})),
            Some(json!({"namespace":false})),
        ] {
            let mut input = document("ConfigMap");
            match metadata {
                Some(value) => input["metadata"] = value,
                None => {
                    input.as_object_mut().unwrap().remove("metadata");
                }
            }
            assert_eq!(
                admit_namespace(encode(&input, yaml), "tenant-a"),
                Err(StatusCode::BAD_REQUEST)
            );
        }
    }
}

#[tokio::test]
async fn admitted_create_and_update_persist_url_identity_for_json_and_yaml() {
    for yaml in [false, true] {
        for ns in ["default", "tenant-a"] {
            for supplied in [None, Some(Value::Null), Some(json!(ns))] {
                for kind in ["HTTPRoute", "ConfigMap", "EdgionPlugins"] {
                    let (state, tmp) = make_fs_state().await;
                    let mut input = document(kind);
                    if let Some(value) = supplied.clone() {
                        input["metadata"]["namespace"] = value;
                    }
                    let (status, _, _) = create_namespaced(
                        State(state.clone()),
                        Path((kind.into(), ns.into())),
                        Bytes::from(encode(&input, yaml)),
                    )
                    .await
                    .unwrap();
                    assert_eq!(status, StatusCode::CREATED);
                    let Json(created) =
                        get_namespaced(State(state.clone()), Path((kind.into(), ns.into(), "sample".into())))
                            .await
                            .unwrap();
                    assert_eq!(created["metadata"]["namespace"], ns);
                    assert_eq!(created["metadata"]["uid"], "original-uid");
                    let path = tmp.path().join(ns).join(format!("{kind}_{ns}_sample.yaml"));
                    let stored: Value = serde_yaml::from_str(&tokio::fs::read_to_string(&path).await.unwrap()).unwrap();
                    assert_eq!(stored["metadata"], created["metadata"]);
                    assert!(!tmp.path().join("cluster").join(format!("{kind}__sample.yaml")).exists());

                    input["metadata"]["labels"] = json!({"revision":"updated"});
                    input["metadata"]["uid"] = json!("ignored-update-uid");
                    // Omitted PUT name retains the current server-owned identity.
                    input["metadata"].as_object_mut().unwrap().remove("name");
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        axum::http::header::IF_MATCH,
                        created["metadata"]["resourceVersion"]
                            .as_str()
                            .unwrap()
                            .parse()
                            .unwrap(),
                    );
                    let _ = update_namespaced(
                        State(state.clone()),
                        Path((kind.into(), ns.into(), "sample".into())),
                        headers,
                        Bytes::from(encode(&input, yaml)),
                    )
                    .await
                    .unwrap();
                    let Json(updated) =
                        get_namespaced(State(state.clone()), Path((kind.into(), ns.into(), "sample".into())))
                            .await
                            .unwrap();
                    assert_eq!(updated["metadata"]["namespace"], ns);
                    assert_eq!(updated["metadata"]["name"], "sample");
                    assert_eq!(updated["metadata"]["uid"], "original-uid");
                    assert_eq!(updated["metadata"]["labels"]["revision"], "updated");
                    assert_ne!(
                        updated["metadata"]["resourceVersion"],
                        created["metadata"]["resourceVersion"]
                    );
                    let stored: Value = serde_yaml::from_str(&tokio::fs::read_to_string(&path).await.unwrap()).unwrap();
                    assert_eq!(stored["metadata"], updated["metadata"]);
                }
            }
        }
    }
}

#[tokio::test]
async fn explicit_namespace_rejection_preserves_storage_and_update_preconditions() {
    for yaml in [false, true] {
        let (state, _tmp) = make_fs_state().await;
        let mut input = document("ConfigMap");
        for invalid in [json!("other"), json!(""), json!(42), json!(false)] {
            input["metadata"]["namespace"] = invalid;
            assert_eq!(
                create_namespaced(
                    State(state.clone()),
                    Path(("ConfigMap".into(), "tenant-a".into())),
                    Bytes::from(encode(&input, yaml))
                )
                .await
                .err(),
                Some(StatusCode::BAD_REQUEST)
            );
        }
        assert!(state
            .conf_center()
            .get_one("ConfigMap", Some("tenant-a"), "sample")
            .await
            .is_err());
        input["metadata"].as_object_mut().unwrap().remove("namespace");
        let _ = create_namespaced(
            State(state.clone()),
            Path(("ConfigMap".into(), "tenant-a".into())),
            Bytes::from(encode(&input, yaml)),
        )
        .await
        .unwrap();
        let before = state
            .conf_center()
            .get_one("ConfigMap", Some("tenant-a"), "sample")
            .await
            .unwrap();
        for invalid in [json!("other"), json!(""), json!(42), json!(false)] {
            input["metadata"]["namespace"] = invalid;
            assert_eq!(
                update_namespaced(
                    State(state.clone()),
                    Path(("ConfigMap".into(), "tenant-a".into(), "sample".into())),
                    HeaderMap::new(),
                    Bytes::from(encode(&input, yaml))
                )
                .await
                .err(),
                Some(StatusCode::BAD_REQUEST)
            );
            assert_eq!(
                state
                    .conf_center()
                    .get_one("ConfigMap", Some("tenant-a"), "sample")
                    .await
                    .unwrap(),
                before
            );
        }
        input["metadata"].as_object_mut().unwrap().remove("namespace");
        input["metadata"]["resourceVersion"] = json!("stale-version");
        assert_eq!(
            update_namespaced(
                State(state.clone()),
                Path(("ConfigMap".into(), "tenant-a".into(), "sample".into())),
                HeaderMap::new(),
                Bytes::from(encode(&input, yaml))
            )
            .await
            .err(),
            Some(StatusCode::CONFLICT)
        );
        assert_eq!(
            state
                .conf_center()
                .get_one("ConfigMap", Some("tenant-a"), "sample")
                .await
                .unwrap(),
            before
        );
    }
}

struct NoopHandler;
#[async_trait::async_trait]
impl ProcessorHandler<HTTPRoute> for NoopHandler {}

#[tokio::test]
async fn admin_persisted_namespace_survives_fs_init_and_queued_update() {
    let (state, tmp) = make_fs_state().await;
    let mut input = document("HTTPRoute");
    let _ = create_namespaced(
        State(state.clone()),
        Path(("HTTPRoute".into(), "tenant-a".into())),
        Bytes::from(encode(&input, true)),
    )
    .await
    .unwrap();
    let path = tmp.path().join("tenant-a/HTTPRoute_tenant-a_sample.yaml");
    let processor = Arc::new(ResourceProcessor::<HTTPRoute>::new(
        "HTTPRoute",
        100,
        Arc::new(NoopHandler),
        Arc::new(ControllerRuntime::default()),
    ));
    let registry = Arc::new(PathRegistry::default());
    registry.record(ResourceKey::new("HTTPRoute", "tenant-a/sample"), path.clone());
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let shutdown = ShutdownHandle::new();
    let controller =
        FileSystemResourceController::new("HTTPRoute", processor.clone(), tmp.path().to_path_buf(), registry, rx)
            .with_shutdown(shutdown.signal());
    let task = tokio::spawn(controller.run());
    tx.send(FileSystemEvent::Init).await.unwrap();
    tx.send(FileSystemEvent::InitApply {
        path: path.clone(),
        content: tokio::fs::read_to_string(&path).await.unwrap(),
    })
    .await
    .unwrap();
    tx.send(FileSystemEvent::InitDone).await.unwrap();
    wait_hostname(&processor, "before.example.com").await;
    assert!(processor.get("sample").is_none());
    assert!(processor.get("default/sample").is_none());

    input["spec"]["hostnames"] = json!(["after.example.com"]);
    let _ = update_namespaced(
        State(state),
        Path(("HTTPRoute".into(), "tenant-a".into(), "sample".into())),
        HeaderMap::new(),
        Bytes::from(input.to_string()),
    )
    .await
    .unwrap();
    tx.send(FileSystemEvent::Apply {
        path: path.clone(),
        content: tokio::fs::read_to_string(&path).await.unwrap(),
    })
    .await
    .unwrap();
    wait_hostname(&processor, "after.example.com").await;
    assert!(processor.get("sample").is_none());
    assert!(processor.get("default/sample").is_none());
    shutdown.shutdown();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

async fn wait_hostname(processor: &ResourceProcessor<HTTPRoute>, expected: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if processor.get("tenant-a/sample").is_some_and(|route| {
                route
                    .spec
                    .hostnames
                    .as_ref()
                    .is_some_and(|hosts| hosts.first().is_some_and(|host| host == expected))
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("FS controller must publish admitted bytes under the URL namespace");
}
