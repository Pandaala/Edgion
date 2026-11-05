use super::*;
#[test]
fn policy_ancestor_signature_ignores_unrelated_resources() {
    assert!(signature(Some(&42_u32)).is_none());
}

#[tokio::test]
async fn policy_ancestor_fanout_is_change_gated_and_includes_dropped_targets() {
    use crate::conf_mgr::sync_runtime::resource_processor::{ProcessorHandler, ResourceProcessor, ResourceRef};
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    struct Routes;
    #[async_trait::async_trait]
    impl ProcessorHandler<HTTPRoute> for Routes {}
    let runtime = Arc::new(ControllerRuntime::default());
    let routes = Arc::new(ResourceProcessor::new(
        "HTTPRoute",
        16,
        Arc::new(Routes),
        runtime.clone(),
    ));
    runtime.processor_registry.register(routes.clone());
    for (target, route) in [("old", "old-route"), ("new", "new-route")] {
        runtime.service_ref_mgr.add_ref(
            format!("apps/{target}"),
            ResourceRef::new(
                edgion_resources::ResourceKind::HTTPRoute,
                Some("apps".into()),
                route.into(),
            ),
        );
    }
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 32, runtime);
    let mut old: BackendTLSPolicy=serde_json::from_value(json!({"metadata":{"namespace":"apps","name":"policy"},"spec":{"targetRefs":[{"group":"","kind":"Service","name":"old"}],"validation":{"hostname":"backend.example","wellKnownCACertificates":"System"}}})).unwrap();
    let mut new = old.clone();
    new.spec.target_refs[0].name = "new".into();
    notify_policy_ancestor_change(Some(&old), Some(&new), &ctx).await;
    let queue = routes.workqueue();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(queue.len(), 2);
    let mut keys = BTreeSet::new();
    for _ in 0..2 {
        let item = queue.dequeue().await.unwrap();
        keys.insert(item.key);
    }
    assert_eq!(keys, BTreeSet::from(["apps/new-route".into(), "apps/old-route".into()]));
    old = new.clone();
    new.metadata.resource_version = Some("status-echo".into());
    notify_policy_ancestor_change(Some(&old), Some(&new), &ctx).await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(queue.is_empty());
    new.spec.prepared_ancestor_admission = Some(
        edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Scoped(vec![
            "apps/gateway".into(),
        ]),
    );
    notify_policy_ancestor_change(Some(&old), Some(&new), &ctx).await;
    assert_eq!(queue.dequeue().await.unwrap().key, "apps/new-route");
    notify_policy_ancestor_change(Some(&new), None, &ctx).await;
    assert_eq!(queue.dequeue().await.unwrap().key, "apps/new-route");
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(queue.is_empty());
}

#[tokio::test]
async fn policy_ancestor_diagnostics_follow_sections_and_winners() {
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ProcessorHandler, ResourceProcessor};
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    struct Prepared;
    #[async_trait::async_trait]
    impl ProcessorHandler<BackendTLSPolicy> for Prepared {}
    #[async_trait::async_trait]
    impl ProcessorHandler<Service> for Prepared {}
    let runtime = Arc::new(ControllerRuntime::default());
    let policies = Arc::new(ResourceProcessor::new(
        "BackendTLSPolicy",
        16,
        Arc::new(Prepared),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    policies.set_raw_store(raw.clone());
    runtime.processor_registry.register(policies.clone());
    let services = Arc::new(ResourceProcessor::<Service>::new(
        "Service",
        16,
        Arc::new(Prepared),
        runtime.clone(),
    ));
    runtime.processor_registry.register(services.clone());
    services.save(serde_json::from_value(json!({"metadata":{"namespace":"child","name":"backend"},"spec":{"ports":[{"port":443,"name":"https"},{"port":80,"name":"http"}]}})).unwrap(),None);
    let publish = |name: &str, section: Option<&str>, allowed: bool, valid: bool| {
        let policy:BackendTLSPolicy=serde_json::from_value(json!({"metadata":{"namespace":"child","name":name},"spec":{
            "targetRefs":[{"group":"","kind":"Service","name":"backend","sectionName":section}],
            "preparedPrecedence":{},"resolvedTargetClass":"service","useSystemCa":true,
            "validation":{"hostname":if valid {"backend.example"} else {"invalid host"},"wellKnownCACertificates":"System"},
            "preparedAncestorAdmission":{"mode":"scoped","gateways":if allowed {vec!["root/gateway"]} else {vec![]}}
        }})).unwrap();
        raw.upsert(format!("child/{name}"), Arc::new(policy.clone()));
        policies.save(policy, None);
    };
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 32, runtime);
    let direct:HTTPRoute=serde_json::from_value(json!({"metadata":{"namespace":"root","name":"route"},"spec":{"rules":[{"backendRefs":[{"namespace":"child","name":"backend","port":443},{"namespace":"child","name":"backend","port":80},{"namespace":"child","name":"unrelated","port":443}]}]}})).unwrap();
    let diagnostics = |route: &HTTPRoute| {
        prepare_diagnostics(HttpRouteBackendRefs::from_route(route).unwrap().effective(), &ctx).errors("root/gateway")
    };
    publish("whole", None, true, true);
    publish("a-exact", Some("https"), false, true);
    let messages = diagnostics(&direct);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("backendRefs[0]"));
    assert!(messages[0].contains("child/a-exact"));
    // Invalid exact occupancy must suppress a refused whole-target fallback.
    publish("a-exact", Some("https"), false, false);
    publish("whole", None, false, true);
    let messages = diagnostics(&direct);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("backendRefs[1]"));
    // A valid candidate in the same bucket overrides invalid occupancy, while
    // a newer refused loser cannot block the oldest admitted effective candidate.
    publish("b-valid", Some("https"), true, true);
    publish("z-loser", Some("https"), false, true);
    publish("whole", None, true, true);
    assert!(diagnostics(&direct).is_empty());
}

