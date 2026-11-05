use super::*;
use edgion_common::link_sys_core::providers::etcd::EtcdBootstrapConfig;

fn unconnected() -> EtcdStorage {
    let client = EtcdLinkClient::from_bootstrap(
        "source-test",
        EtcdBootstrapConfig {
            endpoints: vec!["http://127.0.0.1:1".into()],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
        },
    )
    .unwrap();
    EtcdStorage::new(Arc::new(client), "/source".into())
}
fn options(limit: u32, continue_token: Option<String>) -> ListOptions {
    ListOptions { limit, continue_token }
}
fn document(ns: &str, name: &str) -> String {
    serde_json::json!({"kind":"ConfigMap","metadata":{"namespace":ns,"name":name},"data":{"unknown":{"keep":true}}})
        .to_string()
}

#[test]
fn source_identity_rejects_malformed_scope_and_key_layout() {
    let storage = unconnected();
    for key in [
        "/source/conf/ConfigMap/_/a",
        "/source/conf/ConfigMap//a",
        "/source/conf/ConfigMap/ns/",
        "/source/conf/ConfigMap/ns/a/b",
        "/source/conf/Secret/ns/a",
    ] {
        assert!(storage.source_identity(key, "ConfigMap", None).is_err(), "{key}");
    }
    assert!(storage
        .source_identity("/source/conf/ConfigMap/ns/a", "ConfigMap", Some("other"))
        .is_err());
    assert!(storage
        .source_identity("/source/conf/GatewayClass/ns/a", "GatewayClass", None)
        .is_err());
    assert!(storage
        .source_identity("/source/conf/GatewayClass/_/a", "GatewayClass", None)
        .is_ok());
    assert!(storage
        .source_identity("/source/conf/ConfigMap/ns/a", "ConfigMap", None)
        .is_ok());
    assert!(storage.source_prefix("ConfigMap", Some("a/b")).is_err());
    assert!(storage.source_prefix("GatewayClass", Some("ns")).is_err());
}

#[tokio::test]
async fn source_cursor_is_bound_and_validated_before_etcd_access() {
    let storage = unconnected();
    let scope = query::SourceCursor {
        version: 1,
        backend: "etcd".into(),
        generation: storage.source_generation.clone(),
        kind: "ConfigMap".into(),
        namespace: None,
        limit: 1,
        position: serde_json::to_string(&Position {
            revision: 7,
            last_key: "/source/conf/ConfigMap/ns/a".into(),
        })
        .unwrap(),
    };
    for (kind, namespace, limit) in [
        ("Secret", None, 1),
        ("ConfigMap", Some("ns"), 1),
        ("ConfigMap", None, 2),
    ] {
        assert!(matches!(
            storage
                .source_page(kind, namespace, options(limit, Some(scope.encode().unwrap())))
                .await,
            Err(ConfWriterError::StalePagination(_))
        ));
    }
    for (revision, key) in [
        (0, "/source/conf/ConfigMap/ns/a"),
        (7, "/source/conf/Secret/ns/a"),
        (7, "/source/conf/ConfigMap/_/a"),
    ] {
        let mut invalid = scope.clone();
        invalid.position = serde_json::to_string(&Position {
            revision,
            last_key: key.into(),
        })
        .unwrap();
        assert!(matches!(
            storage
                .source_page("ConfigMap", None, options(1, Some(invalid.encode().unwrap())))
                .await,
            Err(ConfWriterError::StalePagination(_))
        ));
    }
}

