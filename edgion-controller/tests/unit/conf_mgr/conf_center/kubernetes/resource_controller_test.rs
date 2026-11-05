use super::*;

#[test]
fn alpha_watch_events_enter_the_canonical_lifecycle() {
    use edgion_resources::resources::TLSRouteAlpha3;
    let source: TLSRouteAlpha3 = serde_json::from_value(serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1alpha3",
        "kind": "TLSRoute",
        "metadata": {"name": "route", "namespace": "default", "resourceVersion": "17"},
        "spec": {"hostnames": ["example.com"], "rules": [{"backendRefs": [{"name": "backend", "port": 443}]}]},
        "status": {"parents": []}
    }))
    .unwrap();
    assert!(matches!(
        canonical_legacy_event::<TLSRouteAlpha3, edgion_resources::TLSRoute>(Event::Init),
        Event::Init
    ));
    assert!(matches!(
        canonical_legacy_event::<TLSRouteAlpha3, edgion_resources::TLSRoute>(Event::InitDone),
        Event::InitDone
    ));
    for event in [
        Event::InitApply(source.clone()),
        Event::Apply(source.clone()),
        Event::Delete(source.clone()),
    ] {
        let canonical: Event<edgion_resources::TLSRoute> = canonical_legacy_event(event);
        let (Event::InitApply(route) | Event::Apply(route) | Event::Delete(route)) = canonical else {
            unreachable!();
        };
        assert_eq!(route.metadata.resource_version.as_deref(), Some("17"));
        assert_eq!(route.spec.hostnames.as_ref().unwrap(), &vec!["example.com".to_string()]);
        assert!(route.status.is_some());
    }
}

#[test]
fn gateway_class_beta_events_keep_one_canonical_lifecycle() {
    use edgion_resources::{GatewayClassBeta, GatewayClassBetaSpec};
    let class = GatewayClassBeta {
        metadata: kube::api::ObjectMeta {
            name: Some("edge".into()),
            resource_version: Some("17".into()),
            ..Default::default()
        },
        spec: GatewayClassBetaSpec {
            controller_name: "example.com/edge".into(),
            description: None,
            parameters_ref: None,
        },
        status: None,
    };
    assert!(matches!(canonical_gateway_class_event(Event::Init), Event::Init));
    assert!(matches!(
        canonical_gateway_class_event(Event::InitDone),
        Event::InitDone
    ));
    for event in [
        Event::InitApply(class.clone()),
        Event::Apply(class.clone()),
        Event::Delete(class),
    ] {
        let canonical = canonical_gateway_class_event(event);
        let (Event::InitApply(obj) | Event::Apply(obj) | Event::Delete(obj)) = canonical else {
            unreachable!();
        };
        assert_eq!(obj.metadata.resource_version.as_deref(), Some("17"));
        assert_eq!(obj.spec.controller_name, "example.com/edge");
    }
}
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, ProcessResult, ProcessorHandler, ProcessorObj,
};
use crate::conf_mgr::ControllerRuntime;
use k8s_openapi::api::core::v1::ConfigMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

#[test]
fn beta_watch_events_enter_canonical_store_as_v1() {
    use edgion_resources::resources::{ReferenceGrantBetaSpec, ReferenceGrantFrom, ReferenceGrantTo};
    let beta = edgion_resources::ReferenceGrantBeta {
        metadata: kube::api::ObjectMeta {
            namespace: Some("backend".into()),
            name: Some("allow".into()),
            resource_version: Some("17".into()),
            ..Default::default()
        },
        spec: ReferenceGrantBetaSpec {
            from: vec![ReferenceGrantFrom {
                group: "gateway.networking.k8s.io".into(),
                kind: "HTTPRoute".into(),
                namespace: "app".into(),
            }],
            to: vec![ReferenceGrantTo {
                group: "".into(),
                kind: "Service".into(),
                name: Some("api".into()),
            }],
        },
    };
    for event in [
        Event::InitApply(beta.clone()),
        Event::Apply(beta.clone()),
        Event::Delete(beta),
    ] {
        let canonical = canonical_reference_grant_event(event);
        let (Event::InitApply(grant) | Event::Apply(grant) | Event::Delete(grant)) = canonical else {
            panic!("object event must remain an object event");
        };
        assert_eq!(grant.metadata.resource_version.as_deref(), Some("17"));
        assert_eq!(grant.spec.from[0].namespace, "app");
        assert_eq!(grant.spec.to[0].name.as_deref(), Some("api"));
        assert_eq!(<edgion_resources::ReferenceGrant as Resource>::version(&()), "v1");
    }
}

