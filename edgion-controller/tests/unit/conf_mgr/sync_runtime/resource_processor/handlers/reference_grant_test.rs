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

fn backend_only() -> HashSet<String> {
    HashSet::from(["backend".to_string()])
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
    // Create, delete and recreate each revalidate the grant namespace only.
    assert_eq!(listener.snapshot(), vec![backend_only(); 3]);
}

/// Dependents are keyed by target namespace, so a `from` change revalidates
/// only the grant namespace: owners in both the old and new `from` namespaces
/// are registered there. Dispatching `from` namespaces would requeue unrelated
/// resources that merely reference into those namespaces.
#[tokio::test]
async fn from_namespace_update_dispatches_only_grant_namespace() {
    let runtime = Arc::new(ControllerRuntime::default());
    let listener = RecordingListener::new();
    runtime.ref_grant_dispatcher.register_listener(listener.clone());

    let ctx = make_ctx(runtime.clone());
    let handler = ReferenceGrantHandler::new();

    for from in ["app-a", "app-b", "app-b"] {
        let grant = build_grant("backend", "g1", &[from]);
        match handler.parse(grant, &ctx).await {
            ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
            ProcessResult::Skip { .. } => panic!("unexpected skip"),
        }
    }

    assert_eq!(listener.snapshot(), vec![backend_only(); 3]);
}

#[tokio::test]
async fn delete_dispatches_only_grant_namespace() {
    let runtime = Arc::new(ControllerRuntime::default());
    let listener = RecordingListener::new();
    runtime.ref_grant_dispatcher.register_listener(listener.clone());

    let ctx = make_ctx(runtime.clone());
    let handler = ReferenceGrantHandler::new();
    let grant = build_grant("backend", "g1", &["app-a", "app-b"]);
    match handler.parse(grant.clone(), &ctx).await {
        ProcessResult::Continue(rg) => handler.on_change(&rg, &ctx).await,
        ProcessResult::Skip { .. } => panic!("unexpected skip"),
    }
    handler.on_delete(&grant, &ctx).await;

    assert!(runtime
        .ref_grant_store
        .get(&ReferenceGrantHandler::build_key(&grant))
        .is_none());
    assert_eq!(listener.snapshot(), vec![backend_only(); 2]);
}
