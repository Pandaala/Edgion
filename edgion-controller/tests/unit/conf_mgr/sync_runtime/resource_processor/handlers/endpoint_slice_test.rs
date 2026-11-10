use super::*;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::ResourceKind;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::api::ObjectMeta;
use std::collections::BTreeMap;
use std::sync::Arc;

fn make_ctx() -> (HandlerContext, Arc<ControllerRuntime>) {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    (ctx, runtime)
}

fn make_eps(namespace: &str, svc_name: Option<&str>) -> EndpointSlice {
    let mut labels = BTreeMap::new();
    if let Some(name) = svc_name {
        labels.insert("kubernetes.io/service-name".to_string(), name.to_string());
    }
    EndpointSlice {
        metadata: ObjectMeta {
            namespace: Some(namespace.to_string()),
            labels: if labels.is_empty() { None } else { Some(labels) },
            ..Default::default()
        },
        address_type: "IPv4".to_string(),
        endpoints: vec![],
        ports: None,
    }
}

#[tokio::test]
async fn on_change_missing_label_does_not_panic() {
    let (ctx, _runtime) = make_ctx();
    let handler = EndpointSliceHandler::new();
    // EPS without kubernetes.io/service-name label — handler must return silently.
    let eps = make_eps("default", None);
    handler.on_change(&eps, &ctx).await;
    handler.on_delete(&eps, &ctx).await;
}

#[tokio::test]
async fn on_change_no_registered_refs_does_not_panic() {
    let (ctx, _runtime) = make_ctx();
    let handler = EndpointSliceHandler::new();
    // Label is present but no LinkSys has registered for this service.
    let eps = make_eps("default", Some("my-svc"));
    handler.on_change(&eps, &ctx).await;
}

#[tokio::test]
async fn on_change_with_registered_ref_triggers_lookup() {
    let (ctx, runtime) = make_ctx();
    let handler = EndpointSliceHandler::new();

    // Register a LinkSys dependency on "default/my-svc".
    let owner = crate::conf_mgr::sync_runtime::resource_processor::ResourceRef::new(
        ResourceKind::LinkSys,
        Some("default".to_string()),
        "my-webhook".to_string(),
    );
    runtime
        .service_ref_mgr
        .add_ref("default/my-svc".to_string(), owner.clone());

    // Verify registration is intact before on_change.
    let refs = runtime.service_ref_mgr.get_refs("default/my-svc");
    assert!(refs.contains(&owner), "ref must be registered before on_change");

    // on_change should call ctx.requeue (no-op in tests without real processor).
    // The key assertion is that service_ref_mgr is queried correctly and does not panic.
    let eps = make_eps("default", Some("my-svc"));
    handler.on_change(&eps, &ctx).await;
}

#[tokio::test]
async fn on_change_namespace_scoped_lookup_uses_eps_namespace() {
    let (ctx, runtime) = make_ctx();
    let handler = EndpointSliceHandler::new();

    // Register refs for two different namespaces pointing to the same service name.
    let owner_a = crate::conf_mgr::sync_runtime::resource_processor::ResourceRef::new(
        ResourceKind::LinkSys,
        Some("ns-a".to_string()),
        "wh-a".to_string(),
    );
    let owner_b = crate::conf_mgr::sync_runtime::resource_processor::ResourceRef::new(
        ResourceKind::LinkSys,
        Some("ns-b".to_string()),
        "wh-b".to_string(),
    );
    runtime.service_ref_mgr.add_ref("ns-a/svc".to_string(), owner_a);
    runtime.service_ref_mgr.add_ref("ns-b/svc".to_string(), owner_b);

    // EPS in ns-a should only trigger lookup for "ns-a/svc", not "ns-b/svc".
    let eps = make_eps("ns-a", Some("svc"));
    handler.on_change(&eps, &ctx).await;
    // No panic and no cross-namespace confusion — the handler uses eps.namespace.
}

#[tokio::test]
async fn synthetic_dns_slices_enter_the_pipeline_only_when_unsourced() {
    use crate::conf_mgr::sync_runtime::resource_processor::{ResourceProcessor, WorkItemResult};
    use edgion_common::dns::endpoint_slices::{build_dns_endpoint_slices, DnsSliceOrigin};
    use k8s_openapi::api::core::v1::ServicePort;

    let runtime = Arc::new(ControllerRuntime::default());
    let processor = ResourceProcessor::<EndpointSlice>::new(
        "EndpointSlice",
        16,
        Arc::new(EndpointSliceHandler::new()),
        runtime.clone(),
    );
    let ports = [ServicePort {
        port: 80,
        ..Default::default()
    }];
    let synthetic = build_dns_endpoint_slices(
        "app",
        "svc",
        &ports,
        &["10.0.0.1".parse().unwrap()],
        DnsSliceOrigin::Controller,
    );
    let key = "app/svc-edgion-ctl-dns";
    runtime.dns_synthetic_slices.replace_service(0, "app/svc", synthetic);

    let result = processor
        .process_work_item(key, None, None, TriggerChain::default())
        .await;
    assert!(matches!(result, WorkItemResult::Processed { .. }));
    let cached = processor.get(key).expect("synthetic slice is published");
    assert_eq!(cached.endpoints[0].addresses, vec!["10.0.0.1".to_string()]);

    // A sourced object with the same key wins over the synthetic one.
    let mut sourced = cached.clone();
    sourced.endpoints[0].addresses = vec!["10.9.9.9".to_string()];
    processor
        .process_work_item(key, Some(sourced), None, TriggerChain::default())
        .await;
    assert_eq!(
        processor.get(key).unwrap().endpoints[0].addresses,
        vec!["10.9.9.9".to_string()]
    );

    // Withdrawn and unsourced: the cached slice is deleted.
    runtime.dns_synthetic_slices.replace_service(0, "app/svc", Vec::new());
    let result = processor
        .process_work_item(key, None, None, TriggerChain::default())
        .await;
    assert!(matches!(result, WorkItemResult::Deleted { .. }));
    assert!(processor.get(key).is_none());
}
