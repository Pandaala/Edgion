//! Locks the coalescing guarantee: a delete->recreate that changes parentRefs
//! must requeue the dropped parent Gateway even though handler.on_delete never
//! ran (the workqueue coalesces by key).

use super::HttpRouteHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessorHandler, ResourceProcessor};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::{Gateway, HTTPRoute};
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::http_route::HTTPRouteSpec;
use kube::api::ObjectMeta;
use std::sync::Arc;

struct GwNoop;
#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for GwNoop {}

fn route_with_parent(gw_name: &str) -> HTTPRoute {
    HTTPRoute {
        metadata: ObjectMeta {
            name: Some("r1".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: HTTPRouteSpec {
            current_status: None,
            resolved_status_controller: None,
            parent_refs: Some(vec![ParentReference {
                group: None,
                kind: None,
                namespace: Some("default".to_string()),
                name: gw_name.to_string(),
                section_name: None,
                port: None,
            }]),
            hostnames: None,
            resolved_hostnames: None,
            resolved_listeners: None,
            invalid_rule_indices: None,
            rules: None,
            resolved_rules: None,
            delegation_issues: Vec::new(),
            use_default_gateways: None,
        },
        status: None,
    }
}

#[tokio::test]
async fn coalesced_recreate_requeues_dropped_parent_gateway() {
    let runtime = Arc::new(ControllerRuntime::default());

    // Register a Gateway processor so ctx.requeue("Gateway", ...) has a queue
    // we can observe.
    let gw_processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        100,
        Arc::new(GwNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(gw_processor.clone());

    let handler = HttpRouteHandler::new("edgion.io/gateway-controller".to_string());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime.clone());

    // Generation 1: route attached to gw-a.
    handler.on_change(&route_with_parent("gw-a"), &ctx).await;

    // Deterministic barrier: gen-1 requeues gw-a itself (with the 100ms
    // cross-resource delay). We MUST consume that requeue before generation 2,
    // otherwise a gen-1 gw-a item lingering in the queue could satisfy the
    // generation-2 assertion even if the dropped-parent logic were reverted — a
    // false PASS masking a regression. A fixed quiet-window drain cannot prove
    // the delayed requeue landed; instead, block until gw-a is dequeued at least
    // once (proof gen-1's requeue promoted and was consumed), then drain any
    // residue to quiet. dequeue removes the key from the queue and clears its
    // `scheduled` entry at promotion, so gen-2's gw-a requeue is a fresh
    // schedule, never coalesced with gen-1's.
    let gen1_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut gen1_saw_gw_a = false;
    while std::time::Instant::now() < gen1_deadline && !gen1_saw_gw_a {
        if let Ok(Some(item)) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            gw_processor.workqueue().dequeue(),
        )
        .await
        {
            if item.key == "default/gw-a" {
                gen1_saw_gw_a = true;
            }
            gw_processor.workqueue().done(&item.key);
        }
    }
    assert!(
        gen1_saw_gw_a,
        "test precondition failed: generation 1 never requeued gw-a"
    );
    // Drain any residue to quiet so the queue is empty before generation 2.
    while tokio::time::timeout(
        std::time::Duration::from_millis(500),
        gw_processor.workqueue().dequeue(),
    )
    .await
    .is_ok()
    {}

    // Coalesced delete->recreate: worker only ever sees generation 2 (gw-b).
    // on_delete never runs. gw-a must still be requeued (dropped parent).
    handler.on_change(&route_with_parent("gw-b"), &ctx).await;

    let mut seen = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && !seen.contains("default/gw-a") {
        if let Ok(Some(item)) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            gw_processor.workqueue().dequeue(),
        )
        .await
        {
            seen.insert(item.key.clone());
            gw_processor.workqueue().done(&item.key);
        }
    }
    assert!(
        seen.contains("default/gw-a"),
        "dropped parent gw-a was not requeued; seen: {:?}",
        seen
    );
}

