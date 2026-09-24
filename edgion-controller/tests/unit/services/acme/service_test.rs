use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types,
};
use crate::conf_mgr::sync_runtime::resource_processor::{ProcessorHandler, ResourceProcessor};
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::edgion_tls::GatewayAttachmentProof;
use edgion_resources::resources::gateway::{GatewaySpec, Listener};
use http::{Response, StatusCode};
use kube::client::Body;
use serde_json::json;
use std::convert::Infallible;
use tower::service_fn;

struct NoopGatewayHandler;

#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for NoopGatewayHandler {}

struct NoopAcmeHandler;

#[async_trait::async_trait]
impl ProcessorHandler<EdgionAcme> for NoopAcmeHandler {}

fn test_gateway(proof_epoch: &str) -> Gateway {
    Gateway {
        metadata: kube::api::ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("gw".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: GatewaySpec {
            current_status: Some(
                serde_json::from_value(serde_json::json!({
                    "listeners": [{"name":"http", "supportedKinds":[], "attachedRoutes":0, "conditions":[]}]
                }))
                .unwrap(),
            ),
            gateway_class_name: "edgion".to_string(),
            listeners: Some(vec![Listener {
                name: "http".to_string(),
                hostname: None,
                port: 80,
                protocol: "HTTP".to_string(),
                tls: None,
                allowed_routes: None,
            }]),
            addresses: None,
            tls: None,
            allowed_listeners: None,
            infrastructure: None,
            default_scope: None,
            resolved_inbound_proxy_protocol: Default::default(),
            resolved_attachment_proof: Some(GatewayAttachmentProof {
                instance: "test-instance".to_string(),
                generation: 1,
                epoch: proof_epoch.to_string(),
                attachment_digest: "a".repeat(64),
            }),
        },
        status: None,
    }
}

fn test_acme() -> EdgionAcme {
    serde_yaml::from_str(
        r#"
apiVersion: edgion.io/v1
kind: EdgionAcme
metadata:
  namespace: default
  name: test
  uid: uid-1
  generation: 1
spec:
  email: admin@example.com
  privateKeySecretRef:
    name: account
  domains:
    - example.com
  challenge:
    type: http-01
    gatewayRef:
      name: gw
  storage:
    secretName: certificate
"#,
    )
    .expect("test EdgionAcme must deserialize")
}

fn eligibility_runtime(processed: &mut EdgionAcme) -> Arc<ControllerRuntime> {
    let runtime = Arc::new(ControllerRuntime::default());
    let gateway_processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        8,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    gateway_processor.save(test_gateway("1"), None);
    runtime.processor_registry.register(gateway_processor);

    let gateway_ref = match &processed.spec.challenge {
        AcmeChallenge::Http01 { gateway_ref } => gateway_ref,
    };
    processed.spec.resolved_listener_attachments = Some(
        resolve_acme_listener_attachments(gateway_ref, "default", &runtime)
            .expect("test Gateway attachment must resolve"),
    );
    let generation = processed.metadata.generation;
    let status = processed.spec.current_status.get_or_insert_with(Default::default);
    status.conditions = vec![
        condition_true(
            condition_types::ACCEPTED,
            condition_reasons::ACCEPTED,
            "Resource accepted",
            generation,
        ),
        condition_true(
            condition_types::RESOLVED_REFS,
            condition_reasons::RESOLVED_REFS,
            "References resolved",
            generation,
        ),
    ];
    let acme_processor = Arc::new(ResourceProcessor::<EdgionAcme>::new(
        "EdgionAcme",
        8,
        Arc::new(NoopAcmeHandler),
        runtime.clone(),
    ));
    acme_processor.save(processed.clone(), None);
    runtime.processor_registry.register(acme_processor);
    runtime
}

fn test_attempt(client: Client, acme: EdgionAcme, runtime: Arc<ControllerRuntime>, generation: u64) -> AcmeAttempt {
    let serving_attachments = admitted_serving_attachments("default/test", &runtime).unwrap_or_default();
    AcmeAttempt {
        attempt_id: "test-attempt".to_string(),
        client,
        acme,
        runtime,
        leader: None,
        leader_fence: None,
        serving_attachments,
        certificate_issued: Arc::new(AtomicBool::new(false)),
        generation,
        cancel: CancellationToken::new(),
    }
}

fn empty_acme_list() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "apiVersion": "edgion.io/v1",
                "kind": "EdgionAcmeList",
                "metadata": {"resourceVersion": "1"},
                "items": []
            }))
            .expect("response JSON must serialize"),
        ))
        .expect("response must build")
}

