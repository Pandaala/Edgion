use super::*;
use std::collections::BTreeMap;

/// Regression (found in cluster verification): an unforced apply 409s against
/// the POST-created CA Secret, wedging CA renewal forever. Remaining SSA writes
/// must stay forced; root renewal now uses a conditional Merge patch.
#[test]
fn apply_params_force_and_field_manager() {
    let pp = apply_params();
    assert!(
        pp.force,
        "SSA apply must be forced or CA renewal 409s (see apply_params docs)"
    );
    assert_eq!(pp.field_manager.as_deref(), Some(FIELD_MANAGER));
}

#[test]
fn ca_secret_data_preserves_key_across_renewal() {
    use crate::services::conf_sync_ca::Ca;
    // A CA renewal re-signs the cert with a fresh window but the SAME key.
    // The persisted Secret map MUST carry the new cert AND the original key.
    let ca = Ca::generate_root(::time::Duration::days(3650)).unwrap();
    let original_key = ca.ca_key_pem().to_string();
    let renewed = ca.renew_root_same_key(::time::Duration::days(3650)).unwrap();
    let data = ca_secret_data(&renewed);
    // Both fields present.
    assert!(data.contains_key("ca.crt"), "ca.crt must be present");
    assert!(data.contains_key("ca.key"), "ca.key must be present");
    // ca.crt is the renewed cert PEM.
    assert_eq!(
        data.get("ca.crt").unwrap(),
        &renewed.ca_cert_pem().as_bytes().to_vec(),
        "ca.crt must be the renewed certificate"
    );
    // ca.key is the ORIGINAL key bytes (renew keeps the key).
    assert_eq!(
        data.get("ca.key").unwrap(),
        &original_key.as_bytes().to_vec(),
        "ca.key must be preserved as the original root key bytes"
    );
    // And the cert actually changed (sanity: this was a real renewal write).
    assert_ne!(
        data.get("ca.crt").unwrap(),
        &ca.ca_cert_pem().as_bytes().to_vec(),
        "renewed cert should differ from the original cert"
    );
}

#[test]
fn build_secret_sets_name_ns_label_and_data() {
    let mut data = BTreeMap::new();
    data.insert("ca.crt".to_string(), b"X".to_vec());
    let s = build_secret("edgion-conf-sync-ca", "edgion-system", data);
    assert_eq!(s.metadata.name.as_deref(), Some("edgion-conf-sync-ca"));
    assert_eq!(s.metadata.namespace.as_deref(), Some("edgion-system"));
    assert!(s.metadata.labels.as_ref().unwrap().contains_key("edgion.io/managed-by"));
    assert!(s.data.as_ref().unwrap().contains_key("ca.crt"));
    assert_eq!(s.type_.as_deref(), Some("Opaque"));
}

/// Exercise the actual kube client wire contract. The transport supplies API
/// outcomes; it does not substitute for Kubernetes storage integration coverage.
#[tokio::test]
async fn renewal_binds_source_and_propagates_rejection() {
    for status in [200, 404, 409, 422, 500] {
        let original = Ca::generate_root(time::Duration::days(30)).unwrap();
        let mut source = build_secret(CA_SECRET, "test", ca_secret_data(&original));
        source.metadata.uid = Some("root-generation-0".into());
        source.metadata.resource_version = Some("123".into());
        let expected_data = source.data.clone().unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let captured_calls = calls.clone();
        let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            let source = source.clone();
            let expected_data = expected_data.clone();
            let call = captured_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                assert_eq!(
                    request.uri().path(),
                    format!("/api/v1/namespaces/test/secrets/{CA_SECRET}")
                );
                let (code, response) = if call == 0 {
                    assert_eq!(request.method(), http::Method::GET);
                    (200, serde_json::to_value(source).unwrap())
                } else {
                    assert_eq!(call, 1, "a failed candidate must not be retried or publish leaves");
                    assert_eq!(request.method(), http::Method::PATCH);
                    assert_eq!(
                        request.headers()[http::header::CONTENT_TYPE],
                        "application/merge-patch+json"
                    );
                    let bytes = request.into_body().collect_bytes().await.unwrap();
                    let patch: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(patch["metadata"]["uid"], "root-generation-0");
                    assert_eq!(patch["metadata"]["resourceVersion"], "123");
                    let data: BTreeMap<String, ByteString> = serde_json::from_value(patch["data"].clone()).unwrap();
                    assert_eq!(data["ca.key"], expected_data["ca.key"]);
                    assert_ne!(data["ca.crt"], expected_data["ca.crt"]);
                    if status == 200 {
                        let mut updated = source;
                        updated.data = Some(data);
                        (200, serde_json::to_value(updated).unwrap())
                    } else {
                        (
                            status,
                            serde_json::json!({
                                "apiVersion": "v1", "kind": "Status", "status": "Failure",
                                "code": status, "reason": "Rejected", "message": "renewal rejected"
                            }),
                        )
                    }
                };
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(code)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&response).unwrap()))
                        .unwrap(),
                )
            }
        });
        let api = Api::namespaced(kube::Client::new(transport, "test"), "test");
        let result = renew_ca(&api, time::Duration::days(365)).await;
        assert_eq!(result.is_ok(), status == 200);
        if let Ok(renewed) = result {
            assert_eq!(renewed.ca_key_pem(), original.ca_key_pem());
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn renewal_rejects_missing_source_identity_without_writing() {
    for missing in ["uid", "resourceVersion"] {
        let ca = Ca::generate_root(time::Duration::days(30)).unwrap();
        let mut source = build_secret(CA_SECRET, "test", ca_secret_data(&ca));
        source.metadata.uid = (missing != "uid").then(|| "root-0".into());
        source.metadata.resource_version = (missing != "resourceVersion").then(|| "123".into());
        let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(
                request.method(),
                http::Method::GET,
                "must not write without source identity"
            );
            let response = serde_json::to_vec(&source).unwrap();
            async move {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(200)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(response))
                        .unwrap(),
                )
            }
        });
        let api = Api::namespaced(kube::Client::new(transport, "test"), "test");
        assert!(renew_ca(&api, time::Duration::days(365)).await.is_err());
    }
}