struct CountingRawStore<T> {
    inner: crate::conf_mgr::sync_runtime::resource_processor::LocalRawStore<T>,
    snapshots: std::sync::atomic::AtomicUsize,
    gets: parking_lot::Mutex<HashMap<String, usize>>,
}
impl<T> CountingRawStore<T> {
    fn new() -> Self {
        Self {
            inner: crate::conf_mgr::sync_runtime::resource_processor::LocalRawStore::new(),
            snapshots: std::sync::atomic::AtomicUsize::new(0),
            gets: parking_lot::Mutex::new(HashMap::new()),
        }
    }
    fn reset(&self) {
        self.snapshots.store(0, std::sync::atomic::Ordering::Relaxed);
        self.gets.lock().clear();
    }
    fn assert_one_scan(&self, population: usize) {
        assert_eq!(self.snapshots.load(std::sync::atomic::Ordering::Relaxed), 1);
        let gets = self.gets.lock();
        assert_eq!(gets.len(), population);
        assert!(gets.values().all(|count| *count == 1));
    }
}
impl<T: Send + Sync + 'static> crate::conf_mgr::sync_runtime::resource_processor::RawStore<T> for CountingRawStore<T> {
    fn get(&self, key: &str) -> Option<Arc<T>> {
        *self.gets.lock().entry(key.into()).or_default() += 1;
        self.inner.get(key)
    }
    fn snapshot(&self) -> Vec<Arc<T>> {
        self.snapshots.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.snapshot()
    }
}
struct DiagnosticPolicies;
#[async_trait::async_trait]
impl crate::conf_mgr::sync_runtime::resource_processor::ProcessorHandler<BackendTLSPolicy> for DiagnosticPolicies {}
#[async_trait::async_trait]
impl crate::conf_mgr::sync_runtime::resource_processor::ProcessorHandler<EdgionBackendTrafficPolicy>
    for DiagnosticPolicies
{
}

