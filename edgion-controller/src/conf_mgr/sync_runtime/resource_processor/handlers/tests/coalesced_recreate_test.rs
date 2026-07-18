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
    // Drain the requeues generation 1 produced (gw-a itself is requeued).
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
