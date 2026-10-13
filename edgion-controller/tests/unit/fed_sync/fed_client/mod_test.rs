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
        tls: Some(ConfSyncTlsConfig {
            name: None,
            cert: "c".into(),
            key: "k".into(),
            ca: "a".into(),
            skip_tls: true,
        }),
        ..Default::default()
    };
    assert_eq!(skip.resolve_mtls_or_refuse().unwrap_err(), MtlsRefusal::SkipTls);
    assert_eq!(refusal_reason(MtlsRefusal::SkipTls), reasons::SKIP_TLS);

    // Real mTLS -> Ok (gate passes; client proceeds to self-check)
    let mtls = ConfSyncSecurityConfig {
        tls: Some(ConfSyncTlsConfig {
            name: None,
            cert: "c".into(),
            key: "k".into(),
            ca: "a".into(),
            skip_tls: false,
        }),
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
    let mut active_watch_cancel: Option<tokio_util::sync::CancellationToken> = None;

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
    let msg = rx
        .try_recv()
        .expect("expected exactly one ControllerMessage on the channel");
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

    // No watch task must have been started (active_watch_cancel stays None).
    assert!(
        active_watch_cancel.is_none(),
        "expected active_watch_cancel to remain None after RBAC deny"
    );
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
    let mut active_watch_cancel: Option<tokio_util::sync::CancellationToken> = None;

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
    let mut active_watch_cancel: Option<tokio_util::sync::CancellationToken> = None;

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
        path: "/api/v1/reload".to_string(),
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
        path: "/api/v1/reload".to_string(),
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

/// Reload now arrives as an ordinary proxied POST. The tunnel must let
/// `POST /api/v1/reload` through the path allowlist, reach a registered
/// handler, and carry that handler's body back.
#[tokio::test]
async fn http_proxy_reload_post_reaches_router() {
    let resp = run_http_proxy_with_router(
        crate::fed_sync::proto::HttpProxyRequest {
            request_id: "t4".to_string(),
            method: "POST".to_string(),
            path: "/api/v1/reload".to_string(),
            headers: Default::default(),
            body: Vec::new(),
        },
        axum::Router::new().route("/api/v1/reload", axum::routing::post(|| async { "reload-ok" })),
    )
    .await;

    assert_eq!(resp.status_code, 200);
    assert_eq!(String::from_utf8(resp.body).expect("utf8"), "reload-ok");
}

/// ARCH-082: successful forwarding must not create spelling-dependent metric series.
#[tokio::test(flavor = "current_thread")]
async fn successful_watch_metrics_use_one_canonical_kind() {
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, ShutdownHandle};
    use crate::conf_sync::conf_server::{WatchObj, WatchResponseSimple};
    use crate::fed_sync::proto::FedWatchRequest;
    use edgion_common::observe::cardinality_guard::{CardinalityGuard, CardinalityMode};
    use tokio::sync::mpsc;

    struct ReadyWatch;
    impl WatchObj for ReadyWatch {
        fn kind_name(&self) -> &'static str {
            "HTTPRoute"
        }
        fn list_json(&self) -> Result<(String, u64), String> {
            Ok(("[]".into(), 2))
        }
        fn current_version(&self) -> u64 {
            2
        }
        fn watch_json(&self, _: String, _: String, from_version: u64) -> mpsc::Receiver<WatchResponseSimple> {
            assert_eq!(from_version, 1);
            let (tx, rx) = mpsc::channel(1);
            tx.try_send(WatchResponseSimple::new("fixture-event".into(), 2))
                .unwrap();
            rx
        }
        fn is_ready(&self) -> bool {
            true
        }
        fn set_ready(&self) {}
        fn set_not_ready(&self) {}
        fn clear(&self) {}
    }

    let tmp = tempfile::tempdir().unwrap();
    let manager = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(
            FileSystemConfig::new(tmp.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller"),
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
    let server = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(server) = manager.config_sync_server() {
                break server;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    server.register_all(std::collections::HashMap::from([(
        "HTTPRoute".into(),
        Arc::new(ReadyWatch) as Arc<dyn WatchObj>,
    )]));

    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let recorder = CardinalityGuard::new(recorder, 64, CardinalityMode::Enforce);
    // This runtime and its forwarding tasks stay on the same thread.
    let _local_recorder = metrics::set_default_local_recorder(&recorder);
    let authorizer = Arc::new(Authorizer::new(crate::fed_sync::default_policy::default_center_policy()));
    let (tx, mut rx) = mpsc::channel(8);
    let tx = Arc::new(tx);
    let mut cancel = None;
    for (index, kind) in ["HTTPRoute", "httproute", "hTtPrOuTe"].into_iter().enumerate() {
        let request_id = format!("watch-{index}");
        handle_center_message(
            CenterPayload::WatchRequest(FedWatchRequest {
                request_id: request_id.clone(),
                kind: kind.into(),
                from_version: 1,
            }),
            tx.clone(),
            axum::Router::new(),
            manager.clone(),
            &mut cancel,
            Arc::new(tokio::sync::Semaphore::new(1)),
            authorizer.clone(),
        )
        .await
        .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let Some(CtrlPayload::WatchEventResponse(event)) = message.payload else {
            panic!("expected successful watch event");
        };
        assert_eq!(event.request_id, request_id);
        assert_eq!(event.data, "fixture-event");
        assert_eq!(event.sync_version, 2);
        assert_eq!(event.server_id, server.server_id());
        assert!(event.error.is_empty());
    }
    let output = handle.render();
    let samples: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with(fed_metrics::names::WATCH_EVENTS_TOTAL))
        .collect();
    assert_eq!(samples.len(), 1, "{output}");
    assert!(samples[0].contains("kind=\"HTTPRoute\""), "{output}");
    assert!(samples[0].contains("direction=\"sent\""), "{output}");
    assert!(samples[0].ends_with(" 3"), "{output}");
    if let Some(cancel) = cancel {
        cancel.cancel();
    }
    shutdown.shutdown();
    tokio::time::timeout(Duration::from_secs(5), lifecycle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
