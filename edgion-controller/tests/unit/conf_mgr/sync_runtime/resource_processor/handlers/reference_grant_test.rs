use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::events::{
    ReferenceGrantChangedEvent, RevalidationListener,
};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::{
    ReferenceGrantBeta, ReferenceGrantBetaSpec, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use parking_lot::Mutex;

/// Recording listener that captures every dispatched event in order.
struct RecordingListener {
    events: Mutex<Vec<HashSet<String>>>,
}

impl RecordingListener {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
        })
    }

    fn snapshot(&self) -> Vec<HashSet<String>> {
        self.events.lock().clone()
    }
}

#[async_trait::async_trait]
impl RevalidationListener for RecordingListener {
    async fn on_reference_grant_changed(&self, event: &ReferenceGrantChangedEvent) {
        self.events.lock().push(event.affected_namespaces.clone());
    }
}

fn build_grant(namespace: &str, name: &str, from_namespaces: &[&str]) -> ReferenceGrant {
    ReferenceGrant {
        metadata: ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: ReferenceGrantSpec {
            from: from_namespaces
                .iter()
                .map(|ns| ReferenceGrantFrom {
                    group: "gateway.networking.k8s.io".to_string(),
                    kind: "HTTPRoute".to_string(),
                    namespace: (*ns).to_string(),
                })
                .collect(),
            to: vec![ReferenceGrantTo {
                group: "".to_string(),
                kind: "Service".to_string(),
                name: None,
            }],
        },
    }
}

fn make_ctx(runtime: Arc<ControllerRuntime>) -> HandlerContext {
    HandlerContext::new(
        None,
        None,
        TriggerChain::default(),
        8,  // max_trigger_cycles
        32, // max_trigger_depth
        runtime,
    )
}

/// Union of all events' namespaces — useful when assertion does not care
/// whether dispatch happened in one event or two.
fn union_dispatched(events: &[HashSet<String>]) -> HashSet<String> {
    let mut acc: HashSet<String> = HashSet::new();
    for ev in events {
        acc.extend(ev.iter().cloned());
    }
    acc
}

#[tokio::test]
async fn beta_conversion_preserves_delete_recreate_dependency_dispatch() {
    let runtime = Arc::new(ControllerRuntime::default());
    let listener = RecordingListener::new();
    runtime.ref_grant_dispatcher.register_listener(listener.clone());
    let ctx = make_ctx(runtime.clone());
    let handler = ReferenceGrantHandler::new();
    let original = build_grant("backend", "grant", &["app-a"]);
    let beta = ReferenceGrantBeta {
        metadata: original.metadata.clone(),
        spec: ReferenceGrantBetaSpec {
            from: original.spec.from.clone(),
            to: original.spec.to.clone(),
        },
    };
    let canonical: ReferenceGrant = beta.into();
    let key = ReferenceGrantHandler::build_key(&canonical);
    match handler.parse(canonical.clone(), &ctx).await {
        ProcessResult::Continue(grant) => handler.on_change(&grant, &ctx).await,
        ProcessResult::Skip { .. } => panic!("unexpected skip"),
    }
    handler.on_delete(&canonical, &ctx).await;
    assert!(runtime.ref_grant_store.get(&key).is_none());
    let recreated = build_grant("backend", "grant", &["app-b"]);
    match handler.parse(recreated, &ctx).await {
        ProcessResult::Continue(grant) => handler.on_change(&grant, &ctx).await,
        ProcessResult::Skip { .. } => panic!("unexpected skip"),
    }
    assert!(runtime.ref_grant_store.get(&key).is_some());
    let events = listener.snapshot();
    assert!(events.iter().any(|event| event.contains("app-a")));
    assert!(events.iter().any(|event| event.contains("app-b")));
}