#[derive(Default)]
struct GrantEvents(parking_lot::Mutex<Vec<std::collections::HashSet<String>>>);

#[async_trait::async_trait]
impl crate::conf_mgr::sync_runtime::resource_processor::ref_grant::events::RevalidationListener for GrantEvents {
    async fn on_reference_grant_changed(
        &self,
        event: &crate::conf_mgr::sync_runtime::resource_processor::ref_grant::events::ReferenceGrantChangedEvent,
    ) {
        self.0.lock().push(event.affected_namespaces.clone());
    }
}

#[derive(Default)]
struct CountingGrantHandler {
    inner: crate::conf_mgr::sync_runtime::resource_processor::ReferenceGrantHandler,
    init_done: AtomicUsize,
}

#[async_trait::async_trait]
impl ProcessorHandler<edgion_resources::ReferenceGrant> for CountingGrantHandler {
    async fn parse(
        &self,
        obj: edgion_resources::ReferenceGrant,
        ctx: &HandlerContext,
    ) -> ProcessResult<edgion_resources::ReferenceGrant> {
        self.inner.parse(obj, ctx).await
    }
    async fn on_change(&self, obj: &edgion_resources::ReferenceGrant, ctx: &HandlerContext) {
        self.inner.on_change(obj, ctx).await;
    }
    async fn on_delete(&self, obj: &edgion_resources::ReferenceGrant, ctx: &HandlerContext) {
        self.inner.on_delete(obj, ctx).await;
    }
    fn on_init_done(&self, _ctx: &HandlerContext) {
        self.init_done.fetch_add(1, Ordering::SeqCst);
    }
}

fn beta_grant(from_namespace: &str) -> edgion_resources::ReferenceGrantBeta {
    use edgion_resources::resources::{ReferenceGrantBetaSpec, ReferenceGrantFrom, ReferenceGrantTo};
    edgion_resources::ReferenceGrantBeta {
        metadata: kube::api::ObjectMeta {
            namespace: Some("backend".into()),
            name: Some("allow".into()),
            resource_version: Some("17".into()),
            ..Default::default()
        },
        spec: ReferenceGrantBetaSpec {
            from: vec![ReferenceGrantFrom {
                group: "gateway.networking.k8s.io".into(),
                kind: "HTTPRoute".into(),
                namespace: from_namespace.into(),
            }],
            to: vec![ReferenceGrantTo {
                group: "".into(),
                kind: "Service".into(),
                name: Some("api".into()),
            }],
        },
    }
}

