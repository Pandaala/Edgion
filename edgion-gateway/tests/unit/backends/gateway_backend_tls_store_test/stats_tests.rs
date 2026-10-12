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

#[cfg(feature = "boringssl")]
#[test]
fn prepared_identity_reuse_keeps_raw_generation_fresh_and_bad_replacements_present() {
    let store = GatewayBackendTlsStore::new();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["client.example".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap()
        .pem();
    let mut owner: Gateway = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"edge","namespace":"prepared"}, "spec":{"gatewayClassName":"public",
        "tls":{"backend":{"clientCertificateRef":{"name":"client"},"resolvedClientCertificate":{
            "metadata":{"name":"client","namespace":"prepared"},"data":{
                "tls.crt":k8s_openapi::ByteString(cert.into_bytes()),"tls.key":k8s_openapi::ByteString(key.serialize_pem().into_bytes())
            }
        }}}}
    })).unwrap();
    store.update_from_gateway("prepared/edge", &owner);
    let old = store.get_prepared("prepared/edge").unwrap();
    store.update_from_gateway("prepared/edge", &owner);
    let identical = store.get_prepared("prepared/edge").unwrap();
    assert!(!Arc::ptr_eq(&old.raw, &identical.raw));
    assert!(Arc::ptr_eq(&old.prepared, &identical.prepared));
    let material = owner
        .spec
        .tls
        .as_mut()
        .unwrap()
        .backend
        .as_mut()
        .unwrap()
        .resolved_client_certificate
        .as_mut()
        .unwrap();
    material.metadata.resource_version = Some("next".into());
    store.update_from_gateway("prepared/edge", &owner);
    let metadata = store.get_prepared("prepared/edge").unwrap();
    assert!(!Arc::ptr_eq(&identical.raw, &metadata.raw));
    assert!(Arc::ptr_eq(&identical.prepared, &metadata.prepared));
    owner
        .spec
        .tls
        .as_mut()
        .unwrap()
        .backend
        .as_mut()
        .unwrap()
        .resolved_client_certificate
        .as_mut()
        .unwrap()
        .data
        .as_mut()
        .unwrap()
        .insert("tls.key".into(), k8s_openapi::ByteString(b"invalid".to_vec()));
    store.update_from_gateway("prepared/edge", &owner);
    let bad = store.get_prepared("prepared/edge").unwrap();
    assert!(matches!(
        bad.prepared.identity,
        Err(edgion_resources::EdgionErrorCode::Unknown)
    ));
    assert!(!Arc::ptr_eq(&old.prepared, &bad.prepared));
    assert_ne!(old.prepared.fingerprint, bad.prepared.fingerprint);
    assert!(old.prepared.identity.is_ok());
    store.remove("prepared/edge");
    assert!(store.get("prepared/edge").is_none());
    assert!(bad.prepared.identity.is_err());
}