/// Regression: when a ReferenceGrant changes its `spec.from[*].namespace`,
/// the OLD `from` namespace must be re-dispatched so its routes get
/// revalidated. Since `parse()` overwrites the cache before `on_change()`
/// fires, the orphan dispatch must happen inside `parse()` itself.
#[tokio::test]
async fn parse_dispatches_orphaned_from_namespace_on_update() {
    let runtime = Arc::new(ControllerRuntime::default());
    let listener = RecordingListener::new();
    runtime.ref_grant_dispatcher.register_listener(listener.clone());

    let ctx = make_ctx(runtime.clone());
    let handler = ReferenceGrantHandler::new();

    // Initial grant: from=app-a, to(=ns)=backend
    let grant_v1 = build_grant("backend", "g1", &["app-a"]);
    match handler.parse(grant_v1.clone(), &ctx).await {
        ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
        ProcessResult::Skip { .. } => panic!("unexpected skip"),
    }

    // Update grant: from=app-b (replaces app-a), to(=ns)=backend
    let grant_v2 = build_grant("backend", "g1", &["app-b"]);
    match handler.parse(grant_v2.clone(), &ctx).await {
        ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
        ProcessResult::Skip { .. } => panic!("unexpected skip"),
    }

    let events = listener.snapshot();
    assert!(
        events.len() >= 3,
        "expected at least 3 dispatches (initial + orphan-of-update + new-of-update), got {}: {:?}",
        events.len(),
        events
    );

    // The events emitted DURING the update (everything after the initial create)
    // must collectively cover BOTH `app-a` (orphaned) AND `app-b` (new).
    let update_events = &events[1..];
    let dispatched = union_dispatched(update_events);

    assert!(
        dispatched.contains("app-a"),
        "OLD from-namespace `app-a` must be re-dispatched on update; got {:?}",
        dispatched
    );
    assert!(
        dispatched.contains("app-b"),
        "NEW from-namespace `app-b` must be dispatched on update; got {:?}",
        dispatched
    );
    assert!(
        dispatched.contains("backend"),
        "to-namespace `backend` must be dispatched on update; got {:?}",
        dispatched
    );
}

/// Steady-state: if the spec is unchanged across updates, only the
/// on_change dispatch fires — no orphan dispatch is emitted because
/// `old - new` is empty.
#[tokio::test]
async fn parse_does_not_dispatch_orphan_when_namespaces_unchanged() {
    let runtime = Arc::new(ControllerRuntime::default());
    let listener = RecordingListener::new();
    runtime.ref_grant_dispatcher.register_listener(listener.clone());

    let ctx = make_ctx(runtime.clone());
    let handler = ReferenceGrantHandler::new();

    let grant = build_grant("backend", "g1", &["app-a"]);
    match handler.parse(grant.clone(), &ctx).await {
        ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
        ProcessResult::Skip { .. } => panic!("unexpected skip"),
    }
    let events_after_first = listener.snapshot().len();

    // Re-apply the same grant (no namespace change).
    match handler.parse(grant.clone(), &ctx).await {
        ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
        ProcessResult::Skip { .. } => panic!("unexpected skip"),
    }
    let events_after_second = listener.snapshot().len();

    // Second update should add exactly one event (the on_change),
    // NOT two (would mean we also fired an orphan event).
    let added = events_after_second - events_after_first;
    assert_eq!(
        added, 1,
        "expected exactly 1 dispatch on idempotent update (on_change only), got {}",
        added
    );
}

/// First create has no cached old grant — must not synthesize an orphan
/// dispatch from a non-existent old state.
#[tokio::test]
async fn parse_first_create_emits_no_orphan_dispatch() {
    let runtime = Arc::new(ControllerRuntime::default());
    let listener = RecordingListener::new();
    runtime.ref_grant_dispatcher.register_listener(listener.clone());

    let ctx = make_ctx(runtime.clone());
    let handler = ReferenceGrantHandler::new();

    let grant = build_grant("backend", "g1", &["app-a"]);
    match handler.parse(grant.clone(), &ctx).await {
        ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
        ProcessResult::Skip { .. } => panic!("unexpected skip"),
    }

    assert_eq!(listener.snapshot().len(), 1);
}
