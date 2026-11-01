use super::super::tests::{publish_setting, SessionFixture};
use super::*;
use crate::authz::CenterPolicy;

fn request(id: &str) -> FedWatchRequest {
    FedWatchRequest {
        request_id: id.into(),
        kind: VIEW.into(),
        from_version: 0,
    }
}
fn authorizer() -> Arc<Authorizer> {
    Arc::new(Authorizer::new(crate::fed_sync::default_policy::default_center_policy()))
}
async fn receive(rx: &mut mpsc::Receiver<ControllerMessage>) -> CtrlPayload {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("watch frame")
        .unwrap()
        .payload
        .unwrap()
}
fn error(payload: CtrlPayload) -> FedWatchEventResponse {
    let CtrlPayload::WatchEventResponse(error) = payload else {
        panic!("expected terminal event");
    };
    assert!(!error.error.is_empty());
    assert!(error.data.is_empty());
    error
}

#[tokio::test]
async fn dedicated_list_then_update_delete_and_generation_change() {
    let fixture = SessionFixture::new().await;
    publish_setting(&fixture.manager, "one", "1");
    let (tx, mut rx) = mpsc::channel(4);
    let mut session = WatchSession::default();
    session.start(request("live"), Arc::new(tx), fixture.manager.clone(), authorizer());
    let CtrlPayload::WatchListResponse(list) = receive(&mut rx).await else {
        panic!("list");
    };
    assert_eq!(list.sync_version, 1);
    let list_json: serde_json::Value = serde_json::from_slice(&list.data).unwrap();
    assert_eq!(list_json["records"][0]["data"]["identity"]["key"]["name"], "one");
    publish_setting(&fixture.manager, "one", "2");
    let CtrlPayload::WatchEventResponse(update) = receive(&mut rx).await else {
        panic!("event");
    };
    assert_eq!(update.server_id, list.server_id);
    assert_eq!(update.sync_version, 2);
    let changes: serde_json::Value = serde_json::from_str(&update.data).unwrap();
    assert_eq!(
        changes["changes"][0]["record"]["data"]["identity"]["resourceVersion"],
        "2"
    );
    let runtime = fixture.manager.runtime();
    runtime
        .center_watch
        .delete(
            &list.server_id,
            &SourceKey::new(
                crate::fed_sync::center_watch::projection::SourceKind::EdgionConfigData,
                "apps",
                "one",
            )
            .unwrap(),
        )
        .unwrap();
    let CtrlPayload::WatchEventResponse(deleted) = receive(&mut rx).await else {
        panic!("delete");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&deleted.data).unwrap()["changes"][0]["record"],
        serde_json::Value::Null
    );
    runtime.center_watch.reset();
    assert_eq!(error(receive(&mut rx).await).error, "RelistRequired");
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
async fn generic_kinds_are_denied_even_with_wildcard_policy_and_faulted_store() {
    use crate::config::{FedAllowRule, FedRbacConfig};
    let fixture = SessionFixture::new().await;
    let runtime = fixture.manager.runtime();
    let _ = runtime
        .center_watch
        .fail(&runtime.center_watch.generation(), StoreError::Capacity);
    let auth = Arc::new(Authorizer::new(CenterPolicy::from_config(&FedRbacConfig {
        allow: vec![FedAllowRule {
            verbs: vec!["*".into()],
            kinds: vec!["*".into()],
        }],
    })));
    let (tx, mut rx) = mpsc::channel(1);
    let tx = Arc::new(tx);
    let mut session = WatchSession::default();
    for kind in ["EdgionTls", "EdgionConfigData", "Secret", "*", "regionroute"] {
        let mut req = request("denied");
        req.kind = kind.into();
        session.start(req, tx.clone(), fixture.manager.clone(), auth.clone());
        assert_eq!(error(receive(&mut rx).await).error, "Forbidden");
    }
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
async fn invalid_request_id_is_not_echoed_and_resume_requires_full_list() {
    let fixture = SessionFixture::new().await;
    let (tx, mut rx) = mpsc::channel(1);
    let tx = Arc::new(tx);
    let mut session = WatchSession::default();
    session.start(
        request(&"x".repeat(129)),
        tx.clone(),
        fixture.manager.clone(),
        authorizer(),
    );
    let invalid = error(receive(&mut rx).await);
    assert_eq!(invalid.error, "InvalidRequest");
    assert!(invalid.request_id.is_empty());
    let mut req = request("resume");
    req.from_version = 1;
    session.start(req, tx, fixture.manager.clone(), authorizer());
    assert_eq!(error(receive(&mut rx).await).error, "RelistRequired");
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
async fn idle_revocation_ends_watch_without_a_source_event() {
    let fixture = SessionFixture::new().await;
    let auth = authorizer();
    let (tx, mut rx) = mpsc::channel(1);
    let mut session = WatchSession::default();
    session.start(request("revoke"), Arc::new(tx), fixture.manager.clone(), auth.clone());
    assert!(matches!(receive(&mut rx).await, CtrlPayload::WatchListResponse(_)));
    auth.swap_policy(CenterPolicy::deny_all());
    assert_eq!(error(receive(&mut rx).await).error, "Forbidden");
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
async fn waiting_for_capacity_rechecks_permission_and_generation() {
    for revoke in [true, false] {
        let fixture = SessionFixture::new().await;
        let auth = authorizer();
        let (tx, mut rx) = mpsc::channel(1);
        let tx = Arc::new(tx);
        let mut session = WatchSession::default();
        session.start(request("pending"), tx.clone(), fixture.manager.clone(), auth.clone());
        assert!(matches!(receive(&mut rx).await, CtrlPayload::WatchListResponse(_)));
        tx.send(ControllerMessage { payload: None }).await.unwrap();
        publish_setting(&fixture.manager, "pending", "1");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        if revoke {
            auth.swap_policy(CenterPolicy::deny_all());
        } else {
            fixture.manager.runtime().center_watch.reset();
        }
        assert!(rx.recv().await.unwrap().payload.is_none());
        assert_eq!(
            error(receive(&mut rx).await).error,
            if revoke { "Forbidden" } else { "RelistRequired" }
        );
        drop(session);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn error_delivery_backpressure_cancels_session() {
    let fixture = SessionFixture::new().await;
    let (tx, _rx) = mpsc::channel(1);
    let tx = Arc::new(tx);
    tx.send(ControllerMessage { payload: None }).await.unwrap();
    let mut session = WatchSession::default();
    let mut req = request("denied");
    req.kind = "EdgionTls".into();
    tokio::time::pause();
    session.start(req, tx, fixture.manager.clone(), authorizer());
    tokio::time::timeout(SEND_TIMEOUT + Duration::from_secs(1), session.cancelled.cancelled())
        .await
        .expect("unreported terminal error must end session");
    tokio::time::resume();
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
async fn history_expiry_and_projection_failure_end_current_watch() {
    for lag in [true, false] {
        let fixture = SessionFixture::new().await;
        let (tx, mut rx) = mpsc::channel(1);
        let mut session = WatchSession::default();
        session.start(
            request("invalidate"),
            Arc::new(tx),
            fixture.manager.clone(),
            authorizer(),
        );
        assert!(matches!(receive(&mut rx).await, CtrlPayload::WatchListResponse(_)));
        if lag {
            // No await: the watch cannot drain this bounded history on this test thread.
            for n in 0..1100 {
                publish_setting(&fixture.manager, "churn", &n.to_string());
            }
        } else {
            let runtime = fixture.manager.runtime();
            let key = SourceKey::new(
                crate::fed_sync::center_watch::projection::SourceKind::EdgionConfigData,
                "apps",
                "invalid",
            )
            .unwrap();
            let _ = runtime
                .center_watch
                .fail_source(&runtime.center_watch.generation(), key, StoreError::Projection);
        }
        assert_eq!(
            error(receive(&mut rx).await).error,
            if lag { "RelistRequired" } else { "ViewUnavailable" }
        );
        drop(session);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn readiness_loss_ends_current_watch() {
    let fixture = SessionFixture::new().await;
    let (tx, mut rx) = mpsc::channel(1);
    let mut session = WatchSession::default();
    session.start(request("ready"), Arc::new(tx), fixture.manager.clone(), authorizer());
    assert!(matches!(receive(&mut rx).await, CtrlPayload::WatchListResponse(_)));
    fixture.request_shutdown();
    assert_eq!(error(receive(&mut rx).await).error, "NotReady");
    drop(session);
    fixture.finish().await;
}

#[test]
fn proto_documents_the_dedicated_view_contract() {
    let controller = include_str!("../../../../src/fed_sync/proto/fed_sync.proto");
    assert!(controller.contains("only \"RegionRoute\""));
    assert!(controller.contains("CenterWatchStore generation"));
}

#[tokio::test]
async fn wire_encoding_matches_the_cross_repository_fixture() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/center-watch-wire.json")).unwrap();
    let fixture = SessionFixture::new().await;
    publish_setting(&fixture.manager, "fixture", "1");
    let runtime = fixture.manager.runtime();
    let list = runtime.center_watch.snapshot().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&list_bytes(&list.records).unwrap()).unwrap(),
        expected["list"]
    );
    publish_setting(&fixture.manager, "fixture", "2");
    let (_, changes) = runtime.center_watch.events_since(&list.generation, 1).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&event_bytes(&changes).unwrap()).unwrap(),
        expected["update"]
    );
    runtime.center_watch.delete(&list.generation, &changes[0].key).unwrap();
    let (_, changes) = runtime.center_watch.events_since(&list.generation, 2).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&event_bytes(&changes).unwrap()).unwrap(),
        expected["delete"]
    );
    fixture.finish().await;
}

#[test]
fn near_limit_plugin_wire_matches_the_cross_repository_fixture() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/center-watch-near-limit.json")).unwrap();
    let source: edgion_resources::prelude_resources::EdgionPlugins =
        serde_json::from_value(expected["source"].clone()).unwrap();
    let store = crate::fed_sync::center_watch::CenterWatchStore::default();
    let generation = store.generation();
    let record = crate::fed_sync::center_watch::projection::plugins(&source)
        .unwrap()
        .unwrap();
    store.upsert(&generation, record).unwrap();
    let snapshot = store.snapshot().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&list_bytes(&snapshot.records).unwrap()).unwrap(),
        expected["list"]
    );
}
