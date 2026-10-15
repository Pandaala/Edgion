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
async fn policy_ancestor_diagnostics_follow_sections_winners_and_delegated_terminal_rules() {
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
        errors(
            HttpRouteBackendRefs::from_route(route).unwrap().effective(),
            "root/gateway",
            &ctx,
        )
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
    publish("b-valid", Some("https"), false, true);
    let delegated:HTTPRoute=serde_json::from_value(json!({"metadata":{"namespace":"root","name":"delegating"},"spec":{
        "rules":[{"backendRefs":[{"kind":"HTTPRoute","group":"gateway.networking.k8s.io","name":"child-route"}]}],
        "resolvedRules":[{"resolvedTerminalRouteUid":"child-uid","resolvedTerminalRuleIdentity":"index:0","backendRefs":[{"namespace":"child","name":"unrelated","port":443},{"namespace":"child","name":"backend","port":443}]}]
    }})).unwrap();
    let messages = diagnostics(&delegated);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("backendRefs[1]"));
    assert!(messages[0].contains("child/b-valid"));
}