#[tokio::test]
async fn certificate_publication_rechecks_authority_between_writes() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    // Simulate a completed CA request, then change the resource before
    // publication or between its writes. No live CA or sleeps are needed.
    for (scenario, expected_paths) in [
        (
            "proof-feedback",
            vec![
                "/api/v1/namespaces/default/secrets/certificate",
                "/apis/edgion.io/v1/namespaces/default/edgiontls/acme-test",
                "/apis/edgion.io/v1/namespaces/default/edgionacmes/test/status",
            ],
        ),
        (
            "current",
            vec![
                "/api/v1/namespaces/default/secrets/certificate",
                "/apis/edgion.io/v1/namespaces/default/edgiontls/acme-test",
                "/apis/edgion.io/v1/namespaces/default/edgionacmes/test/status",
            ],
        ),
        ("uid", vec![]),
        ("generation", vec![]),
        ("deleted", vec![]),
        ("get-error", vec![]),
        ("stopped", vec![]),
        ("stop-during-get", vec![]),
        ("ownership", vec![]),
        ("after-secret", vec!["/api/v1/namespaces/default/secrets/certificate"]),
        (
            "after-tls",
            vec![
                "/api/v1/namespaces/default/secrets/certificate",
                "/apis/edgion.io/v1/namespaces/default/edgiontls/acme-test",
            ],
        ),
    ] {
        let capture = NotificationCapture::install();
        let mut acme = test_acme();
        acme.metadata.resource_version = Some("10".into());
        acme.spec.auto_edgion_tls.enabled = true;
        let runtime = eligibility_runtime(&mut acme.clone());
        if scenario == "ownership" {
            runtime
                .processor_registry
                .register(Arc::new(ResourceProcessor::<Gateway>::new(
                    "Gateway",
                    8,
                    Arc::new(NoopGatewayHandler),
                    runtime.clone(),
                )));
        }
        if scenario == "stopped" {
            ACME_SERVICE.lock().take();
        }
        let writes = Arc::new(Mutex::new(Vec::new()));
        let recorded = writes.clone();
        let mut current = acme.clone();
        // A status-only resourceVersion change does not retire an attempt.
        current.metadata.resource_version = Some("11".into());
        if scenario == "uid" {
            current.metadata.uid = Some("replacement".into());
        }
        if scenario == "generation" {
            current.metadata.generation = Some(2);
        }
        let feedback_runtime = runtime.clone();
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                let recorded = recorded.clone();
                let mut current = current.clone();
                let feedback_runtime = feedback_runtime.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    let value = if parts.method == http::Method::GET && parts.uri.path().contains("/edgionacmes/") {
                        if scenario == "stop-during-get" {
                            ACME_SERVICE.lock().take();
                        }
                        if scenario == "deleted" || scenario == "get-error" {
                            let code = if scenario == "deleted" { 404 } else { 503 };
                            return Ok::<_, Infallible>(
                                Response::builder()
                                    .status(code)
                                    .header(http::header::CONTENT_TYPE, "application/json")
                                    .body(Body::from(
                                        json!({"apiVersion":"v1", "kind":"Status", "status":"Failure",
                                    "message":"test read failure", "reason":"Unavailable", "code":code})
                                        .to_string()
                                        .into_bytes(),
                                    ))
                                    .unwrap(),
                            );
                        }
                        let count = recorded.lock().len();
                        if (scenario == "after-secret" && count >= 1) || (scenario == "after-tls" && count >= 2) {
                            current.metadata.generation = Some(2);
                        }
                        serde_json::to_value(current).unwrap()
                    } else if parts.method == http::Method::GET {
                        return Ok::<_, Infallible>(
                            Response::builder()
                                .status(404)
                                .header(http::header::CONTENT_TYPE, "application/json")
                                .body(Body::from(
                                    json!({"apiVersion":"v1", "kind":"Status", "status":"Failure",
                                            "message":"not found", "reason":"NotFound", "code":404})
                                    .to_string()
                                    .into_bytes(),
                                ))
                                .unwrap(),
                        );
                    } else {
                        assert_eq!(parts.method, http::Method::PATCH);
                        let value: serde_json::Value =
                            serde_json::from_slice(&body.collect_bytes().await.unwrap()).unwrap();
                        recorded.lock().push((parts.uri.path().to_string(), value.clone()));
                        if scenario == "proof-feedback" && parts.uri.path().ends_with("/secrets/certificate") {
                            refresh_test_serving_proof(&feedback_runtime);
                        }
                        if parts.uri.path().ends_with("/status") {
                            let mut response = serde_json::to_value(current).unwrap();
                            response["status"] = value["status"].clone();
                            response
                        } else {
                            value
                        }
                    };
                    Ok::<_, Infallible>(
                        Response::builder()
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Body::from(value.to_string().into_bytes()))
                            .unwrap(),
                    )
                }
            }),
            "default",
        );
        let api = Api::namespaced(client.clone(), "default");
        let attempt = test_attempt(client.clone(), acme, runtime, capture.generation);
        attempt.certificate_issued.store(true, Ordering::Release);
        let result = publish_certificate(
            &client,
            &api,
            &attempt,
            &AcmeCertificateResult {
                certificate_pem: "test certificate".into(),
                private_key_pem: Zeroizing::new("test key".into()),
            },
            "test-account",
        )
        .await;
        assert_eq!(
            result.is_ok(),
            matches!(scenario, "current" | "proof-feedback"),
            "{scenario}: {result:?}"
        );
        let writes = writes.lock();
        let paths: Vec<_> = writes.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(paths, expected_paths, "{scenario}");
        if scenario == "current" {
            assert_eq!(writes[2].1["metadata"]["uid"], "uid-1");
            assert_eq!(writes[2].1["metadata"]["resourceVersion"], "11");
            assert_eq!(writes[2].1["status"]["phase"], "Ready");
        }
    }
}

