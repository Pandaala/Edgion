//! Ownership/publication regressions through one real serial workqueue per kind.
//! No API server, status writer, or duplicate resource worker is started.

use super::{GatewayClassHandler, GatewayClassOwnership, GatewayHandler};
use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::KubernetesStatusIdentity;
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, LocalRawStore, ProcessResult, ProcessorHandler, ProcessorObj, RawStore, ResourceProcessor,
    WorkItemResult,
};
use crate::conf_mgr::sync_runtime::workqueue::{WorkItem, Workqueue};
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::{Gateway, GatewayClass};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

const CLASS: &str = "selected";
const KEY: &str = "apps/gateway";
const OWNER: &str = "example.com/owner";

fn ownership(owner: &str) -> GatewayClassOwnership {
    GatewayClassOwnership::new(owner.to_string())
}

fn class(owner: &str, name: &str, uid: &str) -> GatewayClass {
    serde_json::from_value(serde_json::json!({
        "metadata": {"name": name, "uid": uid, "generation": 1},
        "spec": {"controllerName": owner}
    }))
    .unwrap()
}

fn gateway() -> Gateway {
    serde_json::from_value(serde_json::json!({
        "metadata": {"namespace": "apps", "name": "gateway", "uid": "gateway-uid", "generation": 1},
        "spec": {"gatewayClassName": CLASS, "listeners": [{
            "name": "https", "port": 443, "protocol": "HTTPS",
            "tls": {"mode": "Terminate", "certificateRefs": [{"name": "cert"}]}
        }]}
    }))
    .unwrap()
}

async fn dequeue(queue: &Workqueue) -> WorkItem {
    tokio::time::timeout(Duration::from_secs(5), queue.dequeue())
        .await
        .expect("ownership dependency was not queued")
        .expect("queue closed")
}

/// Test-only suspension after the real parse has begun, to exercise class
/// unpublication while the Gateway's existing single worker is in flight.
struct PausableGateway {
    inner: GatewayHandler,
    hold: AtomicBool,
    entered: Notify,
    resume: Notify,
}

#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for PausableGateway {
    fn filter(&self, value: &Gateway, ctx: &HandlerContext) -> bool {
        self.inner.filter(value, ctx)
    }
    fn validate(&self, value: &Gateway, ctx: &HandlerContext) -> Vec<String> {
        self.inner.validate(value, ctx)
    }
    fn preparse(&self, value: &mut Gateway, ctx: &HandlerContext) -> Vec<String> {
        self.inner.preparse(value, ctx)
    }
    async fn parse(&self, value: Gateway, ctx: &HandlerContext) -> ProcessResult<Gateway> {
        let result = self.inner.parse(value, ctx).await;
        if self.hold.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        result
    }
    fn update_status(&self, value: &mut Gateway, ctx: &HandlerContext, errors: &[String]) {
        self.inner.update_status(value, ctx, errors);
    }
    fn finalize_transport(&self, value: &mut Gateway, ctx: &HandlerContext) {
        self.inner.finalize_transport(value, ctx);
    }
    async fn on_delete(&self, value: &Gateway, ctx: &HandlerContext) {
        self.inner.on_delete(value, ctx).await;
    }
    async fn after_publish(&self, value: &Gateway, ctx: &HandlerContext) {
        self.inner.after_publish(value, ctx).await;
    }
    async fn after_unpublish(&self, value: &Gateway, ctx: &HandlerContext) {
        self.inner.after_unpublish(value, ctx).await;
    }
}

struct Fixture {
    runtime: Arc<ControllerRuntime>,
    classes: Arc<ResourceProcessor<GatewayClass>>,
    gateways: Arc<ResourceProcessor<Gateway>>,
    class_raw: Arc<LocalRawStore<GatewayClass>>,
    gateway_raw: Arc<LocalRawStore<Gateway>>,
    handler: Arc<PausableGateway>,
}

