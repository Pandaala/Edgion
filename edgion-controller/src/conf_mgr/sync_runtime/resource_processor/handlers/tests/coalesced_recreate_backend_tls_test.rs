//! Locks the coalescing guarantee for route -> BackendTLSPolicy: a
//! delete->recreate that drops a backend Service must requeue the
//! BackendTLSPolicies bound to that dropped Service, even though
//! handler.on_delete never runs (the workqueue coalesces by key). The apply
//! path's `requeue_backend_policies_for_route` only sees the route's
//! CURRENT backends, so the dropped Service's BTPs rely on the return-delta
//! requeue added to `register_service_backend_refs`.

use super::TcpRouteHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, ProcessorHandler, ResourceProcessor, ResourceRef,
};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::{BackendTLSPolicy, TCPRoute};
use edgion_resources::resources::tcp_route::{TCPBackendRef, TCPRouteRule, TCPRouteSpec};
use edgion_resources::ResourceKind;
use kube::api::ObjectMeta;
use std::sync::Arc;

struct BtpNoop;
#[async_trait::async_trait]
impl ProcessorHandler<BackendTLSPolicy> for BtpNoop {}

fn tcp_route_with_backend(svc: &str) -> TCPRoute {
    TCPRoute {
        metadata: ObjectMeta {
            name: Some("r1".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: TCPRouteSpec {
            current_status: None,
            resolved_status_controller: None,
            parent_refs: None,
            rules: Some(vec![TCPRouteRule {
                backend_refs: Some(vec![TCPBackendRef {
                    name: svc.to_string(),
                    namespace: None,
                    port: Some(80),
                    weight: None,
                    group: None,
                    kind: None,
                    ref_denied: None,
                }]),
            }]),
            resolved_listeners: None,
        },
        status: None,
    }
}

#[tokio::test]
async fn coalesced_recreate_requeues_btp_on_dropped_backend_service() {
    let runtime = Arc::new(ControllerRuntime::default());

    // Register a BackendTLSPolicy processor so ctx.requeue("BackendTLSPolicy", ...)
    // has an observable queue.
    let btp_processor = Arc::new(ResourceProcessor::<BackendTLSPolicy>::new(
        "BackendTLSPolicy",
        100,
        Arc::new(BtpNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(btp_processor.clone());

    // A BackendTLSPolicy targets default/svc-a. `parse_policy` registers the
    // target dependency in `service_ref_mgr` unconditionally — before the target
    // or the CA refs are known to resolve — so registering only there also
    // covers a policy whose refs never resolve.
    let btp_ref = ResourceRef::new(
        ResourceKind::BackendTLSPolicy,
        Some("default".to_string()),
        "btp-a".to_string(),
    );
    runtime
        .service_ref_mgr
        .add_ref("default/svc-a".to_string(), btp_ref.clone());

    let handler = TcpRouteHandler::default();
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime.clone());

    // Generation 1: route backends svc-a.
    let _ = handler.parse(tcp_route_with_backend("svc-a"), &ctx).await;
    // Drain any requeues generation 1 produced.
    while tokio::time::timeout(
        std::time::Duration::from_millis(500),
        btp_processor.workqueue().dequeue(),
    )
    .await
    .is_ok()
    {}

    // Coalesced delete->recreate: the worker only ever sees generation 2 (svc-b).
    // on_delete never runs. The BTP on the dropped svc-a must still be requeued.
    let _ = handler.parse(tcp_route_with_backend("svc-b"), &ctx).await;

    let mut seen = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && !seen.contains("default/btp-a") {
        if let Ok(Some(item)) = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            btp_processor.workqueue().dequeue(),
        )
        .await
        {
            seen.insert(item.key.clone());
            btp_processor.workqueue().done(&item.key);
        }
    }
    assert!(
        seen.contains("default/btp-a"),
        "BackendTLSPolicy on dropped backend svc-a was not requeued; seen: {:?}",
        seen
    );
}