fn refresh_test_serving_proof(runtime: &Arc<ControllerRuntime>) {
    let gateway = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        8,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    let mut refreshed = test_gateway("certificate-reload");
    refreshed
        .spec
        .resolved_attachment_proof
        .as_mut()
        .unwrap()
        .attachment_digest = "b".repeat(64);
    gateway.save(refreshed, None);
    runtime.processor_registry.register(gateway);
    let mut processed = runtime
        .processor_registry
        .get("EdgionAcme")
        .unwrap()
        .get_cached_any("default/test")
        .unwrap()
        .downcast::<EdgionAcme>()
        .unwrap()
        .as_ref()
        .clone();
    let AcmeChallenge::Http01 { gateway_ref } = &processed.spec.challenge;
    processed.spec.resolved_listener_attachments =
        Some(resolve_acme_listener_attachments(gateway_ref, "default", runtime).unwrap());
    let processor = Arc::new(ResourceProcessor::<EdgionAcme>::new(
        "EdgionAcme",
        8,
        Arc::new(NoopAcmeHandler),
        runtime.clone(),
    ));
    processor.save(processed, None);
    runtime.processor_registry.register(processor);
}

#[tokio::test(start_paused = true)]
async fn operation_guards_retain_local_state_through_transient_reads() {
    let _lock = ACME_TEST_MUTEX.lock().await;
    let capture = NotificationCapture::install();
    let acme = test_acme();
    let runtime = eligibility_runtime(&mut acme.clone());
    let fail_next = Arc::new(AtomicBool::new(false));
    let response_fail = fail_next.clone();
    let response_acme = acme.clone();
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| {
            assert_eq!(request.method(), http::Method::GET);
            let fail = response_fail.swap(false, Ordering::SeqCst);
            let body = serde_json::to_value(&response_acme).unwrap();
            async move {
                Ok::<_, Infallible>(test_api_response(
                    if fail { 503 } else { 200 },
                    if fail {
                        json!({"apiVersion":"v1", "kind":"Status", "status":"Failure", "reason":"Unavailable", "code":503})
                    } else {
                        body
                    },
                ))
            }
        }),
        "default",
    );
    let attempt = test_attempt(client, acme, runtime, capture.generation);
    let mut completed_operations = Vec::new();
    for operation in [
        "activate HTTP-01 challenge",
        "finalize ACME order",
        "persist ACME account credentials",
    ] {
        fail_next.store(true, Ordering::SeqCst);
        let before = Instant::now();
        attempt.check(operation).await.unwrap();
        assert!(Instant::now().duration_since(before) >= CONTROL_PLANE_RECHECK_DELAY);
        completed_operations.push(operation);
    }
    assert_eq!(completed_operations.len(), 3);
    fail_next.store(true, Ordering::SeqCst);
    let check = attempt.check("finalize ACME order");
    tokio::pin!(check);
    tokio::select! { result = &mut check => panic!("guard must await uncertainty: {result:?}"), _ = tokio::task::yield_now() => {} }
    attempt.cancel.cancel();
    assert!(check.await.is_err());
}