#[tokio::test]
async fn beta_watch_stream_reconciles_canonical_raw_store_and_relist_without_status() {
    let runtime = Arc::new(ControllerRuntime::new());
    let events = Arc::new(GrantEvents::default());
    runtime.ref_grant_dispatcher.register_listener(events.clone());
    let handler = Arc::new(CountingGrantHandler::default());
    let processor = Arc::new(ResourceProcessor::new(
        "ReferenceGrant",
        32,
        handler.clone(),
        runtime.clone(),
    ));
    let requests = Arc::new(AtomicUsize::new(0));
    let captured = requests.clone();
    let client = Client::new(
        tower::service_fn(move |_request: http::Request<kube::client::Body>| {
            let captured = captured.clone();
            async move {
                captured.fetch_add(1, Ordering::SeqCst);
                Ok::<_, std::convert::Infallible>(http::Response::new(kube::client::Body::empty()))
            }
        }),
        "default",
    );
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "ReferenceGrant",
        client,
        processor.clone(),
        ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone());
    let (tx, beta_stream) = source_channel::<edgion_resources::ReferenceGrantBeta>();
    let task = tracker.spawn(controller.run_sources(vec![(
        WatchScope::AllNamespaces,
        canonical_reference_grant_stream(beta_stream),
    )]));
    for event in [Event::Init, Event::InitApply(beta_grant("app-a")), Event::InitDone] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| processor.get("backend/allow").is_some() && !events.0.lock().is_empty()).await;
    let raw = processor.get_raw_any("backend/allow").unwrap();
    let raw = raw.downcast::<edgion_resources::ReferenceGrant>().unwrap();
    assert_eq!(raw.spec.from[0].namespace, "app-a");
    assert_eq!(handler.init_done.load(Ordering::SeqCst), 1);
    assert_eq!(tracker.len(), 2, "one source driver and one resource worker");

    tx.send(Ok(Event::Apply(beta_grant("app-b")))).await.unwrap();
    eventually(|| {
        processor
            .get("backend/allow")
            .is_some_and(|grant| grant.spec.from[0].namespace == "app-b")
            && events.0.lock().len() >= 2
    })
    .await;

    tx.send(Ok(Event::Delete(beta_grant("app-b")))).await.unwrap();
    eventually(|| processor.get("backend/allow").is_none() && events.0.lock().len() >= 3).await;
    tx.send(Ok(Event::Apply(beta_grant("app-c")))).await.unwrap();
    eventually(|| {
        processor
            .get("backend/allow")
            .is_some_and(|grant| grant.spec.from[0].namespace == "app-c")
            && events.0.lock().len() >= 4
    })
    .await;
    tx.send(Err(watcher::Error::NoResourceVersion)).await.unwrap();
    for event in [
        Event::Init,
        Event::InitApply(beta_grant("app-d")),
        Event::InitDone,
        Event::InitDone,
    ] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| {
        processor
            .get("backend/allow")
            .is_some_and(|grant| grant.spec.from[0].namespace == "app-d")
            && events.0.lock().len() >= 5
    })
    .await;
    // Every apply/delete revalidates dependents keyed by the grant namespace.
    assert!(events
        .0
        .lock()
        .iter()
        .all(|event| *event == std::collections::HashSet::from(["backend".to_string()])));
    assert_eq!(handler.init_done.load(Ordering::SeqCst), 1);
    assert_eq!(tracker.len(), 2, "relist must not spawn another worker");
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "ReferenceGrant has no status endpoint"
    );
    cancel.cancel();
    task.await.unwrap().unwrap();
    tracker.close();
    tracker.wait().await;
    assert!(processor.raw_keys().is_empty());
}

struct RecordingHandler {
    completed: AtomicUsize,
    applied: AtomicUsize,
    deleted: AtomicUsize,
    processor: parking_lot::Mutex<Option<std::sync::Weak<ResourceProcessor<ConfigMap>>>>,
    bootstrap_keys: parking_lot::Mutex<Vec<String>>,
    block: tokio::sync::Semaphore,
    entered: AtomicUsize,
}

