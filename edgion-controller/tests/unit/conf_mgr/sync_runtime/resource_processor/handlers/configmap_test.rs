use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::{
    format_configmap_ref_key,
    frontend_ca_ref::{FrontendCaSourceKey, FrontendCaSourceKind},
    HandlerContext, ProcessorHandler, ResourceProcessor, ResourceRef,
};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::ResourceKind;
use kube::api::ObjectMeta;
use std::sync::Arc;

struct GatewayNoop;

#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for GatewayNoop {}

#[tokio::test]
async fn configmap_change_requeues_typed_material_dependent() {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        100,
        Arc::new(GatewayNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let dependent = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );
    let namespace = "default".to_string();
    runtime
        .secret_ref_manager
        .add_ref(format_configmap_ref_key(Some(&namespace), "ca-bundle"), dependent);
    let configmap = ConfigMap {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("ca-bundle".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    ConfigMapHandler::new().on_change(&configmap, &ctx).await;

    let item = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "default/gateway");
    processor.workqueue().done(&item.key);
}

#[tokio::test]
async fn configmap_delete_then_recreate_preserves_typed_edge_and_requeues_twice() {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        100,
        Arc::new(GatewayNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let dependent = ResourceRef::new(
        ResourceKind::Gateway,
        Some("default".to_string()),
        "gateway".to_string(),
    );
    let source = FrontendCaSourceKey::new(FrontendCaSourceKind::ConfigMap, "default", "same-name");
    runtime.frontend_ca_ref_manager.add_ref(source.index_key(), dependent);
    let configmap = ConfigMap {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("same-name".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    ConfigMapHandler::new().on_delete(&configmap, &ctx).await;
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.key, "default/gateway");
    processor.workqueue().done(&first.key);
    assert_eq!(runtime.frontend_ca_ref_manager.get_refs(&source.index_key()).len(), 1);

    ConfigMapHandler::new().on_change(&configmap, &ctx).await;
    let second = tokio::time::timeout(std::time::Duration::from_secs(2), processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.key, "default/gateway");
    processor.workqueue().done(&second.key);
}