impl Fixture {
    fn new(owner: &str) -> Self {
        let runtime = Arc::new(ControllerRuntime::default());
        let classes = Arc::new(ResourceProcessor::new(
            "GatewayClass",
            16,
            Arc::new(GatewayClassHandler::new(ownership(owner))),
            runtime.clone(),
        ));
        let handler = Arc::new(PausableGateway {
            inner: GatewayHandler::new(Some(ownership(owner)), Some("192.0.2.1".into())),
            hold: AtomicBool::new(false),
            entered: Notify::new(),
            resume: Notify::new(),
        });
        let gateways = Arc::new(ResourceProcessor::new("Gateway", 16, handler.clone(), runtime.clone()));
        gateways.set_namespace_filter(vec!["apps".into()]);
        let class_raw = Arc::new(LocalRawStore::new());
        let gateway_raw = Arc::new(LocalRawStore::new());
        classes.set_raw_store(class_raw.clone());
        gateways.set_raw_store(gateway_raw.clone());
        runtime.processor_registry.register(classes.clone());
        runtime.processor_registry.register(gateways.clone());
        gateway_raw.upsert(KEY.into(), Arc::new(gateway()));
        Self {
            runtime,
            classes,
            gateways,
            class_raw,
            gateway_raw,
            handler,
        }
    }

    async fn class_event(&self, name: &str, value: Option<GatewayClass>) -> WorkItemResult<GatewayClass> {
        if let Some(value) = value {
            self.class_raw.upsert(name.into(), Arc::new(value));
        } else {
            self.class_raw.remove(name);
        }
        self.classes.on_event_key(name.into()).await;
        let queue = self.classes.workqueue();
        let item = dequeue(&queue).await;
        let result = self
            .classes
            .process_work_item(
                &item.key,
                self.class_raw.get(&item.key).map(|value| (*value).clone()),
                None,
                item.trigger_chain,
            )
            .await;
        queue.done(&item.key);
        result
    }

    async fn gateway_turn(&self, event: bool) -> WorkItemResult<Gateway> {
        if event {
            self.gateways.on_event_key(KEY.into()).await;
        }
        let queue = self.gateways.workqueue();
        let item = dequeue(&queue).await;
        let result = self
            .gateways
            .process_work_item(
                &item.key,
                self.gateway_raw.get(&item.key).map(|value| (*value).clone()),
                None,
                item.trigger_chain,
            )
            .await;
        queue.done(&item.key);
        result
    }

    fn assert_published(&self, present: bool) {
        assert_eq!(self.gateways.get(KEY).is_some(), present);
        assert_eq!(
            !self.runtime.secret_ref_manager.get_refs("apps/cert").is_empty(),
            present
        );
        assert_eq!(self.runtime.listener_port_manager.stats().1, usize::from(present));
        if present {
            assert!(self.gateways.get(KEY).unwrap().spec.resolved_attachment_proof.is_some());
        }
    }
}

#[tokio::test]
async fn missing_class_recovers_and_delete_recreate_revokes_side_effects() {
    let f = Fixture::new(OWNER);
    assert!(matches!(f.gateway_turn(true).await, WorkItemResult::Skipped));
    f.assert_published(false);
    f.class_event(CLASS, Some(class(OWNER, CLASS, "first"))).await;
    assert!(matches!(f.gateway_turn(false).await, WorkItemResult::Processed { .. }));
    f.assert_published(true);
    let old_proof = f.gateways.get(KEY).unwrap().spec.resolved_attachment_proof;
    f.class_event(CLASS, None).await;
    assert!(matches!(f.gateway_turn(false).await, WorkItemResult::Deleted { .. }));
    f.assert_published(false);
    f.class_event(CLASS, Some(class(OWNER, CLASS, "second"))).await;
    assert!(matches!(f.gateway_turn(false).await, WorkItemResult::Processed { .. }));
    f.assert_published(true);
    assert_ne!(f.gateways.get(KEY).unwrap().spec.resolved_attachment_proof, old_proof);
}