impl Default for RecordingHandler {
    fn default() -> Self {
        Self {
            completed: AtomicUsize::new(0),
            applied: AtomicUsize::new(0),
            deleted: AtomicUsize::new(0),
            processor: parking_lot::Mutex::new(None),
            bootstrap_keys: parking_lot::Mutex::new(Vec::new()),
            block: tokio::sync::Semaphore::new(0),
            entered: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<ConfigMap> for RecordingHandler {
    fn filter(&self, obj: &ConfigMap, _ctx: &HandlerContext) -> bool {
        obj.metadata
            .labels
            .as_ref()
            .is_none_or(|labels| !labels.contains_key("excluded"))
    }
    async fn parse(&self, obj: ConfigMap, _ctx: &HandlerContext) -> ProcessResult<ConfigMap> {
        if obj
            .metadata
            .labels
            .as_ref()
            .is_some_and(|labels| labels.contains_key("panic"))
        {
            panic!("Injected worker failure");
        }
        if obj
            .metadata
            .labels
            .as_ref()
            .is_some_and(|labels| labels.contains_key("block"))
        {
            self.entered.fetch_add(1, Ordering::SeqCst);
            self.block.acquire().await.unwrap().forget();
        }
        ProcessResult::Continue(obj)
    }
    async fn on_change(&self, obj: &ConfigMap, _ctx: &HandlerContext) {
        if obj
            .metadata
            .labels
            .as_ref()
            .is_some_and(|labels| labels.contains_key("saturate"))
        {
            let processor = self.processor.lock().as_ref().unwrap().upgrade().unwrap();
            // Synthetic pressure probe of the existing delayed queue, not a production hook.
            for index in 0..12000 {
                processor
                    .workqueue()
                    .enqueue_after(
                        format!("a/dependency-{index}"),
                        Duration::ZERO,
                        crate::conf_mgr::sync_runtime::workqueue::TriggerChain::default(),
                    )
                    .await;
            }
        }
        if self.completed.load(Ordering::SeqCst) == 0 {
            let processor = self.processor.lock().as_ref().unwrap().upgrade().unwrap();
            let expected = self.bootstrap_keys.lock();
            for key in expected.iter() {
                assert!(
                    processor.get_raw_any(key).is_some(),
                    "missing complete bootstrap key {key}"
                );
            }
        }
        self.applied.fetch_add(1, Ordering::SeqCst);
    }
    async fn on_delete(&self, _obj: &ConfigMap, _ctx: &HandlerContext) {
        self.deleted.fetch_add(1, Ordering::SeqCst);
    }
    fn on_init_done(&self, _ctx: &HandlerContext) {
        self.completed.fetch_add(1, Ordering::SeqCst);
    }
}

fn object(ns: &str, name: &str) -> ConfigMap {
    let mut obj = ConfigMap::default();
    obj.metadata.name = Some(name.into());
    obj.metadata.namespace = Some(ns.into());
    obj
}

fn client() -> Client {
    Client::new(
        tower::service_fn(|_: http::Request<kube::client::Body>| async {
            Ok::<_, std::convert::Infallible>(http::Response::new(kube::client::Body::from(b"{}".to_vec())))
        }),
        "default",
    )
}

fn channel() -> (
    mpsc::Sender<watcher::Result<Event<ConfigMap>>>,
    NamespacedWatcher<ConfigMap>,
) {
    source_channel()
}

fn source_channel<K: Send + 'static>() -> (mpsc::Sender<watcher::Result<Event<K>>>, NamespacedWatcher<K>) {
    let (tx, rx) = mpsc::channel(32);
    (
        tx,
        Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|event| (event, rx))
        })),
    )
}

async fn send(tx: &mpsc::Sender<watcher::Result<Event<ConfigMap>>>, events: Vec<Event<ConfigMap>>) {
    for event in events {
        tx.send(Ok(event)).await.unwrap();
    }
}

async fn eventually(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn setup() -> (
    ResourceController<ConfigMap>,
    Arc<ResourceProcessor<ConfigMap>>,
    Arc<RecordingHandler>,
    CancellationToken,
    TaskTracker,
) {
    let runtime = Arc::new(ControllerRuntime::new());
    let handler = Arc::new(RecordingHandler::default());
    let processor = Arc::new(ResourceProcessor::new("ConfigMap", 10000, handler.clone(), runtime));
    *handler.processor.lock() = Some(Arc::downgrade(&processor));
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "ConfigMap",
        client(),
        processor.clone(),
        ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone());
    (controller, processor, handler, cancel, tracker)
}

