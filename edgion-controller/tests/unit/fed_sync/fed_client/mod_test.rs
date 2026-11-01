use super::*;

#[test]
fn test_backoff_caps_at_60s() {
    let mut backoff = ReconnectBackoff::new();
    for _ in 0..20 {
        backoff.next();
    }
    assert_eq!(backoff.current_secs(), 60);
}

#[test]
fn test_backoff_resets() {
    let mut backoff = ReconnectBackoff::new();
    backoff.next(); // 2
    backoff.next(); // 4
    backoff.reset();
    assert_eq!(backoff.current_secs(), 1);
}

#[test]
fn stats_snapshot_is_gated_by_readiness_and_server_generation() {
    assert_eq!(
        stats_collection_mode(true, false, None, None),
        StatsCollectionMode::Skip
    );
    assert_eq!(
        stats_collection_mode(true, true, None, Some("server-a")),
        StatsCollectionMode::Snapshot
    );
    assert_eq!(
        stats_collection_mode(false, true, Some("server-a"), Some("server-a")),
        StatsCollectionMode::DirtyOnly
    );
    assert_eq!(
        stats_collection_mode(false, false, Some("server-a"), None),
        StatsCollectionMode::Skip
    );
    assert_eq!(
        stats_collection_mode(false, true, Some("server-a"), Some("server-b")),
        StatsCollectionMode::Snapshot
    );
}

#[test]
fn self_check_accepts_matching_path() {
    use crate::fed_sync::spiffe::{parse_controller_spiffe, path_matches};
    let p = parse_controller_spiffe("spiffe://td/controllers/prod/ctrl-1").unwrap();
    assert!(path_matches(&p, "prod", "ctrl-1"));
    assert!(!path_matches(&p, "prod", "other"));
}

#[test]
fn federation_requires_mtls_channel() {
    use edgion_common::conf_sync::config::{ConfSyncSecurityConfig, ConfSyncTlsConfig, MtlsRefusal};
    use edgion_common::observe::system_metrics::reasons;

    // No TLS block -> NoTls -> reason no_tls_configured
    let none = ConfSyncSecurityConfig::default();
    assert_eq!(none.resolve_mtls_or_refuse().unwrap_err(), MtlsRefusal::NoTls);
    assert_eq!(refusal_reason(MtlsRefusal::NoTls), reasons::NO_TLS_CONFIGURED);

    // skip_tls -> SkipTls -> reason skip_tls
    let skip = ConfSyncSecurityConfig {
        certs: vec![ConfSyncTlsConfig {
            name: None,
            cert: "c".into(),
            key: "k".into(),
            ca: "a".into(),
            skip_tls: true,
        }],
        ..Default::default()
    };
    assert_eq!(skip.resolve_mtls_or_refuse().unwrap_err(), MtlsRefusal::SkipTls);
    assert_eq!(refusal_reason(MtlsRefusal::SkipTls), reasons::SKIP_TLS);

    // Real mTLS -> Ok (gate passes; client proceeds to self-check)
    let mtls = ConfSyncSecurityConfig {
        certs: vec![ConfSyncTlsConfig {
            name: None,
            cert: "c".into(),
            key: "k".into(),
            ca: "a".into(),
            skip_tls: false,
        }],
        ..Default::default()
    };
    assert!(mtls.resolve_mtls_or_refuse().is_ok());
}