fn test_api_response(code: u16, value: serde_json::Value) -> Response<Body> {
    Response::builder()
        .status(code)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string().into_bytes()))
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn account_secret_conflict_adopts_concurrent_credentials_or_retries_empty_snapshot() {
    let _lock = ACME_TEST_MUTEX.lock().await;
    let capture = NotificationCapture::install();
    for scenario in ["credentials", "empty", "replaced", "corrupt"] {
        let acme = test_acme();
        let runtime = eligibility_runtime(&mut acme.clone());
        let snapshot: Secret = serde_json::from_value(json!({"apiVersion":"v1", "kind":"Secret",
                "metadata":{"name":"account", "namespace":"default", "uid":"account-uid", "resourceVersion":"10"}}))
        .unwrap();
        // Deserialization is the production validation boundary here;
        // account restoration itself remains the ACME client's operation.
        let winner = json!({"id":"https://ca.example/account/winner", "key_pkcs8":"AQID", "directory":"https://ca.example/directory"}).to_string();
        let mut refreshed = snapshot.clone();
        refreshed.metadata.resource_version = Some("11".into());
        if scenario == "replaced" {
            refreshed.metadata.uid = Some("replacement-uid".into());
        }
        if matches!(scenario, "credentials" | "corrupt") {
            refreshed.data = Some(BTreeMap::from([(
                edgion_resources::credentials::ACME_ACCOUNT_KEY.to_string(),
                ByteString(if scenario == "credentials" {
                    winner.as_bytes().to_vec()
                } else {
                    b"invalid".to_vec()
                }),
            )]));
        }
        let patches = Arc::new(Mutex::new(Vec::new()));
        let recorded = patches.clone();
        let response_acme = acme.clone();
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                let recorded = recorded.clone();
                let refreshed = refreshed.clone();
                let response_acme = response_acme.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    if parts.uri.path().contains("/edgionacmes/") {
                        assert_eq!(parts.method, http::Method::GET);
                        return Ok::<_, Infallible>(test_api_response(
                            200,
                            serde_json::to_value(response_acme).unwrap(),
                        ));
                    }
                    if parts.method == http::Method::PATCH {
                        let patch: serde_json::Value =
                            serde_json::from_slice(&body.collect_bytes().await.unwrap()).unwrap();
                        assert_eq!(patch["metadata"]["uid"], "account-uid");
                        let mut patches = recorded.lock();
                        assert_eq!(
                            patch["metadata"]["resourceVersion"],
                            if patches.is_empty() { "10" } else { "11" }
                        );
                        patches.push(patch);
                        if patches.len() == 1 {
                            return Ok(test_api_response(
                                409,
                                json!({"apiVersion":"v1", "kind":"Status", "status":"Failure", "reason":"Conflict", "code":409}),
                            ));
                        }
                    }
                    Ok(test_api_response(200, serde_json::to_value(refreshed).unwrap()))
                }
            }),
            "default",
        );
        let api = Api::namespaced(client.clone(), "default");
        let attempt = test_attempt(client, acme, runtime, capture.generation);
        let result = persist_registered_account(
            &attempt,
            &api,
            "account",
            snapshot,
            &Zeroizing::new("newly registered credentials".into()),
        )
        .await;
        match scenario {
            "credentials" => {
                let adopted = result.unwrap().unwrap();
                assert_eq!(
                    serde_json::to_value(adopted).unwrap()["id"],
                    "https://ca.example/account/winner"
                );
                assert_eq!(patches.lock().len(), 1);
            }
            "empty" => {
                assert!(result.unwrap().is_none());
                assert_eq!(patches.lock().len(), 2);
            }
            _ => {
                assert!(result.is_err());
                assert_eq!(patches.lock().len(), 1);
            }
        }
    }
}

#[tokio::test]
async fn resource_notification_preserves_issued_certificate_but_cancels_preissuance() {
    let _lock = ACME_TEST_MUTEX.lock().await;
    let capture = NotificationCapture::install();
    let acme = test_acme();
    let runtime = eligibility_runtime(&mut acme.clone());
    let client = Client::new(
        service_fn(|_: http::Request<Body>| async { Ok::<_, Infallible>(test_api_response(500, json!({}))) }),
        "default",
    );
    let mut worker = AcmeServiceWorker {
        client,
        runtime,
        leader: None,
        generation: capture.generation,
        cancel: CancellationToken::new(),
        schedules: HashMap::new(),
        jobs: FuturesUnordered::new(),
    };
    for issued in [false, true] {
        let cancel = CancellationToken::new();
        worker.schedules.insert(
            "default/test".into(),
            ResourceSchedule {
                busy: true,
                active_cancel: Some(cancel.clone()),
                certificate_issued: Some(Arc::new(AtomicBool::new(issued))),
                ..Default::default()
            },
        );
        worker.enqueue_check("default/test".into(), TriggerKind::Resource);
        assert_eq!(cancel.is_cancelled(), !issued);
        assert_eq!(worker.schedules["default/test"].pending, Some(TriggerKind::Resource));
        assert!(worker.jobs.is_empty());
    }
}

