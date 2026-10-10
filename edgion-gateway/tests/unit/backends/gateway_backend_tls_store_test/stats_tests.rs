use super::*;

#[test]
fn empty_store_reports_zero() {
    let store = GatewayBackendTlsStore::new();
    assert_eq!(store.count(), 0);
    assert!(store.keys().is_empty());
}

#[test]
fn actual_gateway_publication_withdraws_invalid_pairs_and_preserves_raw_slots() {
    let store = GatewayBackendTlsStore::new();
    let base = serde_json::json!({"metadata":{"name":"edge","namespace":"apps"},"spec":{"gatewayClassName":"public",
        "tls":{"backend":{"clientCertificateRef":{"name":"client"},"resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps","resourceVersion":"same"}}}}}});
    let owner: Gateway = serde_json::from_value(base.clone()).unwrap();
    store.update_from_gateway("apps/edge", &owner);
    let captured = store.get("apps/edge").unwrap();
    assert_eq!(store.count(), 1);
    assert_eq!(store.keys(), ["apps/edge"]);
    for (path, value) in [
        ("/metadata/name", serde_json::json!(null)),
        ("/metadata/name", serde_json::json!("other")),
        ("/metadata/namespace", serde_json::json!("")),
        ("/spec/tls/backend/clientCertificateRef", serde_json::json!(null)),
        (
            "/spec/tls/backend/clientCertificateRef/name",
            serde_json::json!("retargeted"),
        ),
        ("/spec/tls/backend/resolvedClientCertificate", serde_json::json!(null)),
        (
            "/spec/tls/backend/resolvedClientCertificate/metadata/name",
            serde_json::json!(null),
        ),
        (
            "/spec/tls/backend/resolvedClientCertificate/metadata/name",
            serde_json::json!("other"),
        ),
        (
            "/spec/tls/backend/resolvedClientCertificate/metadata/namespace",
            serde_json::json!(null),
        ),
        (
            "/spec/tls/backend/resolvedClientCertificate/metadata/namespace",
            serde_json::json!(""),
        ),
        (
            "/spec/tls/backend/resolvedClientCertificate/metadata/namespace",
            serde_json::json!("other"),
        ),
    ] {
        let mut value_owner = base.clone();
        *value_owner.pointer_mut(path).unwrap() = value;
        store.update_from_gateway("apps/edge", &serde_json::from_value(value_owner).unwrap());
        assert!(store.get("apps/edge").is_none(), "retained {path}");
        assert_eq!(captured.metadata.name.as_deref(), Some("client"));
        store.update_from_gateway("apps/edge", &owner);
        assert!(!Arc::ptr_eq(&captured, &store.get("apps/edge").unwrap()));
    }
    store.remove("apps/edge");
    let mut bare = base;
    bare["metadata"].as_object_mut().unwrap().remove("namespace");
    bare["spec"]["tls"]["backend"]["resolvedClientCertificate"]["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("namespace");
    let bare: Gateway = serde_json::from_value(bare).unwrap();
    store.update_from_gateway("edge", &bare);
    assert!(store.get("edge").is_some());
    assert!(store.get("default/edge").is_none());
    store.update_from_gateway("default/edge", &bare);
    assert!(store.get("default/edge").is_none());
    assert!(store.get("edge").is_some());
}