#[tokio::test]
async fn independent_deployments_only_publish_matching_owners() {
    for (owner, allowed) in [(OWNER, true), ("example.com/other", false)] {
        let f = Fixture::new(owner);
        let result = f.class_event(CLASS, Some(class(OWNER, CLASS, "uid"))).await;
        assert_eq!(matches!(result, WorkItemResult::Processed { .. }), owner == OWNER);
        assert_eq!(f.classes.get(CLASS).is_some(), allowed);
        // Use an event for denied deployments; they have no class publication requeue.
        let result = f.gateway_turn(!allowed).await;
        assert_eq!(matches!(result, WorkItemResult::Processed { .. }), allowed);
        f.assert_published(allowed);
        // A namespace-isolated deployment must not publish the same Gateway
        // merely because its class and controller identity happen to match.
        if allowed {
            f.gateways.set_namespace_filter(vec!["other-namespace".into()]);
            assert!(matches!(f.gateway_turn(true).await, WorkItemResult::Deleted { .. }));
            f.assert_published(false);
        }
    }
}

#[tokio::test]
async fn raw_owned_class_never_published_then_coalesced_foreign_never_adopts_gateway() {
    let f = Fixture::new(OWNER);
    f.class_raw.upsert(CLASS.into(), Arc::new(class(OWNER, CLASS, "brief")));
    assert!(matches!(f.gateway_turn(true).await, WorkItemResult::Skipped));
    assert!(matches!(
        // Delete and recreate coalesce: the old owned incarnation was never published.
        f.class_event(CLASS, Some(class("example.com/foreign", CLASS, "recreated-foreign")))
            .await,
        WorkItemResult::Skipped
    ));
    assert!(matches!(f.gateway_turn(true).await, WorkItemResult::Skipped));
    f.assert_published(false);
}

#[tokio::test]
async fn class_incarnation_not_generation_or_accepted_controls_gateway_ownership() {
    let f = Fixture::new(OWNER);
    f.class_event(CLASS, Some(class(OWNER, CLASS, "first"))).await;
    f.gateway_turn(false).await;
    let mut updated = class(OWNER, CLASS, "first");
    updated.metadata.generation = Some(2);
    // Invalid parameters make Accepted=False but do not transfer ownership.
    updated.spec.parameters_ref = Some(
        serde_json::from_value(serde_json::json!({
            "group": "wrong.example", "kind": "Wrong", "name": "missing"
        }))
        .unwrap(),
    );
    f.class_raw.upsert(CLASS.into(), Arc::new(updated.clone()));
    assert!(matches!(f.gateway_turn(true).await, WorkItemResult::Processed { .. }));
    f.class_event(CLASS, Some(updated)).await;
    assert!(f
        .classes
        .get(CLASS)
        .unwrap()
        .status
        .unwrap()
        .conditions
        .iter()
        .any(|c| c.type_ == "Accepted" && c.status == "False"));
    assert!(matches!(f.gateway_turn(false).await, WorkItemResult::Processed { .. }));
    f.class_raw
        .upsert(CLASS.into(), Arc::new(class(OWNER, CLASS, "replacement")));
    assert!(matches!(f.gateway_turn(true).await, WorkItemResult::Deleted { .. }));
    f.assert_published(false);
    f.class_event(CLASS, Some(class(OWNER, CLASS, "replacement"))).await;
    f.gateway_turn(false).await;
    f.assert_published(true);
    f.class_event(CLASS, Some(class("example.com/foreign", CLASS, "recreated-foreign")))
        .await;
    assert!(matches!(f.gateway_turn(false).await, WorkItemResult::Deleted { .. }));
    f.assert_published(false);
}

#[tokio::test]
async fn unavailable_class_identity_fails_closed_but_local_sources_remain_unscoped() {
    let f = Fixture::new(OWNER);
    for uid in [Some(""), None] {
        let mut value = class(OWNER, CLASS, "placeholder");
        value.metadata.uid = uid.map(str::to_string);
        f.class_event(CLASS, Some(value)).await;
        assert!(matches!(f.gateway_turn(false).await, WorkItemResult::Skipped));
    }
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime);
    assert!(
        !f.handler.filter(&gateway(), &ctx),
        "no_watch_kinds must not authorize ownership"
    );
    assert!(
        GatewayHandler::new(None, None).filter(&gateway(), &ctx),
        "FS/etcd keep their unscoped behavior"
    );
}