#[tokio::test]
async fn exhausted_interrupted_attempt_recovers_failed_without_reservation() {
    let _lock = ACME_TEST_MUTEX.lock().await;
    let capture = NotificationCapture::install();
    for phase in [AcmeCertPhase::Issuing, AcmeCertPhase::Renewing] {
        let mut acme = test_acme();
        acme.metadata.resource_version = Some("10".into());
        acme.status = Some(EdgionAcmeStatus {
            phase,
            scheduler: Some(AcmeSchedulerStatus {
                observed_generation: 1,
                attempts_used: MAX_RETRY_ATTEMPTS,
                attempt_id: Some("interrupted".into()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let runtime = eligibility_runtime(&mut acme.clone());
        let patches = Arc::new(Mutex::new(Vec::new()));
        let fail_write = Arc::new(AtomicBool::new(true));
        let recorded = patches.clone();
        let response_fail = fail_write.clone();
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                let acme = acme.clone();
                let recorded = recorded.clone();
                let response_fail = response_fail.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    if parts.method == http::Method::PATCH {
                        let patch: serde_json::Value =
                            serde_json::from_slice(&body.collect_bytes().await.unwrap()).unwrap();
                        assert_eq!(patch["status"]["phase"], "Failed");
                        assert!(patch["status"].get("scheduler").is_none());
                        assert_eq!(patch["metadata"]["uid"], "uid-1");
                        recorded.lock().push(patch);
                        if response_fail.swap(false, Ordering::SeqCst) {
                            return Ok::<_, Infallible>(test_api_response(
                                503,
                                json!({"kind":"Status", "apiVersion":"v1", "status":"Failure", "reason":"Unavailable", "code":503}),
                            ));
                        }
                    }
                    Ok::<_, Infallible>(test_api_response(200, serde_json::to_value(acme).unwrap()))
                }
            }),
            "default",
        );
        for expected_retry in [true, false] {
            let outcome = check_resource(
                client.clone(),
                runtime.clone(),
                None,
                capture.generation,
                CancellationToken::new(),
                "default/test",
                TriggerKind::Startup,
            )
            .await;
            assert_eq!(matches!(outcome, CheckOutcome::Recheck(_, _)), expected_retry);
            if !expected_retry {
                assert!(matches!(outcome, CheckOutcome::Idle));
            }
        }
        assert_eq!(patches.lock().len(), 2);
    }
}

#[tokio::test]
async fn retired_attempt_cannot_publish_challenges_or_failure() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    let capture = NotificationCapture::install();
    let acme = test_acme();
    let runtime = eligibility_runtime(&mut acme.clone());
    let mut replacement = acme.clone();
    replacement.metadata.uid = Some("replacement".into());
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| {
            assert_eq!(request.method(), http::Method::GET, "retired attempt must not write");
            let body = serde_json::to_vec(&replacement).unwrap();
            async move {
                Ok::<_, Infallible>(
                    Response::builder()
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
            }
        }),
        "default",
    );
    let api = Api::namespaced(client.clone(), "default");
    let attempt = test_attempt(client, acme, runtime, capture.generation);
    let challenges = [ActiveHttpChallenge {
        domain: "example.com".into(),
        token: "test-token".into(),
        key_authorization: "test-authorization".into(),
        expire_at: 600,
    }];
    assert!(patch_active_challenges(&api, &attempt, "test", Some(&challenges))
        .await
        .is_err());
    assert!(patch_active_challenges(&api, &attempt, "test", None).await.is_err());
    assert!(
        update_acme_status(&api, &attempt, "test", AcmeCertPhase::Failed, |_| {})
            .await
            .is_err()
    );
}

#[tokio::test]
async fn admitted_attempt_rejects_a_new_serving_proof() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    let capture = NotificationCapture::install();
    let acme = test_acme();
    let mut processed = acme.clone();
    let runtime = eligibility_runtime(&mut processed);
    let client = Client::new(
        service_fn({
            let acme = acme.clone();
            move |_request: http::Request<Body>| {
                let body = serde_json::to_vec(&acme).unwrap();
                async move {
                    Ok::<_, Infallible>(
                        Response::builder()
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Body::from(body))
                            .unwrap(),
                    )
                }
            }
        }),
        "default",
    );
    let attempt = test_attempt(client.clone(), acme.clone(), runtime.clone(), capture.generation);

    let replacement_gateway = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        8,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    replacement_gateway.save(test_gateway("2"), None);
    runtime.processor_registry.register(replacement_gateway);
    let gateway_ref = match &processed.spec.challenge {
        AcmeChallenge::Http01 { gateway_ref } => gateway_ref,
    };
    processed.spec.resolved_listener_attachments = Some(
        resolve_acme_listener_attachments(gateway_ref, "default", &runtime).expect("replacement proof must resolve"),
    );
    let replacement_acme = Arc::new(ResourceProcessor::<EdgionAcme>::new(
        "EdgionAcme",
        8,
        Arc::new(NoopAcmeHandler),
        runtime.clone(),
    ));
    replacement_acme.save(processed, None);
    runtime.processor_registry.register(replacement_acme);

    let api = Api::namespaced(client, "default");
    assert!(matches!(
        attempt.current_resource_typed(&api).await,
        Err(AttemptAuthorityError::Lost(_))
    ));
    attempt.certificate_issued.store(true, Ordering::Release);
    assert!(attempt.current_resource_typed(&api).await.is_ok());
    let current = admitted_serving_attachments("default/test", &runtime).unwrap();
    for change in ["gateway-uid", "gateway-generation", "listener", "selection"] {
        let mut changed = current.clone();
        match change {
            "gateway-uid" => changed[0].gateway_proof.instance = "replacement".into(),
            "gateway-generation" => changed[0].gateway_proof.generation += 1,
            "listener" => changed[0].listener_name = "other".into(),
            _ => changed.clear(),
        }
        assert!(
            !same_serving_selection(&attempt.serving_attachments, &changed),
            "{change}"
        );
    }
}

