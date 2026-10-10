use super::*;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ManagedFieldsEntry;

type CapturedRequests = std::sync::Arc<parking_lot::Mutex<Vec<(http::Method, serde_json::Value)>>>;

fn cleanup_api() -> (Client, CapturedRequests) {
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
    (kube::Client::new(transport, "default"), requests)
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

#[derive(Debug)]
struct IoRequest {
    method: http::Method,
    path: String,
    query: String,
    content_type: String,
    body: Value,
}

type IoRequests = std::sync::Arc<parking_lot::Mutex<Vec<IoRequest>>>;

fn response_client(responses: Vec<(u16, Value)>) -> (Client, IoRequests) {
    let responses = std::sync::Arc::new(parking_lot::Mutex::new(std::collections::VecDeque::from(responses)));
    let requests = IoRequests::default();
    let captured = requests.clone();
    let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
        let responses = responses.clone();
        let captured = captured.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.unwrap();
            captured.lock().push(IoRequest {
                method: parts.method,
                path: parts.uri.path().to_string(),
                query: parts.uri.query().unwrap_or_default().to_string(),
                content_type: parts
                    .headers
                    .get(http::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string(),
                body: if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                },
            });
            let (status, body) = responses.lock().pop_front().expect("unexpected extra API request");
            Ok::<_, std::convert::Infallible>(
                http::Response::builder()
                    .status(status)
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
        }
    });
    (Client::new(transport, "wrong-client-default"), requests)
}

fn assert_io_order(requests: &IoRequests, expected: &[(&str, &str)]) {
    let requests = requests.lock();
    let actual: Vec<_> = requests.iter().map(|r| (r.method.as_str(), r.path.as_str())).collect();
    assert_eq!(actual, expected);
}

fn status_response(code: u16) -> Value {
    json!({"apiVersion":"v1", "kind":"Status", "status":"Failure", "reason": if code == 404 { "NotFound" } else { "Conflict" }, "message":"test status", "code":code})
}

#[tokio::test]
async fn checked_api_responses_reject_each_raw_target_dimension_before_reuse() {
    const OBJECT: &str = "/api/v1/namespaces/tenant-a/secrets/provider-plugin";
    const COLLECTION: &str = "/api/v1/namespaces/tenant-a/secrets";
    for variant in 0..6 {
        let mut wrong = managed_secret();
        match variant {
            0 => wrong.metadata.name = None,
            1 => wrong.metadata.name = Some(String::new()),
            2 => wrong.metadata.name = Some("other-plugin".to_string()),
            3 => wrong.metadata.namespace = None,
            4 => wrong.metadata.namespace = Some(String::new()),
            _ => wrong.metadata.namespace = Some("ops".to_string()),
        }
        let identity = identity();
        assert!(!is_owned_by_consumer(&wrong, &identity));
        assert!(!can_adopt_create_race(&wrong, &identity));
        assert!(capture_publication_cas(&wrong, &identity).is_err());
        assert!(capture_delete_expectation(&wrong, identity.clone(), "binding-revision".into()).is_err());
        for operation in 0..6 {
            let (client, calls) = response_client(vec![(200, serde_json::to_value(&wrong).unwrap())]);
            let metadata = metadata();
            let publication = Publication {
                identity: &identity,
                metadata: &metadata,
                bearer_token: b"new-token",
            };
            let expected =
                capture_delete_expectation(&managed_secret(), identity.clone(), "binding-revision".into()).unwrap();
            let cas = capture_publication_cas(&managed_secret(), &identity).unwrap();
            let failed = match operation {
                0 => get_optional(&client, &identity).await.is_err(),
                1 => get_required(&client, &identity).await.is_err(),
                2 => create(&publication, &client).await.is_err(),
                3 => publish(&client, &cas, &publication).await.is_err(),
                4 => repair_and_publish_owned_consumer(&client, &managed_secret(), &publication)
                    .await
                    .is_err(),
                _ => delete_if_matches(&client, &expected, || async {
                    panic!("wrong target must not reach authorization")
                })
                .await
                .is_err(),
            };
            assert!(failed, "operation {operation}, dimension {variant}");
            let method = match operation {
                2 => "POST",
                3 | 4 => "PATCH",
                _ => "GET",
            };
            assert_io_order(&calls, &[(method, if operation == 2 { COLLECTION } else { OBJECT })]);
        }
        let (client, calls) = response_client(vec![]);
        let metadata = metadata();
        let publication = Publication {
            identity: &identity,
            metadata: &metadata,
            bearer_token: b"new-token",
        };
        assert!(repair_and_publish_owned_consumer(&client, &wrong, &publication)
            .await
            .is_err());
        assert!(delete_owned_consumer_object(&client, &wrong, &identity).await.is_err());
        assert_io_order(&calls, &[]);
    }
}