/// WatchRequest-deny path: a deny-all authorizer must cause handle_center_message
/// to immediately send a WatchEventResponse with a non-empty error field and return
/// without starting any watch (active_watch_cancel remains None).
#[tokio::test]
async fn watch_request_denied_by_rbac_sends_forbidden_and_no_watch_started() {
    use crate::authz::{Authorizer, CenterPolicy};
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig};
    use crate::fed_sync::proto::{controller_message::Payload as CtrlPayload, FedWatchRequest};
    use std::sync::Arc;

    // Construct a minimal ConfMgr using the FileSystem backend (temp dir).
    // The deny path returns before conf_mgr is used, but the function signature requires it.
    let tmp = tempfile::tempdir().expect("tempdir");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(
            FileSystemConfig::new(tmp.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller"),
        ))
        .await
        .expect("ConfMgr::create failed"),
    );

    let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ControllerMessage>(8);
    let tx = Arc::new(tx);
    let admin_router = axum::Router::new();
    let proxy_semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let mut active_watch_cancel = WatchSession::default();

    let payload = CenterPayload::WatchRequest(FedWatchRequest {
        request_id: "t1".to_string(),
        kind: "EdgionConfigData".to_string(),
        from_version: 0,
    });

    let result = handle_center_message(
        payload,
        tx,
        admin_router,
        conf_mgr,
        &mut active_watch_cancel,
        proxy_semaphore,
        authorizer,
    )
    .await;

    // The call must succeed (no internal error).
    assert!(result.is_ok(), "handle_center_message returned Err: {:?}", result.err());

    // Exactly one message must have been sent: a WatchEventResponse with non-empty error.
    let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    match msg.payload {
        Some(CtrlPayload::WatchEventResponse(resp)) => {
            assert!(
                !resp.error.is_empty(),
                "expected non-empty error in WatchEventResponse, got empty"
            );
        }
        other => panic!("expected WatchEventResponse, got {:?}", other),
    }

    // No second message must be present.
    assert!(
        rx.try_recv().is_err(),
        "expected channel to be empty after one deny message"
    );

    drop(active_watch_cancel);
}

/// The federation proxy must forward `If-Match` (CAS precondition for
/// Center-issued writes; DELETE has no body fallback) and must keep
/// stripping credential headers such as `Authorization`.
#[tokio::test]
async fn http_proxy_forwards_if_match_and_strips_authorization() {
    use crate::authz::{Authorizer, CenterPolicy};
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig};
    use crate::fed_sync::proto::{controller_message::Payload as CtrlPayload, HttpProxyRequest};
    use std::collections::HashMap;
    use std::sync::Arc;

    let tmp = tempfile::tempdir().expect("tempdir");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(
            FileSystemConfig::new(tmp.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller"),
        ))
        .await
        .expect("ConfMgr::create failed"),
    );
    let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ControllerMessage>(8);
    let tx = Arc::new(tx);
    let admin_router = axum::Router::new().route(
        "/api/v1/echo-headers",
        axum::routing::get(|headers: axum::http::HeaderMap| async move {
            let if_match = headers
                .get("if-match")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let has_authorization = headers.contains_key("authorization");
            format!("if-match={if_match};authorization-present={has_authorization}")
        }),
    );
    let proxy_semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let mut active_watch_cancel = WatchSession::default();

    let payload = CenterPayload::HttpProxy(HttpProxyRequest {
        request_id: "p1".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/echo-headers".to_string(),
        headers: HashMap::from([
            ("if-match".to_string(), "\"v123\"".to_string()),
            ("authorization".to_string(), "Bearer secret".to_string()),
        ]),
        body: Vec::new(),
    });

    handle_center_message(
        payload,
        tx,
        admin_router,
        conf_mgr,
        &mut active_watch_cancel,
        proxy_semaphore,
        authorizer,
    )
    .await
    .expect("handle_center_message failed");

    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for HttpProxyResponse")
        .expect("channel closed without a response");
    let Some(CtrlPayload::HttpProxyResponse(resp)) = msg.payload else {
        panic!("expected HttpProxyResponse, got {:?}", msg.payload);
    };
    assert_eq!(resp.status_code, 200);
    let body = String::from_utf8(resp.body).expect("utf8 body");
    assert_eq!(body, "if-match=\"v123\";authorization-present=false");
}

