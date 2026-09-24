use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::edgion_config_data::EdgionConfigDataSpec;
use std::sync::Arc;
use std::time::Duration;

struct PluginsNoop;

#[async_trait::async_trait]
impl ProcessorHandler<EdgionPlugins> for PluginsNoop {}

struct GatewayNoop;

#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for GatewayNoop {}

fn resource() -> EdgionConfigData {
    let spec: EdgionConfigDataSpec = serde_json::from_value(serde_json::json!({
        "data": {"type": "Misc", "config": {}}
    }))
    .expect("minimal ConfigData spec");
    let mut resource = EdgionConfigData::new("url-list", spec);
    resource.metadata.namespace = Some("default".to_string());
    resource
}

async fn assert_plugins_requeued(processor: &ResourceProcessor<EdgionPlugins>) {
    let item = tokio::time::timeout(Duration::from_secs(1), processor.workqueue().dequeue())
        .await
        .expect("dependent plugin should be requeued")
        .expect("work item should exist");
    assert_eq!(item.key, "default/policy");
    processor.workqueue().done(&item.key);
}

async fn assert_gateway_requeued(processor: &ResourceProcessor<Gateway>) {
    let item = tokio::time::timeout(Duration::from_secs(1), processor.workqueue().dequeue())
        .await
        .expect("dependent Gateway should be requeued")
        .expect("work item should exist");
    assert_eq!(item.key, "default/gateway");
    processor.workqueue().done(&item.key);
}

#[tokio::test]
async fn config_data_apply_and_delete_requeue_request_access_policy_dependents() {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        100,
        Arc::new(PluginsNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let resource = resource();
    let source_key = "default/url-list".to_string();
    let dependent_key = "EdgionPlugins:default/policy".to_string();

    runtime
        .edgion_config_data_refs
        .add_ref(source_key.clone(), dependent_key.clone());
    EdgionConfigDataHandler::new().on_change(&resource, &ctx).await;
    assert_plugins_requeued(&processor).await;

    runtime.edgion_config_data_refs.add_ref(source_key, dependent_key);
    EdgionConfigDataHandler::new().on_delete(&resource, &ctx).await;
    assert_plugins_requeued(&processor).await;
    assert!(runtime.edgion_config_data_refs.get_refs("default/url-list").is_empty());
}

#[tokio::test]
async fn proxy_protocol_config_data_apply_and_delete_requeue_gateway() {
    let runtime = Arc::new(ControllerRuntime::default());
    let processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        100,
        Arc::new(GatewayNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let resource = resource();
    let source_key = "default/url-list".to_string();
    let dependent_key = "Gateway:default/gateway".to_string();

    runtime
        .edgion_config_data_refs
        .add_ref(source_key.clone(), dependent_key.clone());
    EdgionConfigDataHandler::new().on_change(&resource, &ctx).await;
    assert_gateway_requeued(&processor).await;

    runtime.edgion_config_data_refs.add_ref(source_key, dependent_key);
    EdgionConfigDataHandler::new().on_delete(&resource, &ctx).await;
    assert_gateway_requeued(&processor).await;
    assert!(runtime.edgion_config_data_refs.get_refs("default/url-list").is_empty());
}
