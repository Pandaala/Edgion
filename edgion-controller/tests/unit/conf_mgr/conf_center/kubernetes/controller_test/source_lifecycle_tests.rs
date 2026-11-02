use super::super::ShutdownController;
use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::raw_store::LocalRawStore;
use futures::FutureExt;
use kube::runtime::watcher::Event;

#[test]
fn gateway_class_reporting_bypasses_labels_without_changing_other_watches() {
    let config = watcher::Config::default().labels("team=blue");
    assert!(cluster_watcher_config("GatewayClass", &config).label_selector.is_none());
    assert_eq!(
        cluster_watcher_config("EdgionGatewayConfig", &config).label_selector,
        Some("team=blue".into())
    );
    assert_eq!(config.label_selector, Some("team=blue".into()));
}

struct GatewayFixtureHandler;
#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for GatewayFixtureHandler {}

fn ns(name: &str) -> Namespace {
    let mut ns = Namespace::default();
    ns.metadata.name = Some(name.into());
    ns
}

async fn eventually(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn actual_startup_respects_endpoint_slice_selection_and_keeps_namespace_gate() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for enabled in [false, true] {
        let namespace_requests = Arc::new(AtomicUsize::new(0));
        let slice_requests = Arc::new(AtomicUsize::new(0));
        let client = Client::new(
            tower::service_fn({
                let namespace_requests = namespace_requests.clone();
                let slice_requests = slice_requests.clone();
                move |request: http::Request<kube::client::Body>| {
                    let path = request.uri().path();
                    if path == "/api/v1/namespaces" {
                        namespace_requests.fetch_add(1, Ordering::SeqCst);
                    } else if path.ends_with("/endpointslices") {
                        slice_requests.fetch_add(1, Ordering::SeqCst);
                    } else {
                        panic!("unexpected startup request: {path}");
                    }
                    futures::future::pending::<Result<http::Response<kube::client::Body>, std::io::Error>>()
                }
            }),
            "default",
        );
        let cancel = CancellationToken::new();
        let tracker = TaskTracker::new();
        let (ready_tx, ready_rx) = watch::channel(false);
        let mut controller = KubernetesController::with_metadata_filter(
            client,
            "edgion.io/test".into(),
            None,
            vec![],
            None,
            MetadataFilterConfig::default(),
            HashSet::new(),
        )
        .unwrap()
        .with_epoch(cancel.clone(), tracker.clone())
        .with_phases_ready(ready_tx);
        controller.no_watch_kinds = controller
            .all_processor_kinds()
            .into_iter()
            .map(str::to_owned)
            .collect();
        if enabled {
            controller.no_watch_kinds.remove("EndpointSlice");
        }
        let expected = controller.all_processor_kinds();
        assert_eq!(expected, if enabled { vec!["EndpointSlice"] } else { vec![] });
        let runtime = Arc::new(ControllerRuntime::new());
        let (_shutdown, signal) = ShutdownController::new();
        let task = tokio::spawn({
            let runtime = runtime.clone();
            async move { controller.run_controllers(signal, runtime).await }
        });
        eventually(|| {
            namespace_requests.load(Ordering::SeqCst) > 0 && (!enabled || slice_requests.load(Ordering::SeqCst) > 0)
        })
        .await;
        assert_eq!(runtime.processor_registry.all_kinds(), expected);
        assert_eq!(slice_requests.load(Ordering::SeqCst) > 0, enabled);
        assert!(!*ready_rx.borrow(), "Namespace's first snapshot is still pending");
        assert!(!runtime.processor_registry.is_registration_complete());
        assert!(!task.is_finished());
        cancel.cancel();
        assert!(tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        tracker.close();
        tokio::time::timeout(Duration::from_secs(5), tracker.wait())
            .await
            .unwrap();
        assert!(tracker.is_empty());
    }
}

#[tokio::test]
async fn namespace_relist_and_incremental_changes_defer_refresh_until_gateway_bootstrap() {
    let runtime = Arc::new(ControllerRuntime::new());
    let processor = Arc::new(ResourceProcessor::new(
        "Gateway",
        10,
        Arc::new(GatewayFixtureHandler),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let gateway: Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {"name":"gateway", "namespace":"a"},
        "spec": {"gatewayClassName":"edgion", "listeners":[{"name":"http", "port":80,"protocol":"HTTP",
            "allowedRoutes":{"namespaces":{"from":"Selector","selector":{"matchLabels":{"team":"a"}}}}}]}
    }))
    .unwrap();
    let raw = Arc::new(LocalRawStore::new());
    raw.upsert("a/gateway".into(), Arc::new(gateway.clone()));
    processor.set_raw_store(raw);
    let (shutdown, signal) = ShutdownController::new();
    let cancel = CancellationToken::new();
    let (ready_tx, ready_rx) = watch::channel(false);
    let (gateway_tx, gateway_rx) = watch::channel(false);
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let stream = futures::stream::unfold(rx, |mut rx| async { rx.recv().await.map(|event| (event, rx)) });
    let task = tokio::spawn(run_namespace_stream(
        stream,
        signal,
        runtime.clone(),
        cancel.clone(),
        ready_tx,
        gateway_rx,
        true,
    ));
    for event in [Event::Init, Event::InitApply(ns("a")), Event::InitDone] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| *ready_rx.borrow()).await;
    assert!(runtime.namespace_store.get("a").is_some());
    // Relist retains the previous committed map until completion, then removes terminating namespaces.
    let mut terminating = ns("a");
    terminating.metadata.deletion_timestamp = Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
        k8s_openapi::jiff::Timestamp::now(),
    ));
    for event in [
        Event::Init,
        Event::InitApply(terminating),
        Event::InitDone,
        Event::InitDone,
        Event::Apply(ns("b")),
    ] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| runtime.namespace_store.get("a").is_none() && runtime.namespace_store.get("b").is_some()).await;
    assert!(processor.workqueue().dequeue().now_or_never().is_none());
    // Publish Gateway only after the Namespace changes: an early enumeration would miss it.
    processor.save(gateway, None);
    processor.on_init_done();
    gateway_tx.send_replace(true);
    let item = tokio::time::timeout(Duration::from_secs(5), processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "a/gateway");
    tx.send(Ok(Event::Delete(ns("b")))).await.unwrap();
    let item = tokio::time::timeout(Duration::from_secs(5), processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "a/gateway");
    assert!(runtime.namespace_store.get("b").is_none());
    shutdown.shutdown();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn namespace_disabled_gateway_needs_no_ready_signal_and_rejects_bad_runtime_identity() {
    for bad_delete in [false, true] {
        let runtime = Arc::new(ControllerRuntime::new());
        let (_shutdown, signal) = ShutdownController::new();
        let (ready_tx, ready_rx) = watch::channel(false);
        let (_gateway_tx, gateway_rx) = watch::channel(false);
        let mut invalid = ns("bad");
        invalid.metadata.namespace = Some("wrong".into());
        let events = vec![
            Ok(Event::Init),
            Ok(Event::InitApply(ns("a"))),
            Ok(Event::InitDone),
            Ok(Event::Apply(ns("b"))),
            Ok(if bad_delete {
                Event::Delete(invalid)
            } else {
                Event::Apply(invalid)
            }),
        ];
        let result = run_namespace_stream(
            futures::stream::iter(events),
            signal,
            runtime.clone(),
            CancellationToken::new(),
            ready_tx,
            gateway_rx,
            false,
        )
        .await;
        assert!(result.is_err());
        assert!(*ready_rx.borrow());
        assert!(runtime.namespace_store.get("b").is_some());
        assert!(runtime.namespace_store.get("bad").is_none());
    }
}

#[tokio::test]
async fn phase_wait_observes_required_child_failure_without_waiting_for_timeout() {
    let mut children = JoinSet::new();
    children.spawn(async { Err(anyhow::anyhow!("source EOF")) });
    let (_shutdown, mut signal) = ShutdownController::new();
    let result = wait_phase(
        &mut children,
        &CancellationToken::new(),
        &mut signal,
        futures::future::pending(),
        Duration::from_secs(60),
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("Required child"));
}