/// Drive a single HttpProxy request through handle_center_message against
/// `admin_router` and return the whole HttpProxyResponse the handler emits.
#[cfg(test)]
async fn run_http_proxy_with_router(
    req: crate::fed_sync::proto::HttpProxyRequest,
    admin_router: axum::Router,
) -> crate::fed_sync::proto::HttpProxyResponse {
    use crate::authz::{Authorizer, CenterPolicy};
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig};
    use std::sync::Arc;

    let tmp = tempfile::tempdir().expect("tempdir");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(
            FileSystemConfig::new(tmp.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller"),
        ))
        .await
        .expect("ConfMgr::create failed"),
    );
    let authorizer = Arc::new(Authorizer::new(CenterPolicy::deny_all()));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ControllerMessage>(8);
    let tx = Arc::new(tx);
    let proxy_semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let mut active_watch_cancel = WatchSession::default();

    handle_center_message(
        CenterPayload::HttpProxy(req),
        tx,
        admin_router,
        conf_mgr,
        &mut active_watch_cancel,
        proxy_semaphore,
        authorizer,
    )
    .await
    .expect("handle_center_message returned Err");

    // The HttpProxy arm spawns a task; await the response it sends.
    let msg = rx.recv().await.expect("expected an HttpProxyResponse");
    match msg.payload {
        Some(CtrlPayload::HttpProxyResponse(resp)) => resp,
        other => panic!("expected HttpProxyResponse, got {:?}", other),
    }
}

/// Status code for a request driven against an empty admin router: the
/// sanitisation checks built on this all reject before the request ever
/// reaches a route.
#[cfg(test)]
async fn run_http_proxy(req: crate::fed_sync::proto::HttpProxyRequest) -> u32 {
    run_http_proxy_with_router(req, axum::Router::new()).await.status_code
}

/// An oversized request body is rejected with 413 before any router work.
#[tokio::test]
async fn http_proxy_oversized_body_rejected_413() {
    let status = run_http_proxy(crate::fed_sync::proto::HttpProxyRequest {
        request_id: "t1".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/services/conf-sync-ca/rotate".to_string(),
        headers: Default::default(),
        body: vec![0u8; MAX_PROXY_REQUEST_BODY + 1],
    })
    .await;
    assert_eq!(status, 413);
}

/// A malformed method is rejected with 400 instead of silently becoming GET.
#[tokio::test]
async fn http_proxy_invalid_method_rejected_400() {
    let status = run_http_proxy(crate::fed_sync::proto::HttpProxyRequest {
        request_id: "t2".to_string(),
        method: "BAD METHOD".to_string(),
        path: "/api/v1/services/conf-sync-ca/rotate".to_string(),
        headers: Default::default(),
        body: Vec::new(),
    })
    .await;
    assert_eq!(status, 400);
}

/// A path outside the allowlist is rejected with 404 before reaching the router.
#[tokio::test]
async fn http_proxy_disallowed_path_rejected_404() {
    let status = run_http_proxy(crate::fed_sync::proto::HttpProxyRequest {
        request_id: "t3".to_string(),
        method: "GET".to_string(),
        path: "/secret/etc/passwd".to_string(),
        headers: Default::default(),
        body: Vec::new(),
    })
    .await;
    assert_eq!(status, 404);
}

/// Certificate rotation arrives as an ordinary proxied POST. The tunnel must let
/// `POST /api/v1/services/conf-sync-ca/rotate` through the path allowlist, reach a registered
/// handler, and carry that handler's body back.
#[tokio::test]
async fn http_proxy_rotation_post_reaches_router() {
    let resp = run_http_proxy_with_router(
        crate::fed_sync::proto::HttpProxyRequest {
            request_id: "t4".to_string(),
            method: "POST".to_string(),
            path: "/api/v1/services/conf-sync-ca/rotate".to_string(),
            headers: Default::default(),
            body: Vec::new(),
        },
        axum::Router::new().route(
            "/api/v1/services/conf-sync-ca/rotate",
            axum::routing::post(|| async { "rotate-ok" }),
        ),
    )
    .await;

    assert_eq!(resp.status_code, 200);
    assert_eq!(String::from_utf8(resp.body).expect("utf8"), "rotate-ok");
}

