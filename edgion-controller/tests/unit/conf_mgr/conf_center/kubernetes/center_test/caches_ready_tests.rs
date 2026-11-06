use super::*;
use tokio_util::sync::CancellationToken;

#[tokio::test(start_paused = true)]
async fn all_serve_shutdown_during_bootstrap_or_backoff_stops_without_restart() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for (outer_lifecycle, fail_preflight) in [(false, false), (true, false), (true, true)] {
        let mut center = super::leadership_signal_tests::unconnected_center();
        center.config.no_watch_kinds = super::super::version_detection::gateway_api_kind_names()
            .into_iter()
            .filter(|kind| !fail_preflight || *kind != "GatewayClass")
            .map(String::from)
            .collect();
        let requests = Arc::new(AtomicUsize::new(0));
        let request_started = Arc::new(tokio::sync::Notify::new());
        let client = Client::new(
            tower::service_fn({
                let requests = requests.clone();
                let request_started = request_started.clone();
                move |request: http::Request<kube::client::Body>| {
                    if fail_preflight || request.uri().path() == "/api/v1/namespaces" {
                        requests.fetch_add(1, Ordering::SeqCst);
                        request_started.notify_one();
                    }
                    async move {
                        if fail_preflight {
                            return Err(std::io::Error::other("Injected preflight failure"));
                        }
                        futures::future::pending::<Result<http::Response<kube::client::Body>, std::io::Error>>().await
                    }
                }
            }),
            "default",
        );
        // Version probes run through the writer; share the injected transport.
        center.writer = KubernetesStorage::new_unconnected_for_test().with_client_for_test(client.clone());
        let leader = LeaderElection::new(
            client.clone(),
            InternalLeaderElectionConfig {
                lease_name: "test".into(),
                lease_namespace: "default".into(),
                pod_namespace: "default".into(),
                identity: "test".into(),
                lease_duration_secs: 15,
                renew_deadline_secs: 10,
                retry_period_secs: 2,
            },
            center.leader_flag.clone(),
        )
        .handle();
        let shutdown = ShutdownHandle::new();
        let trigger = tokio::spawn({
            let shutdown = shutdown.clone();
            async move {
                request_started.notified().await;
                if fail_preflight {
                    // Let the immediate preflight failure enter its two-second retry backoff.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                shutdown.shutdown();
            }
        });
        // Epoch drain permits the existing five-second forced-abort grace.
        let timeout = Duration::from_secs(if fail_preflight { 1 } else { 8 });
        tokio::time::timeout(timeout, async {
            if outer_lifecycle {
                center
                    .run_all_serve_lifecycle(&client, &leader, &shutdown)
                    .await
                    .unwrap();
            } else {
                assert!(matches!(
                    center.run_serving_flow(&client, &leader, &shutdown).await,
                    ServingFlowExit::Shutdown
                ));
            }
        })
        .await
        .expect("shutdown must drain the pending epoch without a retry");
        assert_eq!(requests.load(Ordering::SeqCst), 1, "no replacement epoch may start");
        tokio::time::timeout(Duration::from_secs(1), trigger)
            .await
            .unwrap()
            .unwrap();
        assert!(center.config_sync_server.read().is_none());
        assert!(center.runtime.processor_registry.all_watch_objs(&[]).is_empty());
    }
}

#[tokio::test]
async fn all_serve_shutdown_arriving_during_drain_overrides_failure() {
    let center = super::leadership_signal_tests::unconnected_center();
    center.set_config_sync_server(Some(Arc::new(ConfigSyncServer::new())));
    let watchers = WatcherSet::new(CancellationToken::new());
    let shutdown = ShutdownHandle::new();
    let cancel = watchers.cancel_token();
    watchers.tracker.spawn({
        let shutdown = shutdown.clone();
        async move {
            cancel.cancelled().await;
            shutdown.shutdown();
        }
    });
    assert!(!shutdown.is_shutdown());
    assert!(matches!(
        center
            .finish_serving_flow(
                watchers,
                false,
                ControllerExitReason::AllControllersStopped,
                false,
                &shutdown
            )
            .await,
        ServingFlowExit::Shutdown
    ));
    assert!(center.config_sync_server.read().is_none());
}

