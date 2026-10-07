use super::*;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ManagedFieldsEntry;

type CapturedRequests = std::sync::Arc<parking_lot::Mutex<Vec<(http::Method, serde_json::Value)>>>;

fn cleanup_api() -> (Api<Secret>, CapturedRequests) {
    let requests = CapturedRequests::default();
    let captured = requests.clone();
    let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
        let captured = captured.clone();
        async move {
            let (parts, body) = request.into_parts();
            assert_eq!(parts.uri.path(), "/api/v1/namespaces/tenant-a/secrets/provider-plugin");
            let bytes = body.collect_bytes().await.unwrap();
            let body = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            captured.lock().push((parts.method.clone(), body));
            let response = match parts.method {
                http::Method::GET => serde_json::to_value(managed_secret()).unwrap(),
                http::Method::DELETE => {
                    json!({ "apiVersion": "v1", "kind": "Status", "status": "Success", "code": 200 })
                }
                method => panic!("unexpected Kubernetes method: {method}"),
            };
            Ok::<_, std::convert::Infallible>(
                http::Response::builder()
                    .status(200)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(kube::client::Body::from(serde_json::to_vec(&response).unwrap()))
                    .unwrap(),
            )
        }
    });
    (
        Api::namespaced(kube::Client::new(transport, "default"), "tenant-a"),
        requests,
    )
}

#[tokio::test]
async fn cleanup_rechecks_after_get_and_wait_without_losing_retry_semantics() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    for stale_worker in [false, true] {
        let (api, requests) = cleanup_api();
        let expected =
            capture_delete_expectation(&managed_secret(), identity(), "binding-revision".to_string()).unwrap();
        let eligible = Arc::new(AtomicBool::new(true));
        let checked = eligible.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let deletion = delete_if_matches(&api, &expected, move || async move {
            entered_tx.send(()).unwrap();
            resume_rx.await.unwrap();
            if stale_worker {
                Err(SecretIoError::StaleOperation)
            } else {
                Ok(checked.load(Ordering::SeqCst))
            }
        });
        let revoke = async {
            entered_rx.await.unwrap();
            assert_eq!(requests.lock().len(), 1);
            assert_eq!(requests.lock()[0].0, http::Method::GET);
            eligible.store(false, Ordering::SeqCst);
            resume_tx.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(deletion, revoke);
        if stale_worker {
            assert!(matches!(result, Err(SecretIoError::StaleOperation)));
        } else {
            assert!(!result.unwrap());
        }
        assert_eq!(requests.lock().len(), 1, "authorization loss must prevent DELETE");
    }
}

#[tokio::test]
async fn authorized_cleanup_preserves_secret_uid_and_resource_version_preconditions() {
    let (api, requests) = cleanup_api();
    let expected = capture_delete_expectation(&managed_secret(), identity(), "binding-revision".to_string()).unwrap();
    assert!(delete_if_matches(&api, &expected, || async { Ok(true) }).await.unwrap());
    let requests = requests.lock();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].0, http::Method::DELETE);
    assert_eq!(requests[1].1["preconditions"]["uid"], "secret-uid");
    assert_eq!(requests[1].1["preconditions"]["resourceVersion"], "42");
}

fn identity() -> ManagedSecretIdentity {
    ManagedSecretIdentity {
        consumer: ConsumerIdentity {
            namespace: "tenant-a".to_string(),
            name: "provider-plugin".to_string(),
            uid: "consumer-uid".to_string(),
            generation: 7,
        },
        source: SourceIdentity {
            name: "ops/provider-a".to_string(),
            uid: "source-uid".to_string(),
            generation: 3,
        },
    }
}

fn metadata() -> PublicationMetadata {
    PublicationMetadata {
        publication_generation: "publication-old".to_string(),
        source_input_revision: "input-revision".to_string(),
        binding_revision: "binding-revision".to_string(),
        issued_at: "2026-08-27T00:00:00Z".to_string(),
        valid_until: "2026-08-27T01:00:00Z".to_string(),
        leader_term: "leader-old".to_string(),
    }
}