/// ARCH-082: successful forwarding must not create spelling-dependent metric series.
#[tokio::test(flavor = "current_thread")]
async fn successful_watch_metrics_use_one_canonical_kind() {
    use edgion_common::observe::cardinality_guard::{CardinalityGuard, CardinalityMode};
    let fixture = SessionFixture::new().await;
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let recorder = CardinalityGuard::new(recorder, 64, CardinalityMode::Enforce);
    let _local_recorder = metrics::set_default_local_recorder(&recorder);
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let mut watch = WatchSession::default();
    watch.start(
        crate::fed_sync::proto::FedWatchRequest {
            request_id: "metrics".into(),
            kind: "RegionRoute".into(),
            from_version: 0,
        },
        Arc::new(tx),
        fixture.manager.clone(),
        Arc::new(Authorizer::new(crate::fed_sync::default_policy::default_center_policy())),
    );
    assert!(matches!(
        rx.recv().await.unwrap().payload,
        Some(CtrlPayload::WatchListResponse(_))
    ));
    publish_setting(&fixture.manager, "metric-setting", "1");
    let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(msg.payload, Some(CtrlPayload::WatchEventResponse(event)) if event.error.is_empty()));
    let output = handle.render();
    let samples: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with(fed_metrics::names::WATCH_EVENTS_TOTAL))
        .collect();
    assert_eq!(samples.len(), 1, "{output}");
    assert!(samples[0].contains("kind=\"RegionRoute\""), "{output}");
    assert!(samples[0].contains("direction=\"sent\""), "{output}");
    drop(watch);
    fixture.finish().await;
}

// ARCH-084: exercise the production session and shutdown boundary without a network server.
pub(super) struct SessionFixture {
    _directory: tempfile::TempDir,
    pub(super) manager: Arc<ConfMgr>,
    shutdown: crate::conf_mgr::ShutdownHandle,
    lifecycle: tokio::task::JoinHandle<Result<()>>,
}
impl SessionFixture {
    pub(super) async fn new() -> Self {
        use crate::conf_mgr::{ConfCenterConfig, FileSystemConfig, ShutdownHandle};
        let directory = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            ConfMgr::create(ConfCenterConfig::FileSystem(
                FileSystemConfig::new(directory.path().to_path_buf())
                    .with_controller_name("edgion.io/gateway-controller"),
            ))
            .await
            .unwrap(),
        );
        let shutdown = ShutdownHandle::new();
        let lifecycle = {
            let manager = manager.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(async move { manager.start_with_shutdown(shutdown).await })
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while !manager.is_ready() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("manager ready");
        Self {
            _directory: directory,
            manager,
            shutdown,
            lifecycle,
        }
    }
    pub(super) fn request_shutdown(&self) {
        self.shutdown.shutdown();
    }

    pub(super) async fn finish(self) {
        self.shutdown.shutdown();
        tokio::time::timeout(Duration::from_secs(5), self.lifecycle)
            .await
            .expect("manager stopped")
            .unwrap()
            .unwrap();
    }
}
pub(super) fn publish_setting(manager: &ConfMgr, name: &str, rv: &str) {
    use crate::conf_mgr::conf_center::traits::CenterLifeCycle;
    let source = serde_json::from_value(serde_json::json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionConfigData",
        "metadata":{"namespace":"apps","name":name,"resourceVersion":rv},
        "spec":{"data":{"type":"ClusterRegionSetting","config":{"clusterName":"fleet","regionName":"east","regions":[]}}}
    })).unwrap();
    let runtime = manager.runtime();
    runtime
        .center_watch
        .upsert(
            &runtime.center_watch.generation(),
            crate::fed_sync::center_watch::projection::setting(&source)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
}

type SessionFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Option<Result<()>>> + Send>>;