#[tokio::test]
async fn namespace_watchers_expose_one_union_initialization_sequence() {
    let (controller, processor, handler, cancel, tracker) = setup();
    *handler.bootstrap_keys.lock() = vec!["a/old".into(), "a/added".into(), "b/kept".into(), "a/filtered".into()];
    let (a, stream_a) = channel();
    let (b, stream_b) = channel();
    let task = tracker.spawn(controller.run_sources(vec![
        (WatchScope::Namespace("a".into()), stream_a),
        (WatchScope::Namespace("b".into()), stream_b),
    ]));
    let mut filtered = object("a", "filtered");
    filtered.metadata.labels = Some(std::collections::BTreeMap::from([("excluded".into(), "true".into())]));
    send(
        &a,
        vec![
            Event::Init,
            Event::InitApply(object("a", "abandoned")),
            Event::Init,
            Event::InitApply(object("a", "old")),
            Event::InitApply(object("a", "removed")),
            Event::InitApply(filtered),
            Event::InitDone,
        ],
    )
    .await;
    eventually(|| processor.get_raw_any("a/old").is_some()).await;
    assert_eq!(handler.applied.load(Ordering::SeqCst), 0);
    assert!(processor.get_raw_any("a/abandoned").is_none());
    let mut updated = object("a", "old");
    updated.metadata.uid = Some("updated-before-bootstrap".into());
    send(
        &a,
        vec![
            Event::Apply(updated),
            Event::Apply(object("a", "added")),
            Event::Delete(object("a", "removed")),
            Event::Init,
            Event::InitApply(object("a", "new")),
        ],
    )
    .await;
    eventually(|| processor.get_raw_any("a/added").is_some() && processor.get_raw_any("a/removed").is_none()).await;
    assert_eq!(handler.applied.load(Ordering::SeqCst), 0);
    send(
        &b,
        vec![Event::Init, Event::InitApply(object("b", "kept")), Event::InitDone],
    )
    .await;
    eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
    assert_eq!(
        processor.get("a/old").unwrap().metadata.uid.as_deref(),
        Some("updated-before-bootstrap")
    );
    assert!(processor.get("a/added").is_some());
    assert!(processor.get("a/removed").is_none());
    assert!(processor.get("a/new").is_none());
    assert!(processor.get("a/filtered").is_none());
    send(&a, vec![Event::InitDone, Event::InitDone]).await;
    eventually(|| processor.get("a/new").is_some() && processor.get("a/old").is_none()).await;
    assert!(processor.get("b/kept").is_some());
    send(&a, vec![Event::Init, Event::InitDone]).await;
    eventually(|| processor.get("a/new").is_none()).await;
    assert!(processor.get("b/kept").is_some());
    assert_eq!(handler.completed.load(Ordering::SeqCst), 1);
    cancel.cancel();
    task.await.unwrap().unwrap();
    tracker.close();
    tracker.wait().await;
    assert!(processor.raw_keys().is_empty());
}