#[tokio::test]
async fn unpublication_during_gateway_processing_requeues_the_same_worker_for_cleanup() {
    let f = Arc::new(Fixture::new(OWNER));
    f.class_event(CLASS, Some(class(OWNER, CLASS, "first"))).await;
    f.handler.hold.store(true, Ordering::SeqCst);
    let worker = {
        let f = f.clone();
        tokio::spawn(async move { f.gateway_turn(false).await })
    };
    tokio::time::timeout(Duration::from_secs(5), f.handler.entered.notified())
        .await
        .unwrap();
    // The Gateway passed filter while the class existed. The class worker
    // independently withdraws publication before the Gateway publishes.
    f.class_event(CLASS, None).await;
    f.handler.resume.notify_one();
    assert!(matches!(worker.await.unwrap(), WorkItemResult::Processed { .. }));
    assert!(matches!(f.gateway_turn(false).await, WorkItemResult::Deleted { .. }));
    f.assert_published(false);
}

#[tokio::test]
async fn repeated_class_and_gateway_status_echoes_converge_without_status_changes() {
    let f = Fixture::new(OWNER);
    let mut current_class = class(OWNER, CLASS, "uid");
    for round in 0..6 {
        let result = f.class_event(CLASS, Some(current_class)).await;
        let WorkItemResult::Processed {
            obj, status_changed, ..
        } = result
        else {
            panic!("owned class rejected");
        };
        assert_eq!(status_changed, round == 0);
        current_class = obj;
        let result = f.gateway_turn(false).await;
        let WorkItemResult::Processed {
            obj, status_changed, ..
        } = result
        else {
            panic!("owned Gateway rejected");
        };
        assert_eq!(status_changed, round == 0);
        f.gateway_raw.upsert(KEY.into(), Arc::new(obj));
    }
    f.assert_published(true);
    // Each explicit class event produces exactly one Gateway dependency item.
    // No delayed item is left hidden behind an empty ready queue.
    assert!(f.classes.workqueue().is_empty());
    assert!(f.gateways.workqueue().is_empty());
    assert_eq!(f.classes.workqueue().metrics().get_adds_total(), 6);
    assert_eq!(f.classes.workqueue().metrics().delayed_total.load(Ordering::Relaxed), 0);
    assert_eq!(f.gateways.workqueue().metrics().get_adds_total(), 6);
    assert_eq!(
        f.gateways.workqueue().metrics().delayed_total.load(Ordering::Relaxed),
        6
    );
}