#[tokio::test]
async fn policy_ancestor_annotation_scans_once_per_kind_independent_of_parent_and_backend_counts() {
    use crate::conf_mgr::sync_runtime::resource_processor::{
        status_ownership::KubernetesStatusIdentity, ResourceProcessor,
    };
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    for population in [128, 512] {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.set_status_identity(KubernetesStatusIdentity {
            controller_name: "example.com/controller".into(),
        });
        let tls = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
            "BackendTLSPolicy",
            16,
            Arc::new(DiagnosticPolicies),
            runtime.clone(),
        ));
        let traffic = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
            "EdgionBackendTrafficPolicy",
            16,
            Arc::new(DiagnosticPolicies),
            runtime.clone(),
        ));
        let tls_raw = Arc::new(CountingRawStore::new());
        let traffic_raw = Arc::new(CountingRawStore::new());
        tls.set_raw_store(tls_raw.clone());
        traffic.set_raw_store(traffic_raw.clone());
        runtime.processor_registry.register(tls.clone());
        runtime.processor_registry.register(traffic.clone());
        for index in 0..population {
            let name = format!("policy-{index}");
            let target = if index == 0 {
                "backend".into()
            } else {
                format!("unrelated-{index}")
            };
            let tls_policy: BackendTLSPolicy = serde_json::from_value(json!({"metadata":{"namespace":"apps","name":name,"uid":name,"generation":1},"spec":{"targetRefs":[{"group":"","kind":"Service","name":target}],"validation":{"hostname":"backend.example","wellKnownCACertificates":"System"}}})).unwrap();
            let traffic_policy: EdgionBackendTrafficPolicy = serde_json::from_value(json!({"metadata":{"namespace":"apps","name":name,"uid":name,"generation":1},"spec":{"targetRefs":[{"group":"","kind":"Service","name":target},{"group":"","kind":"Service","name":format!("{target}-second")} ]}})).unwrap();
            tls_raw.inner.upsert(format!("apps/{name}"), Arc::new(tls_policy));
            traffic_raw
                .inner
                .upsert(format!("apps/{name}"), Arc::new(traffic_policy));
        }
        let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 32, runtime);
        for (parents, backends) in [(1, 2), (8, 32)] {
            tls_raw.reset();
            traffic_raw.reset();
            let mut route: HTTPRoute = serde_json::from_value(json!({"metadata":{"namespace":"apps","name":"route"},"spec":{"rules":[{"backendRefs":(0..backends).map(|i| json!({"name":if i%2 == 0 {"backend"} else {"backend-second"},"port":if i%4 < 2 {443} else {80}})).collect::<Vec<_>>()}]},"status":{"parents":(0..parents).map(|i| json!({"parentRef":{"name":format!("gateway-{i}")},"controllerName":"example.com/controller","conditions":[]})).collect::<Vec<_>>()}})).unwrap();
            annotate(&mut route, "example.com/controller", &ctx);
            tls_raw.assert_one_scan(population);
            traffic_raw.assert_one_scan(population);
            for parent in &route.status.as_ref().unwrap().parents {
                assert!(parent
                    .conditions
                    .iter()
                    .any(|condition| condition.message.contains("pending publication")));
            }
            let prepared = prepare_diagnostics(HttpRouteBackendRefs::from_route(&route).unwrap().effective(), &ctx);
            assert_eq!(prepared.decisions.len(), if backends == 2 { 2 } else { 4 });
            assert_eq!(prepared.backends.len(), backends);
        }
        // Publication is observed by a fresh invocation, without a persistent inventory.
        use crate::conf_mgr::sync_runtime::resource_processor::RawStore;
        tls.save(tls_raw.inner.get("apps/policy-0").unwrap().as_ref().clone(), None);
        traffic.save(traffic_raw.inner.get("apps/policy-0").unwrap().as_ref().clone(), None);
        let route: HTTPRoute = serde_json::from_value(json!({"metadata":{"namespace":"apps","name":"route"},"spec":{"rules":[{"backendRefs":[{"name":"backend","port":443},{"name":"backend-second","port":80}]}]}})).unwrap();
        let prepared = prepare_diagnostics(HttpRouteBackendRefs::from_route(&route).unwrap().effective(), &ctx);
        assert!(prepared.decisions.values().all(|decision| !decision.pending));
    }
}

#[tokio::test]
async fn policy_ancestor_inventory_retains_indexed_absence_and_moved_uid_generation_pending() {
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor, ResourceRef};
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
        "BackendTLSPolicy",
        16,
        Arc::new(DiagnosticPolicies),
        runtime.clone(),
    ));
    let raw = Arc::new(LocalRawStore::new());
    processor.set_raw_store(raw.clone());
    runtime.processor_registry.register(processor.clone());
    runtime.service_ref_mgr.add_ref(
        "apps/old".into(),
        ResourceRef::new(
            edgion_resources::ResourceKind::BackendTLSPolicy,
            Some("apps".into()),
            "policy".into(),
        ),
    );
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 32, runtime);
    let old: BackendTLSPolicy = serde_json::from_value(json!({"metadata":{"namespace":"apps","name":"policy","uid":"uid-1","generation":1},"spec":{"targetRefs":[{"group":"","kind":"Service","name":"old"}],"validation":{"hostname":"backend.example","wellKnownCACertificates":"System"}}})).unwrap();
    let old_key = PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: "apps".into(),
        name: "old".into(),
    };
    let new_key = PolicyTargetKey {
        name: "new".into(),
        ..old_key.clone()
    };
    let targets = HashSet::from([old_key.clone(), new_key.clone()]);
    let inventory = || {
        policies::<BackendTLSPolicy>(
            "BackendTLSPolicy",
            &targets,
            |policy| {
                BackendTlsPolicyTargets::from_policy(policy)
                    .fields()
                    .filter_map(|field| field.canonical_key().ok())
                    .collect()
            },
            &ctx,
        )
    };
    processor.save(old.clone(), None);
    let buckets = inventory();
    assert_eq!(buckets[&old_key].prepared.len(), 1);
    assert!(!buckets[&old_key].pending);
    for (uid, generation) in [("uid-1", 2), ("uid-2", 1)] {
        let mut moved = old.clone();
        moved.spec.target_refs[0].name = "new".into();
        moved.metadata.uid = Some(uid.into());
        moved.metadata.generation = Some(generation);
        raw.upsert("apps/policy".into(), Arc::new(moved.clone()));
        processor.save(old.clone(), None);
        let buckets = inventory();
        assert!(buckets[&old_key].pending && buckets[&new_key].pending);
        assert!(buckets[&old_key].prepared.is_empty() && buckets[&new_key].prepared.is_empty());
        processor.save(moved, None);
        let buckets = inventory();
        assert!(buckets[&old_key].pending);
        assert!(!buckets[&new_key].pending);
        assert_eq!(buckets[&new_key].prepared.len(), 1);
    }
}
