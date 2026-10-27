use super::*;

#[test]
fn extract_account_credentials() {
    let mut data = BTreeMap::new();
    data.insert(
        ACME_ACCOUNT_KEY.to_string(),
        ByteString(br#"{"preferred":true}"#.to_vec()),
    );
    let z = extract_account_json_string(&data).unwrap().expect("Some");
    assert!(z.contains("preferred"));
}

#[test]
fn extract_legacy_key_is_missing() {
    let mut data = BTreeMap::new();
    data.insert("credentials".to_string(), ByteString(br#"{"only":"legacy"}"#.to_vec()));
    assert!(extract_account_json_string(&data).unwrap().is_none());
}

#[test]
fn extract_none_when_no_keys() {
    let data = BTreeMap::new();
    assert!(extract_account_json_string(&data).unwrap().is_none());
}

#[tokio::test]
async fn actual_account_patch_rejects_api_identity_before_io() {
    use kube::client::Body;
    use std::convert::Infallible;
    use tower::service_fn;
    let owner: edgion_resources::EdgionAcme = serde_json::from_value(serde_json::json!({"metadata":{"name":"issuer","namespace":"default"},"spec":{"email":"ops@example.com","domains":["example.com"],"privateKeySecretRef":{"name":"account"},"challenge":{"type":"http-01","gatewayRef":{"name":"edge"}},"storage":{"secretName":"cert"}}})).unwrap();
    let refs = edgion_resources::resources::refs::acme_fields::AcmeRefs::from_acme(&owner);
    let client = Client::new(
        service_fn(|_: http::Request<Body>| async {
            panic!("identity rejection must precede API IO");
            #[allow(unreachable_code)]
            Ok::<http::Response<Body>, Infallible>(http::Response::new(Body::empty()))
        }),
        "default",
    );
    let snapshot = Secret {
        metadata: kube::api::ObjectMeta {
            name: Some("decoy".into()),
            namespace: Some("default".into()),
            uid: Some("uid".into()),
            resource_version: Some("rv".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(patch_account_key_into_existing_secret(
        &client,
        &refs.account(),
        &snapshot,
        &Zeroizing::new("private".into())
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("does not match its declared reference"));
}

#[tokio::test]
async fn actual_account_create_uses_account_target_and_source_owner_metadata() {
    use kube::client::Body;
    use std::convert::Infallible;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tower::service_fn;
    for (source_namespace, account_namespace, uid, expected_namespace, expect_owner) in [
        (
            Some("source-ns"),
            Some("account-ns"),
            Some("issuer-uid"),
            "account-ns",
            false,
        ),
        (
            Some("source-ns"),
            Some("source-ns"),
            Some("issuer-uid"),
            "source-ns",
            true,
        ),
        (Some("source-ns"), None, Some("issuer-uid"), "source-ns", true),
        (Some("source-ns"), None, None, "source-ns", false),
        (None, None, Some("issuer-uid"), "default", true),
    ] {
        let owner: edgion_resources::EdgionAcme = serde_json::from_value(serde_json::json!({"metadata":{"name":"issuer","namespace":source_namespace,"uid":uid},"spec":{"email":"ops@example.com","domains":["example.com"],"privateKeySecretRef":{"name":"account-key","namespace":account_namespace},"externalAccountBinding":{"keyId":"kid","keySecretRef":{"name":"eab-key","namespace":"eab-ns"}},"challenge":{"type":"http-01","gatewayRef":{"name":"edge"}},"storage":{"secretName":"cert"}}})).unwrap();
        let refs = edgion_resources::resources::refs::acme_fields::AcmeRefs::from_acme(&owner);
        assert!(refs.eab().is_some());
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                let observed = observed.clone();
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(request.method(), http::Method::PATCH);
                    assert_eq!(
                        request.uri().path(),
                        format!("/api/v1/namespaces/{expected_namespace}/secrets/account-key")
                    );
                    let query = request.uri().query().unwrap();
                    assert!(query.contains("fieldManager=edgion-acme-controller"));
                    assert!(query.contains("force=true"));
                    assert_eq!(
                        request.headers()[http::header::CONTENT_TYPE],
                        "application/apply-patch+yaml"
                    );
                    let bytes = request.into_body().collect_bytes().await.unwrap();
                    let payload: Secret = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(payload.metadata.name.as_deref(), Some("account-key"));
                    assert_eq!(payload.metadata.namespace.as_deref(), Some(expected_namespace));
                    assert_eq!(
                        payload.metadata.labels.as_ref().unwrap(),
                        &BTreeMap::from([
                            (EDGION_IO_MANAGED_BY.into(), "acme".into()),
                            (EDGION_IO_ACME_RESOURCE.into(), "issuer".into()),
                        ])
                    );
                    // Exercise the actual API payload; live Kubernetes GC is a separate check.
                    if expect_owner {
                        assert_eq!(
                            payload.metadata.owner_references.as_ref().unwrap(),
                            &vec![OwnerReference {
                                api_version: "edgion.io/v1".into(),
                                kind: "EdgionAcme".into(),
                                name: "issuer".into(),
                                uid: "issuer-uid".into(),
                                controller: Some(true),
                                block_owner_deletion: Some(true),
                            }]
                        );
                    } else {
                        assert!(payload.metadata.owner_references.is_none());
                    }
                    assert_eq!(
                        payload.data.as_ref().unwrap(),
                        &BTreeMap::from([(ACME_ACCOUNT_KEY.into(), ByteString(b"account-sentinel".to_vec()))])
                    );
                    Ok::<_, Infallible>(http::Response::new(Body::from(serde_json::to_vec(&payload).unwrap())))
                }
            }),
            "unused-client-default",
        );
        create_account_secret(&client, &refs.account(), &Zeroizing::new("account-sentinel".into()))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