struct TestSession {
    future: SessionFuture,
    inbound: tokio::sync::mpsc::Sender<std::result::Result<CenterMessage, tonic::Status>>,
    outbound: Arc<tokio::sync::mpsc::Sender<ControllerMessage>>,
    receiver: tokio::sync::mpsc::Receiver<ControllerMessage>,
    shutdown: watch::Sender<bool>,
}

impl TestSession {
    fn new(manager: Arc<ConfMgr>, full: bool) -> Self {
        let (inbound, rx) = tokio::sync::mpsc::channel(8);
        let (outbound, receiver) = tokio::sync::mpsc::channel(1);
        if full {
            outbound.try_send(ControllerMessage { payload: None }).unwrap();
        }
        let outbound = Arc::new(outbound);
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let tx = outbound.clone();
        let future = Box::pin(async move {
            until_shutdown(
                &mut shutdown_rx,
                run_session(
                    tokio_stream::wrappers::ReceiverStream::new(rx),
                    tx,
                    axum::Router::new(),
                    manager,
                    Arc::new(Authorizer::new(crate::fed_sync::default_policy::default_center_policy())),
                    std::time::Instant::now(),
                ),
            )
            .await
        });
        Self {
            future,
            inbound,
            outbound,
            receiver,
            shutdown,
        }
    }

    async fn poll_pending(&mut self) {
        assert!(futures::poll!(&mut self.future).is_pending());
    }

