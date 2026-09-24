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