#[tokio::test]
async fn checked_publication_protocol_preserves_post_race_manager_and_json_cas() {
    const OBJECT: &str = "/api/v1/namespaces/tenant-a/secrets/provider-plugin";
    const COLLECTION: &str = "/api/v1/namespaces/tenant-a/secrets";
    let identity = identity();
    let metadata = metadata();
    let publication = Publication {
        identity: &identity,
        metadata: &metadata,
        bearer_token: b"new-token",
    };
    let mut winner = managed_secret();
    winner.metadata.managed_fields = Some(vec![ManagedFieldsEntry {
        manager: Some(FIELD_MANAGER.into()),
        ..Default::default()
    }]);
    for race in [false, true] {
        let mut responses = vec![(404, status_response(404))];
        if race {
            responses.extend([
                (409, status_response(409)),
                (200, serde_json::to_value(&winner).unwrap()),
            ]);
        }
        responses.push((200, serde_json::to_value(&winner).unwrap()));
        let (client, calls) = response_client(responses);
        assert!(get_optional(&client, &identity).await.unwrap().is_none());
        let result = match create(&publication, &client).await {
            Ok(secret) => secret,
            Err(SecretIoError::Kubernetes(kube::Error::Api(error))) if error.code == 409 => {
                let winner = get_required(&client, &identity).await.unwrap();
                assert!(can_adopt_create_race(&winner, &identity));
                let cas = capture_publication_cas(&winner, &identity).unwrap();
                publish(&client, &cas, &publication).await.unwrap()
            }
            Err(error) => panic!("unexpected protocol error: {error}"),
        };
        assert!(capture_delete_expectation(&result, identity.clone(), "binding-revision".into()).is_ok());
        let mut expected = vec![("GET", OBJECT), ("POST", COLLECTION)];
        if race {
            expected.extend([("GET", OBJECT), ("PATCH", OBJECT)]);
        }
        assert_io_order(&calls, &expected);
        let calls = calls.lock();
        assert!(calls[1].query.contains(&format!("fieldManager={FIELD_MANAGER}")));
        assert_eq!(calls[1].body["metadata"]["namespace"], "tenant-a");
        assert_eq!(calls[1].body["metadata"]["name"], "provider-plugin");
        assert_eq!(calls[1].body["metadata"]["ownerReferences"][0]["uid"], "consumer-uid");
        assert_eq!(
            calls[1].body["metadata"]["annotations"][ANNOTATION_SOURCE_NAME],
            "ops/provider-a"
        );
        assert_eq!(
            calls[1].body["data"][DATA_BEARER_TOKEN],
            base64::engine::general_purpose::STANDARD.encode(b"new-token")
        );
        if race {
            assert_eq!(calls[3].content_type, "application/json-patch+json");
            assert_eq!(
                calls[3].body,
                serde_json::to_value(
                    build_publication_patch(&capture_publication_cas(&winner, &identity).unwrap(), &publication)
                        .unwrap()
                )
                .unwrap()
            );
        }
    }
    winner.metadata.managed_fields = None;
    let (client, calls) = response_client(vec![
        (409, status_response(409)),
        (200, serde_json::to_value(winner).unwrap()),
    ]);
    assert!(
        matches!(create(&publication, &client).await, Err(SecretIoError::Kubernetes(kube::Error::Api(error))) if error.code == 409)
    );
    assert!(!can_adopt_create_race(
        &get_required(&client, &identity).await.unwrap(),
        &identity
    ));
    assert_io_order(&calls, &[("POST", COLLECTION), ("GET", OBJECT)]);
}