#[tokio::test]
async fn no_processor_kinds_still_wait_for_namespace_and_phase_completion() {
    let (tx, mut rx) = mpsc::channel(4);
    let (phase_tx, phase_rx) = watch::channel(false);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(KubernetesCenter::run_caches_ready(
        Arc::new(ConfigSyncServer::new()),
        Arc::new(ControllerRuntime::new()),
        Vec::new(),
        Vec::new(),
        tx,
        None,
        None,
        Vec::new(),
        cancel.clone(),
        phase_rx,
    ));
    tokio::task::yield_now().await;
    assert!(rx.try_recv().is_err());
    phase_tx.send_replace(true);
    assert!(matches!(rx.recv().await, Some(LifecycleEvent::CachesReady)));
    task.await.unwrap();
}

#[tokio::test]
async fn tracker_drain_waits_for_descendants_after_spawner_is_aborted() {
    let cancel = CancellationToken::new();
    let mut watchers = WatcherSet::new(cancel);
    let tracker = watchers.tracker.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    watchers.spawn("controller", async move {
        tracker.spawn(async move {
            let _ = started_tx.send(());
            let _ = finish_rx.await;
        });
        futures::future::pending::<()>().await;
    });
    started_rx.await.unwrap();
    watchers.set.abort_all();
    let task = tokio::spawn(watchers.abort_and_wait());
    tokio::task::yield_now().await;
    assert!(!task.is_finished(), "descendant still owns the epoch");
    finish_tx.send(()).unwrap();
    task.await.unwrap();
}

#[test]
fn epoch_cancellation_before_leadership_event_never_becomes_process_shutdown() {
    for leader in [false, true] {
        for reason in [
            ControllerExitReason::Shutdown,
            ControllerExitReason::AllControllersStopped,
        ] {
            assert!(matches!(
                classify_leader_only_exit(reason, false, leader, true),
                ControllerExitReason::LostLeadership
            ));
        }
    }
    assert!(matches!(
        classify_leader_only_exit(ControllerExitReason::AllControllersStopped, true, false, true),
        ControllerExitReason::Shutdown
    ));
    assert!(matches!(
        classify_leader_only_exit(ControllerExitReason::AllControllersStopped, false, true, false),
        ControllerExitReason::AllControllersStopped
    ));
}

#[tokio::test]
async fn pre_cancelled_token_suppresses_caches_ready() {
    let (tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let runtime = Arc::new(ControllerRuntime::default());
    let css = Arc::new(ConfigSyncServer::new());
    // No expected kinds → readiness poll passes immediately; the cancelled
    // token must stop the tail before CachesReady is sent.
    KubernetesCenter::run_caches_ready(
        css,
        runtime,
        Vec::new(),
        Vec::new(),
        tx,
        None,
        None,
        Vec::new(),
        cancel,
        watch::channel(true).1,
    )
    .await;
    assert!(
        rx.try_recv().is_err(),
        "cancelled caches task must not send CachesReady"
    );
}

#[tokio::test(start_paused = true)]
async fn caches_ready_poll_has_no_inner_deadline_and_stops_on_cancel() {
    let (tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
    let cancel = CancellationToken::new();
    // An expected kind that never registers keeps the poll pending.
    let task = tokio::spawn(KubernetesCenter::run_caches_ready(
        Arc::new(ConfigSyncServer::new()),
        Arc::new(ControllerRuntime::default()),
        vec!["Gateway"],
        Vec::new(),
        tx,
        None,
        None,
        Vec::new(),
        cancel.clone(),
        watch::channel(true).1,
    ));
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(!task.is_finished(), "the poll must not give up on an inner deadline");
    assert!(rx.try_recv().is_err(), "no event may be sent while kinds are pending");
    cancel.cancel();
    task.await.unwrap();
    assert!(rx.try_recv().is_err());
}