#[tokio::test]
async fn ready_recovery_clears_stale_scheduler_checkpoint() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    let capture = NotificationCapture::install();
    let mut acme = test_acme();
    acme.metadata.resource_version = Some("10".into());
    let runtime = eligibility_runtime(&mut acme.clone());
    acme.status = Some(EdgionAcmeStatus {
        phase: AcmeCertPhase::Ready,
        certificate_not_after: Some("2099-01-01T00:00:00Z".into()),
        scheduler: Some(AcmeSchedulerStatus {
            observed_generation: 1,
            attempt_id: Some("completed-attempt".into()),
            attempts_used: 4,
            attempt_started_at: Some("2026-01-01T00:00:00Z".into()),
            next_retry_at: Some("2098-01-01T00:00:00Z".into()),
        }),
        ..Default::default()
    });
    let patches = Arc::new(Mutex::new(Vec::new()));
    let recorded = patches.clone();
    let response_acme = acme.clone();
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| {
            let recorded = recorded.clone();
            let response_acme = response_acme.clone();
            async move {
                let (parts, body) = request.into_parts();
                if parts.method == http::Method::PATCH {
                    let bytes = body.collect_bytes().await.unwrap();
                    let patch = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
                    recorded.lock().push(patch);
                }
                Ok::<_, Infallible>(
                    Response::builder()
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&response_acme).unwrap()))
                        .unwrap(),
                )
            }
        }),
        "default",
    );
    let outcome = check_resource(
        client,
        runtime,
        None,
        capture.generation,
        CancellationToken::new(),
        "default/test",
        TriggerKind::Startup,
    )
    .await;
    assert!(matches!(outcome, CheckOutcome::Wait(_, TriggerKind::Renewal)));
    let patches = patches.lock();
    assert_eq!(patches.len(), 1);
    assert!(patches[0]["status"]["scheduler"].is_null());
}

#[tokio::test]
async fn terminating_resource_does_not_start_an_attempt() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    let capture = NotificationCapture::install();
    let mut acme = test_acme();
    acme.metadata.deletion_timestamp = Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(chrono::Utc::now()));
    let runtime = eligibility_runtime(&mut acme.clone());
    let reads = Arc::new(AtomicU64::new(0));
    let read_count = reads.clone();
    let client = Client::new(
        service_fn(move |request: http::Request<Body>| {
            assert_eq!(request.method(), http::Method::GET);
            read_count.fetch_add(1, Ordering::SeqCst);
            let body = serde_json::to_vec(&acme).unwrap();
            async move {
                Ok::<_, Infallible>(
                    Response::builder()
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
            }
        }),
        "default",
    );
    let outcome = check_resource(
        client,
        runtime,
        None,
        capture.generation,
        CancellationToken::new(),
        "default/test",
        TriggerKind::Resource,
    )
    .await;
    assert!(matches!(outcome, CheckOutcome::Deleted));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn resource_get_classifies_not_found_and_transient_failure() {
    for (code, deleted) in [(404, true), (503, false)] {
        let client = Client::new(
            service_fn(move |_request: http::Request<Body>| async move {
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(code)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            json!({
                                "apiVersion": "v1",
                                "kind": "Status",
                                "status": "Failure",
                                "message": "test read failure",
                                "reason": if code == 404 { "NotFound" } else { "Unavailable" },
                                "code": code,
                            })
                            .to_string()
                            .into_bytes(),
                        ))
                        .unwrap(),
                )
            }),
            "default",
        );
        let trigger = if code == 503 {
            TriggerKind::Admin
        } else {
            TriggerKind::Startup
        };
        let outcome = check_resource(
            client,
            Arc::new(ControllerRuntime::default()),
            None,
            1,
            CancellationToken::new(),
            "default/test",
            trigger,
        )
        .await;
        assert_eq!(matches!(&outcome, CheckOutcome::Deleted), deleted, "HTTP {code}");
        assert_eq!(
            matches!(&outcome, CheckOutcome::Recheck(delay, retry_trigger)
                    if *delay == CONTROL_PLANE_RECHECK_DELAY && *retry_trigger == trigger),
            !deleted,
            "HTTP {code}"
        );
    }
}

#[tokio::test]
async fn startup_scan_lists_only_configured_namespaces() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&paths);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            captured.lock().push(request.uri().path().to_string());
            Ok::<_, Infallible>(empty_acme_list())
        }
    });

    let client = Client::new(service, "default");
    let items = list_acme_resources(
        client,
        &["team-a".into(), "team-b".into()],
        &kube::api::ListParams::default(),
    )
    .await
    .expect("namespace-scoped lists must succeed");

    assert!(items.is_empty());
    assert_eq!(
        *paths.lock(),
        vec![
            "/apis/edgion.io/v1/namespaces/team-a/edgionacmes",
            "/apis/edgion.io/v1/namespaces/team-b/edgionacmes"
        ]
    );
}

// ================================================================
// exponential_backoff
// ================================================================

#[test]
fn test_exponential_backoff_sequence() {
    // base=300s, multiplier=4^attempt
    assert_eq!(exponential_backoff(300, 0), Duration::from_secs(300)); // 5min
    assert_eq!(exponential_backoff(300, 1), Duration::from_secs(1200)); // 20min
    assert_eq!(exponential_backoff(300, 2), Duration::from_secs(4800)); // 80min
    assert_eq!(exponential_backoff(300, 3), Duration::from_secs(19200)); // 5.3h
    assert_eq!(exponential_backoff(300, 4), Duration::from_secs(76800)); // 21.3h
}

#[test]
fn test_exponential_backoff_saturates() {
    // Very large attempt should not overflow (saturating_pow + saturating_mul)
    let result = exponential_backoff(300, 100);
    assert!(result.as_secs() > 0); // Should not be zero/wrap
}