#[tokio::test]
async fn coalesced_service_ai_retarget_requeues_dropped_policies_and_restores_mirror_role() {
    use crate::conf_mgr::sync_runtime::resource_processor::service_ref::{
        forwarding_service_dependencies, mirror_service_key,
    };
    use crate::conf_mgr::sync_runtime::resource_processor::{ProcessResult, ResourceRef};
    use edgion_resources::{EdgionBackendTrafficPolicy, ResourceKind};
    struct PolicyNoop;
    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionBackendTrafficPolicy> for PolicyNoop {}
    let runtime = Arc::new(ControllerRuntime::default());
    let policies = Arc::new(ResourceProcessor::<EdgionBackendTrafficPolicy>::new(
        "EdgionBackendTrafficPolicy",
        16,
        Arc::new(PolicyNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(policies.clone());
    let service_policy = ResourceRef::new(
        ResourceKind::EdgionBackendTrafficPolicy,
        Some("default".into()),
        "service-policy".into(),
    );
    let ai_policy = ResourceRef::new(
        ResourceKind::EdgionBackendTrafficPolicy,
        Some("default".into()),
        "ai-policy".into(),
    );
    runtime
        .service_ref_mgr
        .add_ref("default/service".into(), service_policy);
    runtime
        .backend_ref_mgr
        .add_ref("edgion.io/EdgionBackend/default/models".into(), ai_policy);
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
    let handler = HttpRouteHandler::default();
    let owner = ResourceRef::new(ResourceKind::HTTPRoute, Some("default".into()), "route".into());
    let route = |ai: bool| -> HTTPRoute {
        serde_json::from_value(serde_json::json!({
            "metadata":{"name":"route","namespace":"default"},
            "spec":{"rules":[{"backendRefs":[if ai {
                serde_json::json!({"group":"edgion.io","kind":"EdgionBackend","name":"models"})
            }else{serde_json::json!({"name":"service","port":80})}],
                "filters":[{"type":"RequestMirror","requestMirror":{"backendRef":{"name":"mirror","port":8080}}}]
            }]}
        }))
        .unwrap()
    };
    assert!(matches!(
        handler.parse(route(false), &ctx).await,
        ProcessResult::Continue(_)
    ));
    assert_eq!(
        forwarding_service_dependencies(&runtime.service_ref_mgr, &owner.key()),
        vec!["default/service"]
    );
    assert_eq!(
        runtime
            .service_ref_mgr
            .get_refs(&mirror_service_key("default", "mirror")),
        vec![owner.clone()]
    );
    assert!(matches!(
        handler.parse(route(true), &ctx).await,
        ProcessResult::Continue(_)
    ));
    let mut seen = std::collections::HashSet::new();
    while seen.len() < 2 {
        let item = tokio::time::timeout(std::time::Duration::from_secs(3), policies.workqueue().dequeue())
            .await
            .expect("retarget must requeue both affected policies")
            .expect("queue remains open");
        seen.insert(item.key.clone());
        policies.workqueue().done(&item.key);
    }
    assert_eq!(
        seen,
        std::collections::HashSet::from(["default/service-policy".into(), "default/ai-policy".into()])
    );
    assert!(forwarding_service_dependencies(&runtime.service_ref_mgr, &owner.key()).is_empty());
    assert_eq!(
        runtime
            .service_ref_mgr
            .get_refs(&mirror_service_key("default", "mirror")),
        vec![owner.clone()]
    );
    assert!(runtime
        .backend_ref_mgr
        .get_refs("edgion.io/EdgionBackend/default/models")
        .contains(&owner));
    assert!(matches!(
        handler.parse(route(false), &ctx).await,
        ProcessResult::Continue(_)
    ));
    let item = tokio::time::timeout(std::time::Duration::from_secs(3), policies.workqueue().dequeue())
        .await
        .expect("dropped AI policy must requeue")
        .unwrap();
    assert_eq!(item.key, "default/ai-policy");
    policies.workqueue().done(&item.key);
    assert!(!runtime
        .backend_ref_mgr
        .get_refs("edgion.io/EdgionBackend/default/models")
        .contains(&owner));
    assert_eq!(
        forwarding_service_dependencies(&runtime.service_ref_mgr, &owner.key()),
        vec!["default/service"]
    );
    assert_eq!(
        runtime
            .service_ref_mgr
            .get_refs(&mirror_service_key("default", "mirror")),
        vec![owner]
    );
}