#[tokio::test]
#[ignore = "requires an isolated etcd in EDGION_SOURCE_TEST_ETCD_ENDPOINT"]
async fn source_query_real_pages_counts_keys_and_invalid_documents() {
    let prefix = format!("/source-{}", uuid::Uuid::new_v4());
    let client = Arc::new(
        EtcdLinkClient::from_bootstrap(
            "source-test",
            EtcdBootstrapConfig {
                endpoints: vec![std::env::var("EDGION_SOURCE_TEST_ETCD_ENDPOINT").expect("isolated test endpoint")],
                auth: None,
                tls: None,
                timeout: None,
                keep_alive: None,
                namespace: Some("/tenant/".into()),
            },
        )
        .unwrap(),
    );
    client.init().await.unwrap();
    let storage = EtcdStorage::new(client.clone(), prefix.clone());
    for i in 0..13 {
        storage
            .set_one(
                "ConfigMap",
                Some("ns"),
                &format!("item-{i:02}"),
                document("ns", &format!("item-{i:02}")),
            )
            .await
            .unwrap();
    }
    storage
        .set_one("ConfigMap", Some("other"), "item", document("other", "item"))
        .await
        .unwrap();
    client
        .put_string(&format!("{prefix}/status/ConfigMap/ns/status"), "not a resource")
        .await
        .unwrap();
    assert!(storage
        .get_source_one("ConfigMap", Some("ns"), "item-00")
        .await
        .unwrap()
        .contains("unknown"));
    assert!(matches!(
        storage.get_source_one("ConfigMap", Some("ns"), "item").await,
        Err(ConfWriterError::NotFound(_))
    ));
    let first = storage
        .get_list_by_kind("ConfigMap", Some(options(10, None)))
        .await
        .unwrap();
    assert_eq!(first.items.len(), 10);
    assert!(first.continue_token.is_some());
    storage
        .set_one(
            "ConfigMap",
            Some("ns"),
            "item-12",
            document("ns", "item-12").replace("true", "false"),
        )
        .await
        .unwrap();
    let second = storage
        .get_list_by_kind("ConfigMap", Some(options(10, first.continue_token)))
        .await
        .unwrap();
    assert_eq!(second.items.len(), 4);
    assert!(second.continue_token.is_none());
    assert!(second
        .items
        .iter()
        .find(|e| e.name == "item-12")
        .unwrap()
        .content
        .contains("true"));
    assert_eq!(storage.count_by_kind("ConfigMap", None).await.unwrap().total, 14);
    assert_eq!(storage.count_by_kind("ConfigMap", Some("ns")).await.unwrap().total, 13);
    assert_eq!(
        storage.count_by_kind("ConfigMap", Some("empty")).await.unwrap().total,
        0
    );
    let keys = storage
        .list_keys_by_kind_ns("ConfigMap", "ns", Some(options(10, None)))
        .await
        .unwrap();
    assert_eq!(keys.items.len(), 10);
    assert!(keys.continue_token.is_some());
    let current = storage
        .get_source_one("ConfigMap", Some("ns"), "item-00")
        .await
        .unwrap();
    let expected = query::SourcePrecondition::from_value(&yaml_serde::from_str(&current).unwrap()).unwrap();
    let (first_write, second_write) = tokio::join!(
        storage.update_one_if_source(
            "ConfigMap",
            Some("ns"),
            "item-00",
            current.replace("true", "false"),
            &expected
        ),
        storage.update_one_if_source(
            "ConfigMap",
            Some("ns"),
            "item-00",
            current.replace("true", "42"),
            &expected
        )
    );
    assert_eq!(usize::from(first_write.is_ok()) + usize::from(second_write.is_ok()), 1);
    assert!(
        matches!(first_write, Err(ConfWriterError::Conflict(_)))
            || matches!(second_write, Err(ConfWriterError::Conflict(_)))
    );
    for i in 0..101 {
        let name = format!("item-{i:03}");
        storage
            .set_one("ConfigMap", Some("bulk"), &name, document("bulk", &name))
            .await
            .unwrap();
    }
    assert_eq!(
        storage.count_by_kind("ConfigMap", Some("bulk")).await.unwrap().total,
        101
    );
    storage
        .set_one("ConfigMap", Some("bulk"), "zz-bad", "malformed: [".into())
        .await
        .unwrap();
    assert!(
        storage.count_by_kind("ConfigMap", Some("bulk")).await.is_err(),
        "late parse error must not return a partial total"
    );
    client
        .delete_prefix(&format!("{prefix}/conf/ConfigMap/bulk/"))
        .await
        .unwrap();
    storage
        .set_one("ConfigMap", Some("ns"), "bad", "malformed: [".into())
        .await
        .unwrap();
    assert!(storage.count_by_kind("ConfigMap", None).await.is_err());
    assert!(storage
        .get_list_by_kind("ConfigMap", Some(options(100, None)))
        .await
        .is_err());
    assert!(storage
        .list_keys_by_kind("ConfigMap", Some(options(100, None)))
        .await
        .is_err());
    storage.delete_one("ConfigMap", Some("ns"), "bad").await.unwrap();
    client
        .put_string(&format!("{prefix}/conf/ConfigMap/_/bad"), &document("ns", "bad"))
        .await
        .unwrap();
    assert!(storage.count_by_kind("ConfigMap", None).await.is_err());
    client.delete_prefix(&format!("{prefix}/")).await.unwrap();
}