#[test]
fn issuance_eligibility_missing_owner_does_not_start_transient_retry() {
    use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::KubernetesStatusIdentity;
    let raw = test_acme();
    let runtime = ControllerRuntime::default();
    runtime.set_status_identity(KubernetesStatusIdentity {
        controller_name: "example.com/controller".into(),
    });
    assert_eq!(
        evaluate_issuance_eligibility(&raw, "default/test-acme", &runtime),
        EligibilityDecision::AuthoritativelyIneligible
    );
}

#[tokio::test]
async fn issuance_eligibility_requires_matching_processed_generation_and_live_proof() {
    let raw = test_acme();
    let mut processed = raw.clone();
    let runtime = eligibility_runtime(&mut processed);
    assert_eq!(
        evaluate_issuance_eligibility(&raw, "default/test", &runtime),
        EligibilityDecision::Eligible
    );

    let mut replaced_raw = raw.clone();
    replaced_raw.metadata.uid = Some("uid-2".to_string());
    assert_eq!(
        evaluate_issuance_eligibility(&replaced_raw, "default/test", &runtime),
        EligibilityDecision::Transient
    );

    let replacement = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        8,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    replacement.save(test_gateway("2"), None);
    runtime.processor_registry.register(replacement);
    assert_eq!(
        evaluate_issuance_eligibility(&raw, "default/test", &runtime),
        EligibilityDecision::Transient,
        "a proof-only Gateway publication must invalidate stale processed authority"
    );

    let missing = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        8,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    runtime.processor_registry.register(missing);
    assert_eq!(
        evaluate_issuance_eligibility(&raw, "default/test", &runtime),
        EligibilityDecision::Transient,
        "a projected-Gateway publication gap must retain a convergence check"
    );

    let mut no_http_gateway = test_gateway("3");
    let listener = no_http_gateway
        .spec
        .listeners
        .as_mut()
        .and_then(|listeners| listeners.first_mut())
        .expect("test Gateway listener must exist");
    listener.protocol = "HTTPS".to_string();
    listener.port = 443;
    let no_http = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        8,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    no_http.save(no_http_gateway, None);
    runtime.processor_registry.register(no_http);
    assert_eq!(
        evaluate_issuance_eligibility(&raw, "default/test", &runtime),
        EligibilityDecision::AuthoritativelyIneligible,
        "a live non-HTTP selection is an authoritative invalid result"
    );
}

#[tokio::test]
async fn issuance_eligibility_rejects_current_false_condition() {
    let raw = test_acme();
    let mut processed = raw.clone();
    let runtime = eligibility_runtime(&mut processed);
    let status = processed.spec.current_status.get_or_insert_with(Default::default);
    status
        .conditions
        .retain(|condition| condition.type_ != condition_types::RESOLVED_REFS);
    status.conditions.push(condition_false(
        condition_types::RESOLVED_REFS,
        condition_reasons::NO_MATCHING_PARENT,
        "No HTTP listener",
        processed.metadata.generation,
    ));
    let replacement = Arc::new(ResourceProcessor::<EdgionAcme>::new(
        "EdgionAcme",
        8,
        Arc::new(NoopAcmeHandler),
        runtime.clone(),
    ));
    replacement.save(processed, None);
    runtime.processor_registry.register(replacement);
    assert_eq!(
        evaluate_issuance_eligibility(&raw, "default/test", &runtime),
        EligibilityDecision::AuthoritativelyIneligible
    );
}

// ================================================================
// parse_resource_key
// ================================================================

#[test]
fn test_parse_resource_key_normal() {
    let (ns, name) = parse_resource_key("default/my-acme");
    assert_eq!(ns, "default");
    assert_eq!(name, "my-acme");
}

#[test]
fn test_parse_resource_key_with_slash_in_name() {
    // splitn(2, '/') means only split on the first '/'
    let (ns, name) = parse_resource_key("ns/name/with/slashes");
    assert_eq!(ns, "ns");
    assert_eq!(name, "name/with/slashes");
}

#[test]
fn test_parse_resource_key_no_namespace() {
    let (ns, name) = parse_resource_key("just-a-name");
    assert_eq!(ns, "default");
    assert_eq!(name, "just-a-name");
}

// ================================================================
// parse_cert_info
// ================================================================

// ================================================================
// timer generation isolation
// ================================================================