#[tokio::test]
async fn historical_cleanup_and_repair_use_exact_target_without_live_owner() {
    const OBJECT: &str = "/api/v1/namespaces/tenant-a/secrets/provider-plugin";
    let original = managed_secret();
    let expected = delete_expectation_from_secret(&original).unwrap();
    for old_source in [false, true] {
        let mut historical = original.clone();
        if old_source {
            historical
                .metadata
                .annotations
                .as_mut()
                .unwrap()
                .insert(ANNOTATION_SOURCE_NAME.into(), "old-ops/removed-source".into());
        }
        let expected = delete_expectation_from_secret(&historical).unwrap();
        for dimension in 0..3 {
            let mut response = historical.clone();
            if dimension == 1 {
                response.metadata.name = Some("replacement".into());
            }
            if dimension == 2 {
                response.metadata.namespace = Some("other-tenant".into());
            }
            let mut responses = vec![(200, serde_json::to_value(response).unwrap())];
            if dimension == 0 {
                responses.push((
                    200,
                    json!({"apiVersion":"v1","kind":"Status","status":"Success","code":200}),
                ));
            }
            let (client, calls) = response_client(responses);
            let result = delete_if_matches(&client, &expected, || async { Ok(true) }).await;
            if dimension == 0 {
                assert!(result.unwrap());
                assert_io_order(&calls, &[("GET", OBJECT), ("DELETE", OBJECT)]);
                assert_eq!(
                    calls.lock()[1].body["preconditions"],
                    json!({"uid":"secret-uid","resourceVersion":"42"})
                );
            } else {
                assert!(result.is_err());
                assert_io_order(&calls, &[("GET", OBJECT)]);
            }
        }
    }
    let metadata = metadata();
    let publication = Publication {
        identity: &expected.identity,
        metadata: &metadata,
        bearer_token: b"repaired-token",
    };
    let (client, calls) = response_client(vec![(200, serde_json::to_value(&original).unwrap())]);
    let mut repairable = original.clone();
    repairable.metadata.annotations = None;
    repair_and_publish_owned_consumer(&client, &repairable, &publication)
        .await
        .unwrap();
    assert_io_order(&calls, &[("PATCH", OBJECT)]);
    assert_eq!(
        calls.lock()[0].body[0],
        json!({"op":"test","path":"/metadata/resourceVersion","value":"42"})
    );
    assert_eq!(calls.lock()[0].body[1]["path"], "/metadata/ownerReferences");
    let (client, calls) = response_client(vec![
        (
            200,
            json!({"apiVersion":"v1","kind":"Status","status":"Success","code":200}),
        ),
        (200, serde_json::to_value(&original).unwrap()),
    ]);
    let mut immutable = original.clone();
    immutable.immutable = Some(true);
    delete_owned_consumer_object(&client, &immutable, &expected.identity)
        .await
        .unwrap();
    create(&publication, &client).await.unwrap();
    assert_io_order(
        &calls,
        &[("DELETE", OBJECT), ("POST", "/api/v1/namespaces/tenant-a/secrets")],
    );
    assert_eq!(
        calls.lock()[0].body["preconditions"],
        json!({"uid":"secret-uid","resourceVersion":"42"})
    );
}

#[test]
fn historical_identity_requires_observed_metadata_and_exact_markers() {
    for dimension in 0..7 {
        let mut secret = managed_secret();
        match dimension {
            0 => secret.metadata.namespace = None,
            1 => secret.metadata.namespace = Some(String::new()),
            2 => secret.metadata.name = None,
            3 => secret.metadata.name = Some("other".into()),
            4 => secret.metadata.owner_references = None,
            5 => {
                secret
                    .metadata
                    .annotations
                    .as_mut()
                    .unwrap()
                    .remove(ANNOTATION_SOURCE_UID);
            }
            _ => {
                secret
                    .metadata
                    .annotations
                    .as_mut()
                    .unwrap()
                    .insert(ANNOTATION_CONSUMER_GENERATION.into(), "invalid".into());
            }
        }
        assert!(
            delete_expectation_from_secret(&secret).is_none(),
            "dimension {dimension}"
        );
    }
}