#[tokio::test]
async fn relist_larger_than_queue_capacity_converges_without_reinitialization() {
    let (controller, processor, handler, cancel, tracker) = setup();
    let (tx, stream) = channel();
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    assert!(super::super::WorkqueueConfig::default().capacity < 5100);
    tx.send(Ok(Event::Init)).await.unwrap();
    // Bootstrap ingress must never fill the pre-worker queue.
    for index in 0..5100 {
        tx.send(Ok(Event::InitApply(object("a", &format!("item-{index}")))))
            .await
            .unwrap();
    }
    tx.send(Ok(Event::InitDone)).await.unwrap();
    eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
    assert_eq!(processor.cache().list_keys().len(), 5100);
    send(&tx, vec![Event::Init, Event::InitDone]).await;
    eventually(|| processor.cache().list_keys().is_empty()).await;
    assert_eq!(handler.deleted.load(Ordering::SeqCst), 5100);
    assert_eq!(handler.completed.load(Ordering::SeqCst), 1);
    cancel.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn dirty_requeue_reads_latest_committed_object_and_eligibility_loss_deletes_cached_generation() {
    let (controller, processor, handler, cancel, tracker) = setup();
    let (tx, stream) = channel();
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    send(
        &tx,
        vec![Event::Init, Event::InitApply(object("a", "x")), Event::InitDone],
    )
    .await;
    eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
    let mut blocked = object("a", "x");
    blocked.metadata.labels = Some(std::collections::BTreeMap::from([("block".into(), "true".into())]));
    send(&tx, vec![Event::Apply(blocked)]).await;
    eventually(|| handler.entered.load(Ordering::SeqCst) == 1).await;
    let mut replacement = object("a", "x");
    replacement.metadata.uid = Some("replacement".into());
    replacement.metadata.labels = Some(std::collections::BTreeMap::from([("excluded".into(), "true".into())]));
    send(&tx, vec![Event::Init, Event::InitApply(replacement), Event::InitDone]).await;
    eventually(|| {
        processor
            .get_raw_any("a/x")
            .unwrap()
            .downcast::<ConfigMap>()
            .unwrap()
            .metadata
            .uid
            .as_deref()
            == Some("replacement")
    })
    .await;
    handler.block.add_permits(1);
    eventually(|| processor.get("a/x").is_none()).await;
    assert!(processor.get_raw_any("a/x").is_some());
    assert_eq!(handler.deleted.load(Ordering::SeqCst), 1);
    cancel.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn required_source_eof_and_worker_panic_are_terminal_and_drop_clears_raw_state() {
    for panic_worker in [false, true] {
        let (controller, processor, handler, _cancel, tracker) = setup();
        let (tx, stream) = channel();
        let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
        send(
            &tx,
            vec![Event::Init, Event::InitApply(object("a", "x")), Event::InitDone],
        )
        .await;
        eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
        if panic_worker {
            let mut obj = object("a", "x");
            obj.metadata.labels = Some(std::collections::BTreeMap::from([("panic".into(), "true".into())]));
            send(&tx, vec![Event::Apply(obj)]).await;
        } else {
            drop(tx);
        }
        assert!(tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        tracker.close();
        tracker.wait().await;
        assert!(processor.raw_keys().is_empty());
    }
}

#[tokio::test]
async fn cancellation_during_replay_releases_store_and_does_not_start_worker() {
    let (controller, processor, handler, cancel, tracker) = setup();
    let (tx, stream) = channel();
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    let mut obj = object("a", "x");
    obj.metadata.labels = Some(std::collections::BTreeMap::from([("block".into(), "true".into())]));
    send(&tx, vec![Event::Init, Event::InitApply(obj), Event::InitDone]).await;
    eventually(|| handler.entered.load(Ordering::SeqCst) == 1).await;
    cancel.cancel();
    task.await.unwrap().unwrap();
    tracker.close();
    tracker.wait().await;
    assert!(processor.raw_keys().is_empty());
    assert_eq!(handler.completed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn secret_accumulator_commits_once_and_relist_removes_only_its_source() {
    use crate::conf_mgr::sync_runtime::resource_processor::SecretHandler;
    use k8s_openapi::api::core::v1::Secret;
    let runtime = Arc::new(ControllerRuntime::new());
    let processor = Arc::new(ResourceProcessor::new(
        "Secret",
        100,
        Arc::new(SecretHandler::new()),
        runtime.clone(),
    ));
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "Secret",
        client(),
        processor.clone(),
        ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone());
    let (a, sa) = source_channel();
    let (b, sb) = source_channel();
    let task = tracker.spawn(controller.run_sources(vec![
        (WatchScope::Namespace("a".into()), sa),
        (WatchScope::Namespace("b".into()), sb),
    ]));
    let secret = |ns: &str, name: &str| Secret {
        metadata: object(ns, name).metadata,
        ..Default::default()
    };
    for event in [
        Event::Init,
        Event::InitApply(secret("a", "abandoned")),
        Event::Init,
        Event::InitApply(secret("a", "old")),
        Event::InitDone,
    ] {
        a.send(Ok(event)).await.unwrap();
    }
    eventually(|| processor.get_raw_any("a/old").is_some()).await;
    assert_eq!(runtime.secret_store.count(), 0);
    for event in [Event::Init, Event::InitApply(secret("b", "kept")), Event::InitDone] {
        b.send(Ok(event)).await.unwrap();
    }
    eventually(|| processor.is_ready()).await;
    assert_eq!(runtime.secret_store.keys(), vec!["a/old", "b/kept"]);
    for event in [Event::Init, Event::InitApply(secret("a", "new")), Event::InitDone] {
        a.send(Ok(event)).await.unwrap();
    }
    eventually(|| {
        runtime.secret_store.get(Some("a"), "new").is_some() && runtime.secret_store.get(Some("a"), "old").is_none()
    })
    .await;
    assert!(runtime.secret_store.get(Some("b"), "kept").is_some());
    assert!(processor.is_ready());
    cancel.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn identical_rv_relist_retries_failed_status_through_the_existing_worker() {
    use k8s_openapi::api::core::v1::{LoadBalancerIngress, LoadBalancerStatus, Service, ServiceStatus};
    struct StatusHandler;
    #[async_trait::async_trait]
    impl ProcessorHandler<Service> for StatusHandler {
        fn update_status(&self, obj: &mut Service, _: &HandlerContext, _: &[String]) {
            obj.status = Some(ServiceStatus {
                load_balancer: Some(LoadBalancerStatus {
                    ingress: Some(vec![LoadBalancerIngress {
                        ip: Some("192.0.2.1".into()),
                        ..Default::default()
                    }]),
                }),
                ..Default::default()
            });
        }
    }
    let attempts = Arc::new(AtomicUsize::new(0));
    let captured = attempts.clone();
    let transport = tower::service_fn(move |request: http::Request<kube::client::Body>| {
        let attempt = captured.fetch_add(1, Ordering::SeqCst);
        async move {
            assert_eq!(request.method(), http::Method::PATCH);
            assert_eq!(request.uri().path(), "/api/v1/namespaces/a/services/backend/status");
            let (status, value) = if attempt == 0 {
                (
                    503,
                    serde_json::json!({"kind":"Status","apiVersion":"v1","status":"Failure","message":"Unavailable","reason":"ServiceUnavailable","code":503}),
                )
            } else {
                (
                    200,
                    serde_json::json!({"kind":"Service","apiVersion":"v1","metadata":{"name":"backend","namespace":"a"}}),
                )
            };
            Ok::<_, std::convert::Infallible>(
                http::Response::builder()
                    .status(status)
                    .body(kube::client::Body::from(serde_json::to_vec(&value).unwrap()))
                    .unwrap(),
            )
        }
    });
    let processor = Arc::new(ResourceProcessor::new(
        "Service",
        100,
        Arc::new(StatusHandler),
        Arc::new(ControllerRuntime::new()),
    ));
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "Service",
        Client::new(transport, "default"),
        processor.clone(),
        ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone());
    let (tx, stream) = source_channel();
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    let mut service = Service {
        metadata: object("a", "backend").metadata,
        ..Default::default()
    };
    service.metadata.resource_version = Some("same-rv".into());
    for event in [Event::Init, Event::InitApply(service.clone()), Event::InitDone] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| processor.is_ready()).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    for event in [Event::Init, Event::InitApply(service), Event::InitDone] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| attempts.load(Ordering::SeqCst) == 2).await;
    assert!(processor.is_ready());
    cancel.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn source_backoff_does_not_stall_other_sources_or_cancellation() {
    let (controller, processor, handler, cancel, tracker) = setup();
    let (a, sa) = channel();
    let (b, sb) = channel();
    let task = tracker.spawn(controller.run_sources(vec![
        (WatchScope::Namespace("a".into()), Box::pin(sa.default_backoff())),
        (WatchScope::Namespace("b".into()), sb),
    ]));
    send(
        &a,
        vec![Event::Init, Event::InitApply(object("a", "old")), Event::InitDone],
    )
    .await;
    eventually(|| processor.get_raw_any("a/old").is_some()).await;
    a.send(Err(watcher::Error::NoResourceVersion)).await.unwrap();
    send(&a, vec![Event::Init, Event::InitDone]).await;
    send(
        &b,
        vec![Event::Init, Event::InitApply(object("b", "kept")), Event::InitDone],
    )
    .await;
    eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
    assert!(processor.get("a/old").is_some());
    assert!(processor.get("b/kept").is_some());
    cancel.cancel();
    task.await.unwrap().unwrap();
    assert!(processor.raw_keys().is_empty());
}

#[tokio::test]
async fn cancellation_of_blocked_resync_and_forced_abort_drain_the_owned_worker() {
    for forced_abort in [false, true] {
        let (controller, processor, handler, cancel, tracker) = setup();
        let (tx, stream) = channel();
        let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
        send(&tx, vec![Event::Init, Event::InitDone]).await;
        eventually(|| handler.completed.load(Ordering::SeqCst) == 1).await;
        let mut blocked = object("a", "blocked");
        blocked.metadata.labels = Some(std::collections::BTreeMap::from([("block".into(), "true".into())]));
        send(&tx, vec![Event::Apply(blocked)]).await;
        eventually(|| handler.entered.load(Ordering::SeqCst) == 1).await;
        tx.send(Ok(Event::Init)).await.unwrap();
        for index in 0..5100 {
            tx.send(Ok(Event::InitApply(object("a", &format!("item-{index}")))))
                .await
                .unwrap();
        }
        tx.send(Ok(Event::InitDone)).await.unwrap();
        eventually(|| {
            processor.workqueue().metrics().get_depth() > super::super::WorkqueueConfig::default().capacity as u64
        })
        .await;
        assert_eq!(processor.raw_keys().len(), 5100);
        if forced_abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            cancel.cancel();
            task.await.unwrap().unwrap();
        }
        tracker.close();
        tokio::time::timeout(Duration::from_secs(5), tracker.wait())
            .await
            .unwrap();
        assert!(processor.raw_keys().is_empty());
        assert_eq!(handler.completed.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn route_uid_recreation_relist_replaces_real_handler_parent_dependencies() {
    use crate::conf_mgr::sync_runtime::resource_processor::HttpRouteHandler;
    use edgion_resources::prelude_resources::{Gateway, HTTPRoute};
    struct GatewayFixture;
    #[async_trait::async_trait]
    impl ProcessorHandler<Gateway> for GatewayFixture {}
    let runtime = Arc::new(ControllerRuntime::new());
    let gateway = Arc::new(ResourceProcessor::new(
        "Gateway",
        100,
        Arc::new(GatewayFixture),
        runtime.clone(),
    ));
    runtime.processor_registry.register(gateway.clone());
    let processor = Arc::new(ResourceProcessor::new(
        "HTTPRoute",
        100,
        Arc::new(HttpRouteHandler::new("edgion.io/gateway-controller".into())),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();
    let controller = ResourceController::new(
        "HTTPRoute",
        client(),
        processor.clone(),
        ApiScope::Namespaced(NamespaceWatchMode::AllNamespaces),
        watcher::Config::default(),
    )
    .with_epoch(cancel.clone(), tracker.clone());
    let (tx, stream) = source_channel();
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    let route = |parent: &str, uid: &str| {
        serde_json::from_value::<HTTPRoute>(serde_json::json!({
            "metadata":{"name":"route","namespace":"a","uid":uid},"spec":{"parentRefs":[{"name":parent}]}
        }))
        .unwrap()
    };
    for event in [Event::Init, Event::InitApply(route("old", "uid1")), Event::InitDone] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| processor.is_ready()).await;
    let first = tokio::time::timeout(Duration::from_secs(5), gateway.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.key, "a/old");
    for event in [Event::Init, Event::InitApply(route("new", "uid2")), Event::InitDone] {
        tx.send(Ok(event)).await.unwrap();
    }
    eventually(|| {
        processor
            .get("a/route")
            .is_some_and(|route| route.metadata.uid.as_deref() == Some("uid2"))
    })
    .await;
    assert!(runtime.gateway_route_index.get_routes_for_gateway("a/old").is_empty());
    assert_eq!(runtime.gateway_route_index.get_routes_for_gateway("a/new").len(), 1);
    let mut requeued = std::collections::HashSet::new();
    while requeued.len() < 2 {
        requeued.insert(
            tokio::time::timeout(Duration::from_secs(5), gateway.workqueue().dequeue())
                .await
                .unwrap()
                .unwrap()
                .key,
        );
    }
    assert_eq!(
        requeued,
        std::collections::HashSet::from(["a/old".into(), "a/new".into()])
    );
    cancel.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn synthetic_pre_worker_delayed_requeue_saturation_remains_cancellable() {
    let (controller, processor, handler, cancel, tracker) = setup();
    let capacity = super::super::WorkqueueConfig::default().capacity as u64;
    // Deliberately seed the pre-existing queue pressure before source ingestion.
    // This models the scheduler boundary; it is not a real production hook claim.
    for index in 0..capacity {
        processor.on_event_key(format!("a/prior-{index}")).await;
    }
    processor
        .workqueue()
        .enqueue_after(
            "a/prior-delayed".into(),
            Duration::ZERO,
            crate::conf_mgr::sync_runtime::workqueue::TriggerChain::default(),
        )
        .await;
    eventually(|| processor.workqueue().metrics().get_depth() > capacity).await;
    let (tx, stream) = channel();
    let task = tracker.spawn(controller.run_sources(vec![(WatchScope::AllNamespaces, stream)]));
    let mut obj = object("a", "pressure");
    obj.metadata.labels = Some(std::collections::BTreeMap::from([("saturate".into(), "true".into())]));
    send(&tx, vec![Event::Init, Event::InitApply(obj), Event::InitDone]).await;
    eventually(|| {
        processor.workqueue().metrics().get_depth() > capacity
            && processor.workqueue().metrics().delayed_total.load(Ordering::SeqCst) >= capacity + 2
    })
    .await;
    assert_eq!(handler.completed.load(Ordering::SeqCst), 0);
    assert!(!processor.is_ready());
    cancel.cancel();
    task.await.unwrap().unwrap();
    tracker.close();
    tracker.wait().await;
    assert!(processor.raw_keys().is_empty());
}