    async fn watch(&mut self, id: &str) {
        self.inbound
            .send(Ok(CenterMessage {
                payload: Some(CenterPayload::WatchRequest(crate::fed_sync::proto::FedWatchRequest {
                    request_id: id.into(),
                    kind: "RegionRoute".into(),
                    from_version: 0,
                })),
            }))
            .await
            .unwrap();
        // Wait for this request to leave the inbound channel while the outbound
        // slot remains occupied. A single poll can exhaust Tokio's cooperative
        // budget before run_session consumes the replacement request.
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.inbound.capacity() != 8 {
                self.poll_pending().await;
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("watch request consumed by the session");
    }

    async fn take_list(&mut self, id: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(CtrlPayload::WatchListResponse(list)) = self.receiver.recv().await.unwrap().payload {
                    assert_eq!(list.request_id, id);
                    break;
                }
            }
        })
        .await
        .expect("dedicated List");
    }

    async fn stop_without_waiting(&mut self) {
        self.shutdown.send(true).unwrap();
        assert!(
            matches!(futures::poll!(&mut self.future), std::task::Poll::Ready(None)),
            "shutdown must complete in one poll without scheduling or joining descendants"
        );
    }

    async fn descendants_released(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&self.outbound) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("session descendants must release outbound sender");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_releases_quiet_watch_without_waiting_for_descendants() {
    let fixture = SessionFixture::new().await;
    let mut session = TestSession::new(fixture.manager.clone(), false);
    session.watch("quiet").await;
    session.take_list("quiet").await;
    session.stop_without_waiting().await;
    session.descendants_released().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_cancels_blocked_stats_send_without_draining() {
    let fixture = SessionFixture::new().await;
    tokio::time::pause();
    let mut session = TestSession::new(fixture.manager.clone(), true);
    session.poll_pending().await;
    // Register the initial interval, then advance its timer into the full-channel send.
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(1)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    tokio::time::resume();
    session.stop_without_waiting().await;
    session.descendants_released().await;
    assert!(session.receiver.try_recv().unwrap().payload.is_none());
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_cancels_blocked_watch_send_without_draining() {
    let fixture = SessionFixture::new().await;
    let mut session = TestSession::new(fixture.manager.clone(), false);
    session.watch("blocked").await;
    session.take_list("blocked").await;
    session.outbound.try_send(ControllerMessage { payload: None }).unwrap();
    publish_setting(&fixture.manager, "blocked", "1");
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    session.stop_without_waiting().await;
    session.descendants_released().await;
    assert!(session.receiver.try_recv().unwrap().payload.is_none());
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_cancels_blocked_generation_change_send_without_draining() {
    let mut fixture = SessionFixture::new().await;
    let mut session = TestSession::new(fixture.manager.clone(), false);
    session.watch("generation-change").await;
    session.take_list("generation-change").await;
    session.outbound.try_send(ControllerMessage { payload: None }).unwrap();
    let old_id = fixture.manager.config_sync_server().unwrap().server_id();
    // Exercise a fresh lifecycle after orderly shutdown, not an administrative reload.
    fixture.shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(5), &mut fixture.lifecycle)
        .await
        .expect("old lifecycle drained")
        .unwrap()
        .unwrap();
    fixture.shutdown = crate::conf_mgr::ShutdownHandle::new();
    let manager = fixture.manager.clone();
    let shutdown = fixture.shutdown.clone();
    fixture.lifecycle = tokio::spawn(async move { manager.start_with_shutdown(shutdown).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if fixture.manager.is_ready()
                && fixture
                    .manager
                    .config_sync_server()
                    .is_some_and(|css| css.server_id() != old_id)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replacement lifecycle completes");
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    session.stop_without_waiting().await;
    tokio::time::resume();
    session.descendants_released().await;
    assert!(session.receiver.try_recv().unwrap().payload.is_none());
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn watch_backpressure_and_replacement_remain_session_local() {
    let fixture = SessionFixture::new().await;
    let mut session = TestSession::new(fixture.manager.clone(), false);
    session.watch("old").await;
    session.take_list("old").await;
    session.outbound.try_send(ControllerMessage { payload: None }).unwrap();
    publish_setting(&fixture.manager, "replaced", "1");
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    session.watch("new").await;
    assert!(session.receiver.try_recv().unwrap().payload.is_none());
    let msg = tokio::time::timeout(Duration::from_secs(2), session.receiver.recv())
        .await
        .unwrap()
        .unwrap();
    let Some(CtrlPayload::WatchListResponse(list)) = msg.payload else {
        panic!("replacement List, never old delta");
    };
    assert_eq!(list.request_id, "new");
    assert!(String::from_utf8(list.data.to_vec()).unwrap().contains("replaced"));
    session.stop_without_waiting().await;
    session.descendants_released().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "current_thread")]
async fn session_eof_and_error_cancel_quiet_watch() {
    for error in [false, true] {
        let fixture = SessionFixture::new().await;
        let mut session = TestSession::new(fixture.manager.clone(), false);
        session.watch("terminal").await;
        session.take_list("terminal").await;
        if error {
            session
                .inbound
                .send(Err(tonic::Status::unavailable("fixture disconnect")))
                .await
                .unwrap();
        } else {
            // Closing the last sender supplies a real stream EOF.
            let (replacement, _) = tokio::sync::mpsc::channel(1);
            drop(std::mem::replace(&mut session.inbound, replacement));
        }
        let result = (&mut session.future).await.expect("not shutdown");
        assert_eq!(result.is_err(), error);
        session.descendants_released().await;
        fixture.finish().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn watch_backpressure_ends_session_even_when_ping_reply_is_blocked() {
    let fixture = SessionFixture::new().await;
    let mut session = TestSession::new(fixture.manager.clone(), true);
    session.watch("blocked-list").await;
    session
        .inbound
        .send(Ok(CenterMessage {
            payload: Some(CenterPayload::Ping(crate::fed_sync::proto::Ping { timestamp: 1 })),
        }))
        .await
        .unwrap();
    session.poll_pending().await;
    tokio::time::pause();
    let result = tokio::time::timeout(Duration::from_secs(6), &mut session.future)
        .await
        .expect("watch delivery deadline must interrupt blocked Ping")
        .expect("session exits without external shutdown");
    assert!(result.is_err());
    tokio::time::resume();
    session.descendants_released().await;
    assert!(session.receiver.try_recv().unwrap().payload.is_none());
    fixture.finish().await;
}