fn managed_secret() -> Secret {
    let identity = identity();
    let metadata = metadata();
    let publication = Publication {
        identity: &identity,
        metadata: &metadata,
        bearer_token: b"not-logged",
    };
    let mut secret = build_secret(&publication);
    secret.metadata.resource_version = Some("42".to_string());
    secret.metadata.uid = Some("secret-uid".to_string());
    secret.metadata.managed_fields = Some(vec![ManagedFieldsEntry {
        manager: Some(FIELD_MANAGER.to_string()),
        ..Default::default()
    }]);
    secret
}

#[test]
fn ownership_requires_exact_consumer_owner_and_markers() {
    let identity = identity();
    let secret = managed_secret();
    assert_eq!(classify_ownership(&secret, &identity), Ownership::Owned);
    assert!(can_adopt_create_race(&secret, &identity));

    let mut wrong_owner = secret.clone();
    wrong_owner.metadata.owner_references.as_mut().unwrap()[0].uid = "recreated-consumer".to_string();
    assert_eq!(classify_ownership(&wrong_owner, &identity), Ownership::Conflict);

    let mut unmanaged = secret.clone();
    unmanaged.metadata.labels.as_mut().unwrap().clear();
    assert_eq!(classify_ownership(&unmanaged, &identity), Ownership::Conflict);
    assert!(is_owned_by_consumer(&unmanaged, &identity));
}

#[test]
fn same_consumer_with_old_source_is_not_a_general_conflict() {
    let mut secret = managed_secret();
    secret
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert(ANNOTATION_SOURCE_UID.to_string(), "old-source-uid".to_string());
    assert_eq!(
        classify_ownership(&secret, &identity()),
        Ownership::OwnedByConsumerForAnotherSource
    );
    assert!(!can_adopt_create_race(&secret, &identity()));
}

#[test]
fn create_race_adoption_requires_expected_field_manager() {
    let mut secret = managed_secret();
    secret.metadata.managed_fields = None;
    assert_eq!(classify_ownership(&secret, &identity()), Ownership::Owned);
    assert!(!can_adopt_create_race(&secret, &identity()));
}

#[test]
fn publication_patch_tests_old_state_before_atomic_replacement() {
    let identity = identity();
    let old_metadata = metadata();
    let new_metadata = PublicationMetadata {
        publication_generation: "publication-new".to_string(),
        leader_term: "leader-new".to_string(),
        ..old_metadata.clone()
    };
    let publication = Publication {
        identity: &identity,
        metadata: &new_metadata,
        bearer_token: b"new-token",
    };
    let cas = capture_publication_cas(&managed_secret(), &identity).unwrap();
    let patch = build_publication_patch(&cas, &publication).unwrap();
    let value = serde_json::to_value(patch).unwrap();
    let ops = value.as_array().unwrap();

    assert_eq!(
        ops[0],
        json!({"op":"test","path":"/metadata/resourceVersion","value":"42"})
    );
    assert_eq!(ops[1]["op"], "test");
    assert_eq!(ops[2]["value"], "consumer-uid");
    assert!(ops.iter().any(|op| {
        op["op"] == "test"
            && op["path"] == "/metadata/annotations/edgion.io~1credential-binding-revision"
            && op["value"] == "binding-revision"
    }));
    assert!(ops.iter().any(|op| op["path"] == "/metadata/annotations"));
    assert!(ops
        .iter()
        .any(|op| { op["path"] == "/data" && op["value"][DATA_BEARER_TOKEN] == "bmV3LXRva2Vu" }));
    assert!(ops.iter().any(|op| op["path"] == "/immutable" && op["value"] == false));
    assert!(ops.iter().any(|op| op["path"] == "/type" && op["value"] == "Opaque"));
}

#[test]
fn delete_uses_secret_uid_and_resource_version_preconditions() {
    let params = delete_params(&managed_secret()).unwrap();
    let preconditions = params.preconditions.unwrap();
    assert_eq!(preconditions.uid.as_deref(), Some("secret-uid"));
    assert_eq!(preconditions.resource_version.as_deref(), Some("42"));
}

#[test]
fn delete_expectation_captures_object_and_publication_identity() {
    let expected = capture_delete_expectation(&managed_secret(), identity(), "binding-revision".to_string()).unwrap();
    assert_eq!(expected.secret_uid, "secret-uid");
    assert_eq!(expected.publication_generation, "publication-old");
    assert_eq!(expected.source_input_revision, "input-revision");
    assert_eq!(expected.leader_term, "leader-old");
}