#[tokio::test]
async fn every_matching_class_validates_parameters_and_preserves_foreign_conditions() {
    let f = Fixture::new(OWNER);
    f.runtime.set_status_identity(KubernetesStatusIdentity {
        controller_name: OWNER.into(),
    });
    let mut value = class(OWNER, "unassigned", "uid-unassigned");
    value.spec.parameters_ref = Some(
        serde_json::from_value(serde_json::json!({
            "group": "wrong.example", "kind": "Wrong", "name": "missing"
        }))
        .unwrap(),
    );
    let foreign = serde_json::json!({
        "type": "example.com/External", "status": "True", "reason": "External",
        "message": "external condition", "observedGeneration": 1,
        "lastTransitionTime": "2000-01-01T00:00:00Z"
    });
    value.status = Some(
        serde_json::from_value(serde_json::json!({
            "conditions": [foreign.clone(), {
                "type": "ResolvedRefs", "status": "False", "reason": "InvalidParameters",
                "message": "old diagnostic", "lastTransitionTime": "2000-01-01T00:00:00Z"
            }, {
                "type": "SupportedVersion", "status": "True", "reason": "SupportedVersion",
                "message": "old diagnostic", "lastTransitionTime": "2000-01-01T00:00:00Z"
            }],
            "supportedFeatures": [{"name": "Gateway"}]
        }))
        .unwrap(),
    );
    for round in 0..4 {
        let WorkItemResult::Processed {
            obj,
            status_changed,
            persisted_status,
        } = f.class_event("unassigned", Some(value)).await
        else {
            panic!("matching class needs a status result");
        };
        assert_eq!(status_changed, round == 0);
        let status = persisted_status.unwrap();
        assert_eq!(status["conditions"][0], foreign);
        assert_eq!(status["conditions"].as_array().unwrap().len(), 4);
        let accepted = status["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["type"] == "Accepted")
            .unwrap();
        assert_eq!(accepted["status"], "False");
        assert_eq!(accepted["reason"], "InvalidParameters");
        let supported_version = status["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|condition| condition["type"] == "SupportedVersion")
            .unwrap();
        assert_eq!(supported_version["status"], "Unknown");
        assert_eq!(supported_version["reason"], "CompatibilityUnknown");
        assert_eq!(
            supported_version["observedGeneration"],
            serde_json::json!(obj.metadata.generation)
        );
        assert!(!status["supportedFeatures"].as_array().unwrap().is_empty());
        assert!(f.classes.get("unassigned").is_some());
        assert!(f.gateways.workqueue().is_empty(), "unrelated Gateways are not requeued");
        value = obj;
    }
    // A features-only drift is semantic too; omitted/null is not an explicit clear.
    value.status.as_mut().unwrap().supported_features = None;
    assert!(matches!(
        f.class_event("unassigned", Some(value)).await,
        WorkItemResult::Processed {
            status_changed: true,
            ..
        }
    ));
    f.assert_published(false);
}

#[tokio::test]
async fn matching_class_init_and_leader_recovery_use_raw_keys() {
    let f = Fixture::new(OWNER);
    let value = class(OWNER, "unassigned", "old");
    f.class_raw.upsert("unassigned".into(), Arc::new(value.clone()));
    assert!(matches!(
        f.classes.on_init_apply(value.clone(), None).await,
        WorkItemResult::Processed {
            status_changed: true,
            ..
        }
    ));
    assert!(f.classes.get("unassigned").is_some());

    // Publishing the class enqueues raw Gateway dependents.
    let mut dependent = gateway();
    dependent.spec.gateway_class_name = "unassigned".into();
    f.gateway_raw.upsert(KEY.into(), Arc::new(dependent));
    assert!(matches!(
        f.class_event("unassigned", Some(value)).await,
        WorkItemResult::Processed { .. }
    ));
    assert!(f.classes.get("unassigned").is_some());
    assert!(matches!(f.gateway_turn(false).await, WorkItemResult::Processed { .. }));
    f.assert_published(true);

    // Delete/recreate before a leader change. The unpublished new incarnation
    // must be visited even though ServerCache has no key for it.
    f.class_event("unassigned", None).await;
    f.class_raw
        .upsert("unassigned".into(), Arc::new(class(OWNER, "unassigned", "new")));
    assert_eq!(f.classes.requeue_all_keys().await, 1);
    let queue = f.classes.workqueue();
    let item = dequeue(&queue).await;
    let result = f
        .classes
        .process_work_item(
            &item.key,
            f.class_raw.get(&item.key).map(|value| (*value).clone()),
            None,
            item.trigger_chain,
        )
        .await;
    queue.done(&item.key);
    let WorkItemResult::Processed {
        obj, status_changed, ..
    } = result
    else {
        panic!("leader must recover matching classes");
    };
    assert_eq!(obj.metadata.uid.as_deref(), Some("new"));
    assert!(status_changed);
    assert!(f.classes.get("unassigned").is_some());
}

#[tokio::test]
async fn distinct_controller_identities_converge_with_multiple_classes() {
    let a = Fixture::new("example.com/a");
    let b = Fixture::new("example.com/b");
    for (owner, name, accepted) in [
        ("example.com/a", "a", "True"),
        ("example.com/b", "b", "True"),
        ("example.com/a", "unassigned", "True"),
    ] {
        let mut value = class(owner, name, name);
        for round in 0..4 {
            for (identity, deployment) in [("example.com/a", &a), ("example.com/b", &b)] {
                match deployment.class_event(name, Some(value.clone())).await {
                    WorkItemResult::Processed {
                        obj,
                        status_changed,
                        persisted_status,
                    } => {
                        assert_eq!(identity, owner);
                        assert_eq!(status_changed, round == 0);
                        let status = persisted_status.unwrap();
                        let condition = status["conditions"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|condition| condition["type"] == "Accepted")
                            .unwrap();
                        assert_eq!(condition["status"], accepted);
                        value = obj;
                    }
                    WorkItemResult::Skipped => assert_ne!(identity, owner),
                    other => panic!("unexpected result: {other:?}"),
                }
            }
        }
    }
    assert!(a.classes.get("a").is_some());
    assert!(b.classes.get("b").is_some());
    assert!(a.classes.get("b").is_none());
    assert!(b.classes.get("a").is_none());
    assert!(a.classes.get("unassigned").is_some());
    assert!(b.classes.get("unassigned").is_none());
}

#[tokio::test]
async fn one_selected_class_still_publishes_multiple_gateways() {
    let f = Fixture::new(OWNER);
    let mut second = gateway();
    second.metadata.name = Some("second".into());
    second.metadata.uid = Some("second-uid".into());
    f.gateway_raw.upsert("apps/second".into(), Arc::new(second));
    f.class_event(CLASS, Some(class(OWNER, CLASS, "class-uid"))).await;
    f.gateway_turn(false).await;
    f.gateway_turn(false).await;
    for key in [KEY, "apps/second"] {
        assert!(f.gateways.get(key).unwrap().spec.resolved_attachment_proof.is_some());
    }
    assert!(f.gateways.workqueue().is_empty());
}

#[tokio::test]
async fn two_matching_classes_publish_distinct_parameters_and_gateway_transfer_revokes_ownership() {
    use super::EdgionGatewayConfigHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::gateway_is_owned;
    use edgion_resources::EdgionGatewayConfig;

    let f = Fixture::new(OWNER);
    f.runtime.set_status_identity(KubernetesStatusIdentity {
        controller_name: OWNER.into(),
    });
    let configs = Arc::new(ResourceProcessor::<EdgionGatewayConfig>::new(
        "EdgionGatewayConfig",
        16,
        Arc::new(EdgionGatewayConfigHandler::new()),
        f.runtime.clone(),
    ));
    let config_raw = Arc::new(LocalRawStore::new());
    configs.set_raw_store(config_raw.clone());
    f.runtime.processor_registry.register(configs);
    let mut second = gateway();
    second.metadata.name = Some("second".into());
    second.metadata.uid = Some("second-uid".into());
    second.spec.gateway_class_name = "second-class".into();
    second.spec.listeners.as_mut().unwrap()[0].port = 8443;
    f.gateway_raw.upsert("apps/second".into(), Arc::new(second));
    for (name, config_name) in [(CLASS, "config-a"), ("second-class", "config-b")] {
        let config = serde_json::from_value(serde_json::json!({"metadata":{"name": config_name}, "spec":{}})).unwrap();
        config_raw.upsert(config_name.into(), Arc::new(config));
        let mut value = class(OWNER, name, name);
        value.spec.parameters_ref = Some(
            serde_json::from_value(serde_json::json!({
                "group":"edgion.io", "kind":"EdgionGatewayConfig", "name":config_name
            }))
            .unwrap(),
        );
        f.class_event(name, Some(value)).await;
        let published = f.classes.get(name).unwrap();
        assert_eq!(published.spec.parameters_ref.as_ref().unwrap().name, config_name);
        assert!(published
            .status
            .unwrap()
            .conditions
            .iter()
            .any(|c| c.type_ == "Accepted" && c.status == "True"));
    }
    f.gateway_turn(false).await;
    f.gateway_turn(false).await;
    for key in [KEY, "apps/second"] {
        assert!(gateway_is_owned(&f.gateways.get(key).unwrap(), &f.runtime));
    }
    assert!(matches!(
        f.class_event("foreign", Some(class("example/foreign", "foreign", "foreign")))
            .await,
        WorkItemResult::Skipped
    ));
    let mut transferred = gateway();
    transferred.spec.gateway_class_name = "foreign".into();
    f.gateway_raw.upsert(KEY.into(), Arc::new(transferred.clone()));
    assert!(!gateway_is_owned(&transferred, &f.runtime));
    assert!(matches!(f.gateway_turn(true).await, WorkItemResult::Deleted { .. }));
    assert!(f.gateways.get(KEY).is_none());
    assert!(f.gateways.get("apps/second").is_some());
}
