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

#[tokio::test]
async fn extension_missing_repair_delete_and_coalesced_retarget_keep_child_edges_and_raw_root_wakeup() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ProcessResult};
    use edgion_resources::EdgionPlugins;
    let runtime = Arc::new(ControllerRuntime::default());
    let routes = Arc::new(ResourceProcessor::<HTTPRoute>::new(
        "HTTPRoute",
        16,
        Arc::new(HttpRouteHandler::default()),
        runtime.clone(),
    ));
    let route_raw = Arc::new(LocalRawStore::new());
    routes.set_raw_store(route_raw.clone());
    runtime.processor_registry.register(routes.clone());
    let plugins = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let plugin_raw = Arc::new(LocalRawStore::new());
    plugins.set_raw_store(plugin_raw.clone());
    runtime.processor_registry.register(plugins);
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime.clone());
    let mut child:HTTPRoute=serde_json::from_value(serde_json::json!({"metadata":{"namespace":"children","name":"child","uid":"uid-a","generation":1},"spec":{"rules":[{"backendRefs":[{"name":"service","port":80,"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"bundle","resolvedNamespace":"forged"}}]}]}]}})).unwrap();
    route_raw.upsert("children/child".into(), Arc::new(child.clone()));
    routes.save(child.clone(), None);
    let parent:HTTPRoute=serde_json::from_value(serde_json::json!({"metadata":{"name":"parent"},"spec":{"rules":[{"backendRefs":[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute","name":"child","namespace":"children"}]}]}})).unwrap();
    let handler = HttpRouteHandler::default();
    let ProcessResult::Continue(first) = handler.parse(parent.clone(), &ctx).await else {
        panic!("parse must continue")
    };
    assert_eq!(first.spec.resolved_rules.as_ref().unwrap().len(), 1);
    let owner = HttpRouteHandler::create_resource_ref(&parent);
    assert_eq!(owner.key(), "HTTPRoute//parent");
    assert_eq!(
        runtime.plugin_ref_mgr.get_dependencies(&owner.key()),
        vec!["children/bundle"]
    );
    assert!(runtime
        .plugin_ref_mgr
        .get_dependencies("HTTPRoute/default/parent")
        .is_empty());
    let initial = tokio::time::timeout(std::time::Duration::from_secs(3), routes.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        initial.key, "children/child",
        "drain the initial delegation-edge notification before the plugin repair event"
    );
    routes.workqueue().done(&initial.key);
    let bundle:EdgionPlugins=serde_json::from_value(serde_json::json!({"metadata":{"namespace":"children","name":"bundle"},"spec":{"waf":{"policyRef":{"name":"policy"}}}})).unwrap();
    plugin_raw.upsert("children/bundle".into(), Arc::new(bundle.clone()));
    EdgionPluginsHandler::new().on_change(&bundle, &ctx).await;
    let item = tokio::time::timeout(std::time::Duration::from_secs(3), routes.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        item.key, "parent",
        "plugin change wakes the actual omitted-namespace root key"
    );
    routes.workqueue().done(&item.key);
    let ProcessResult::Continue(blocked) = handler.parse(parent.clone(), &ctx).await else {
        panic!("parse must continue")
    };
    assert!(blocked.spec.resolved_rules.as_ref().unwrap().is_empty());
    assert!(blocked.spec.delegation_issues.iter().any(|issue| matches!(issue,edgion_resources::resources::http_route::DelegationIssue::RuleDropped { message } if message.contains("cannot contain WAF"))));
    assert_eq!(
        runtime.plugin_ref_mgr.get_dependencies(&owner.key()),
        vec!["children/bundle"],
        "scope drop retains the recovery edge"
    );
    plugin_raw.remove("children/bundle");
    EdgionPluginsHandler::new().on_delete(&bundle, &ctx).await;
    let item = tokio::time::timeout(std::time::Duration::from_secs(3), routes.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "parent");
    routes.workqueue().done(&item.key);
    let ProcessResult::Continue(repaired) = handler.parse(parent.clone(), &ctx).await else {
        panic!("parse must continue")
    };
    assert_eq!(repaired.spec.resolved_rules.as_ref().unwrap().len(), 1);
    child.metadata.uid = Some("uid-b".into());
    child.metadata.generation = Some(1);
    child.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0]
        .filters
        .as_mut()
        .unwrap()[0]
        .extension_ref
        .as_mut()
        .unwrap()
        .name = "replacement".into();
    // No on_delete and no processed-cache replacement: flatten must use latest desired child.
    route_raw.upsert("children/child".into(), Arc::new(child));
    handler.parse(parent.clone(), &ctx).await;
    assert_eq!(
        runtime.plugin_ref_mgr.get_dependencies(&owner.key()),
        vec!["children/replacement"]
    );
    assert!(runtime.plugin_ref_mgr.get_refs("children/bundle").is_empty());
    let mut removed = parent;
    removed.spec.rules = None;
    handler.parse(removed, &ctx).await;
    assert!(runtime.plugin_ref_mgr.get_dependencies(&owner.key()).is_empty());
}

#[tokio::test]
async fn delegated_extension_dependency_repairs_dropped_child_and_replaces_coalesced_generation() {
    use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ProcessResult, ResourceRef};
    use edgion_resources::{EdgionPlugins, ResourceKind};

    let runtime = Arc::new(ControllerRuntime::default());
    let routes = Arc::new(ResourceProcessor::<HTTPRoute>::new(
        "HTTPRoute",
        16,
        Arc::new(HttpRouteHandler::default()),
        runtime.clone(),
    ));
    let route_raw = Arc::new(LocalRawStore::new());
    routes.set_raw_store(route_raw.clone());
    runtime.processor_registry.register(routes.clone());
    let plugins = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        16,
        Arc::new(EdgionPluginsHandler::new()),
        runtime.clone(),
    ));
    let plugin_raw = Arc::new(LocalRawStore::new());
    plugins.set_raw_store(plugin_raw.clone());
    runtime.processor_registry.register(plugins);
    let ctx = HandlerContext::new(None, None, Default::default(), 3, 20, runtime.clone());
    let handler = HttpRouteHandler::default();
    let plugin_handler = EdgionPluginsHandler::new();
    let owner = ResourceRef::new(ResourceKind::HTTPRoute, None, "root".into());
    let root: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"root","uid":"root-one","generation":1},
        "spec":{"rules":[{"backendRefs":[{"group":"gateway.networking.k8s.io","kind":"HTTPRoute","namespace":"child","name":"leaf"}]}]}
    })).unwrap();
    let child = |name: &str, uid: &str, generation| -> HTTPRoute {
        serde_json::from_value(serde_json::json!({
            "metadata":{"name":"leaf","namespace":"child","uid":uid,"generation":generation},
            "spec":{"rules":[{"backendRefs":[{"name":"service","port":80,"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":name,"resolvedNamespace":"forged"}}]}]}]}
        })).unwrap()
    };
    let plugin = |enabled, uid: &str| -> EdgionPlugins {
        serde_json::from_value(serde_json::json!({
            "metadata":{"name":"bundle","namespace":"child","uid":uid},
            "spec":{"requestPlugins":[{"type":"RequestId","enable":enabled,"config":{}}]}
        }))
        .unwrap()
    };
    let child_one = child("bundle", "child-one", 1);
    routes.save(child_one.clone(), None);
    route_raw.upsert("child/leaf".into(), Arc::new(child_one));
    let parse = |result| match result {
        ProcessResult::Continue(route) => route,
        _ => panic!("route parse must continue"),
    };
    let missing = parse(handler.parse(root.clone(), &ctx).await);
    assert_eq!(missing.spec.resolved_rules.as_ref().unwrap().len(), 1);
    assert_eq!(runtime.plugin_ref_mgr.get_refs("child/bundle"), vec![owner.clone()]);
    assert!(runtime.plugin_ref_mgr.get_refs("default/bundle").is_empty());
    assert!(runtime.plugin_ref_mgr.get_refs("forged/bundle").is_empty());

    // Creation requeues the raw-None root even though the target is child-local.
    let active = plugin(true, "plugin-one");
    plugin_raw.upsert("child/bundle".into(), Arc::new(active.clone()));
    plugin_handler.on_change(&active, &ctx).await;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let item = routes.workqueue().dequeue().await.expect("queue remains open");
            let is_root = item.key == owner.resource_key();
            routes.workqueue().done(&item.key);
            if is_root {
                break;
            }
        }
    })
    .await
    .expect("plugin event must enqueue the root despite other delegation wakeups");
    let mut dropped = parse(handler.parse(root.clone(), &ctx).await);
    assert!(dropped.spec.resolved_rules.as_ref().unwrap().is_empty());
    assert!(dropped.spec.delegation_issues.iter().any(|issue| matches!(issue,
        edgion_resources::resources::http_route::DelegationIssue::RuleDropped { message }
        if message.contains("RequestId"))));
    handler.update_status(&mut dropped, &ctx, &[]);
    assert_eq!(
        runtime.plugin_ref_mgr.get_refs("child/bundle"),
        vec![owner.clone()],
        "status must preserve the dependency of the dropped child rule"
    );

    // Delete and recreate with a new UID cannot retain old scope restrictions.
    plugin_raw.remove("child/bundle");
    plugin_handler.on_delete(&active, &ctx).await;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let item = routes.workqueue().dequeue().await.expect("queue remains open");
            let is_root = item.key == owner.resource_key();
            routes.workqueue().done(&item.key);
            if is_root {
                break;
            }
        }
    })
    .await
    .expect("plugin event must enqueue the root despite other delegation wakeups");
    let repaired = parse(handler.parse(root.clone(), &ctx).await);
    assert_eq!(repaired.spec.resolved_rules.as_ref().unwrap().len(), 1);
    let disabled = plugin(false, "plugin-two");
    plugin_raw.upsert("child/bundle".into(), Arc::new(disabled.clone()));
    plugin_handler.on_change(&disabled, &ctx).await;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let item = routes.workqueue().dequeue().await.expect("queue remains open");
            let is_root = item.key == owner.resource_key();
            routes.workqueue().done(&item.key);
            if is_root {
                break;
            }
        }
    })
    .await
    .expect("plugin event must enqueue the root despite other delegation wakeups");
    let repaired = parse(handler.parse(root.clone(), &ctx).await);
    assert_eq!(repaired.spec.resolved_rules.as_ref().unwrap().len(), 1);

    // No route on_delete runs between child generations: replace every old edge.
    let child_two = child("replacement", "child-two", 1);
    route_raw.upsert("child/leaf".into(), Arc::new(child_two));
    let replaced = parse(handler.parse(root.clone(), &ctx).await);
    assert_eq!(replaced.spec.resolved_rules.as_ref().unwrap().len(), 1);
    assert!(runtime.plugin_ref_mgr.get_refs("child/bundle").is_empty());
    assert_eq!(
        runtime.plugin_ref_mgr.get_refs("child/replacement"),
        vec![owner.clone()]
    );
    handler.on_delete(&replaced, &ctx).await;
    assert!(runtime.plugin_ref_mgr.get_dependencies(&owner.key()).is_empty());

    // An invalid new generation clears the prior set without minting new edges.
    let _ = handler.parse(root.clone(), &ctx).await;
    let mut invalid = root;
    invalid.metadata.namespace = Some("Bad".into());
    let invalid_owner = ResourceRef::new(ResourceKind::HTTPRoute, Some("Bad".into()), "root".into());
    runtime
        .plugin_ref_mgr
        .add_ref("child/stale".into(), invalid_owner.clone());
    let invalid = parse(handler.parse(invalid, &ctx).await);
    assert!(invalid.spec.resolved_rules.as_ref().unwrap().is_empty());
    assert!(runtime.plugin_ref_mgr.get_dependencies(&invalid_owner.key()).is_empty());
}