/// A timer scheduled by one ACME service generation must not enqueue into a
/// later generation after a stop/restart. All assertions live in a single
/// test because they mutate the `ACME_SERVICE` global; splitting them would
/// let the cases race against each other.
#[tokio::test]
async fn test_service_generation_sender_isolation() {
    // Serialize against any other test that touches the global `ACME_SERVICE`
    // (e.g. the handler tests whose `after_publish` reaches `notify_resource_changed`
    // -> `get_sender()`), so a concurrent notify cannot clone the handle we
    // install below and enqueue a stray message into our receiver.
    let _guard = ACME_TEST_MUTEX.lock().await;

    // --- Generation 1 starts ---
    let (tx1, _rx1) = mpsc::channel::<AcmeCommand>(8);
    let gen1 = ACME_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    *ACME_SERVICE.lock() = Some(AcmeServiceHandle {
        tx: tx1,
        generation: gen1,
        cancel: CancellationToken::new(),
    });

    // A timer stamped with gen1 can enqueue while gen1 is running.
    assert!(get_sender_for_generation(gen1).is_some());
    assert_eq!(current_generation(), gen1);

    // --- Service stops (drops the handle) ---
    ACME_SERVICE.lock().take();
    assert!(get_sender_for_generation(gen1).is_none());
    assert_eq!(current_generation(), 0);

    // --- Generation 2 starts (re-election) ---
    let (tx2, mut rx2) = mpsc::channel::<AcmeCommand>(8);
    let gen2 = ACME_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    assert!(gen2 > gen1, "generations must be strictly increasing");
    *ACME_SERVICE.lock() = Some(AcmeServiceHandle {
        tx: tx2,
        generation: gen2,
        cancel: CancellationToken::new(),
    });

    // A stale gen1 timer must NOT see gen2's sender.
    assert!(get_sender_for_generation(gen1).is_none());
    // The current generation's timer still works.
    assert!(get_sender_for_generation(gen2).is_some());

    assert!(get_sender_for_generation(gen1).is_none());
    get_sender_for_generation(gen2)
        .unwrap()
        .send(AcmeCommand::StartupCheck {
            key: "default/live".to_string(),
        })
        .await
        .unwrap();
    let live = rx2.recv().await;
    assert!(matches!(live, Some(AcmeCommand::StartupCheck { key }) if key == "default/live"));

    // Cleanup global state.
    ACME_SERVICE.lock().take();
}

#[test]
fn scheduler_keeps_one_earliest_deadline_and_admin_pending() {
    let mut state = ResourceSchedule::default();
    state.set_deadline(Duration::from_secs(30), TriggerKind::Renewal);
    let first = state.deadline.unwrap().0;
    state.set_deadline(Duration::from_secs(60), TriggerKind::Retry);
    assert_eq!(state.deadline.unwrap().0, first);
    state.set_deadline(Duration::from_secs(1), TriggerKind::Eligibility);
    assert!(state.deadline.unwrap().0 < first);

    state.merge_pending(TriggerKind::Resource);
    state.merge_pending(TriggerKind::Admin);
    state.merge_pending(TriggerKind::Startup);
    assert_eq!(state.pending, Some(TriggerKind::Admin));
}

#[test]
fn admin_recheck_priority_survives_ordinary_notifications() {
    let mut waiting = ResourceSchedule::default();
    waiting.set_deadline(CONTROL_PLANE_RECHECK_DELAY, TriggerKind::Admin);
    assert_eq!(claim_trigger(&mut waiting, TriggerKind::Resource), TriggerKind::Admin);
    assert!(waiting.deadline.is_none());

    let mut completing = ResourceSchedule {
        busy: true,
        pending: Some(TriggerKind::Resource),
        ..Default::default()
    };
    completing.set_deadline(CONTROL_PLANE_RECHECK_DELAY, TriggerKind::Admin);
    completing.busy = false;
    let pending = completing.pending.take().unwrap();
    assert_eq!(claim_trigger(&mut completing, pending), TriggerKind::Admin);
}

#[test]
fn delete_completion_preserves_a_concurrent_recreate_notification() {
    let mut schedules = HashMap::from([(
        "default/test".to_string(),
        ResourceSchedule {
            busy: true,
            pending: Some(TriggerKind::Resource),
            ..Default::default()
        },
    )]);
    assert_eq!(
        take_deleted_pending(&mut schedules, "default/test"),
        Some(TriggerKind::Resource)
    );
    assert!(!schedules.contains_key("default/test"));
}

#[test]
fn persisted_retry_budget_resets_only_for_admin_or_new_generation() {
    let checkpoint = AcmeSchedulerStatus {
        observed_generation: 7,
        attempts_used: 4,
        ..Default::default()
    };
    assert_eq!(persisted_attempts(TriggerKind::Resource, 7, Some(&checkpoint)), 4);
    assert_eq!(persisted_attempts(TriggerKind::Startup, 7, Some(&checkpoint)), 4);
    assert_eq!(persisted_attempts(TriggerKind::Admin, 7, Some(&checkpoint)), 0);
    assert_eq!(persisted_attempts(TriggerKind::Resource, 8, Some(&checkpoint)), 0);
}

#[test]
fn test_parse_cert_info_invalid_pem() {
    let info = parse_cert_info("not a pem");
    assert!(info.serial.is_none());
    assert!(info.not_after.is_none());
}

#[test]
fn test_parse_cert_info_valid_self_signed() {
    // Generate a self-signed cert for testing
    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let params = rcgen::CertificateParams::new(vec!["test.example.com".to_string()]).unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let pem = cert.pem();

    let info = parse_cert_info(&pem);
    assert!(info.serial.is_some(), "Should have serial");
    assert!(info.not_after.is_some(), "Should have not_after");

    // Verify not_after is valid RFC 3339 (parseable by chrono)
    let not_after_str = info.not_after.unwrap();
    let parsed = chrono::DateTime::parse_from_rfc3339(&not_after_str);
    assert!(
        parsed.is_ok(),
        "not_after should be valid RFC 3339, got: {}",
        not_after_str
    );
}
