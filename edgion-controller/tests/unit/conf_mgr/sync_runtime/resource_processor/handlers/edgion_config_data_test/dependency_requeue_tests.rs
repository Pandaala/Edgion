use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::edgion_config_data::EdgionConfigDataSpec;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

struct PluginsNoop;

#[async_trait::async_trait]
impl ProcessorHandler<EdgionPlugins> for PluginsNoop {}

struct GatewayNoop;

struct ConfigDataNoop;

#[async_trait::async_trait]
impl ProcessorHandler<EdgionConfigData> for ConfigDataNoop {}

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
    assert_eq!(
        runtime.edgion_config_data_refs.get_refs("default/url-list"),
        ["EdgionPlugins:default/policy"]
    );
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
    assert_eq!(
        runtime.edgion_config_data_refs.get_refs("default/url-list"),
        ["Gateway:default/gateway"]
    );
}

fn waf_policy() -> EdgionConfigData {
    let spec: EdgionConfigDataSpec = serde_json::from_value(serde_json::json!({
        "data": {
            "type": "WafPolicy",
            "config": {
                "profiles": {
                    "base": {"bundleRefs": [{"name": "rules", "expectedDigest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}
                },
                "defaultProfile": "base"
            }
        }
    }))
    .expect("WAF policy spec");
    let mut resource = EdgionConfigData::new("policy", spec);
    resource.metadata.namespace = Some("default".to_string());
    resource
}

#[tokio::test]
async fn waf_policy_dependency_requeues_before_plugin_and_type_switch_clears_edges() {
    let runtime = Arc::new(ControllerRuntime::default());
    let data_processor = Arc::new(ResourceProcessor::<EdgionConfigData>::new(
        "EdgionConfigData",
        100,
        Arc::new(ConfigDataNoop),
        runtime.clone(),
    ));
    let plugin_processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
        "EdgionPlugins",
        100,
        Arc::new(PluginsNoop),
        runtime.clone(),
    ));
    runtime.processor_registry.register(data_processor.clone());
    runtime.processor_registry.register(plugin_processor.clone());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let handler = EdgionConfigDataHandler::new();
    let policy = waf_policy();
    let _ = handler.parse(policy.clone(), &ctx).await;
    assert_eq!(
        runtime.edgion_config_data_refs.get_refs("default/rules"),
        vec!["EdgionConfigData:default/policy"]
    );

    runtime
        .edgion_config_data_refs
        .add_ref("default/policy".into(), "EdgionPlugins:default/plugin".into());
    let mut child = resource();
    child.metadata.name = Some("rules".into());
    handler.on_change(&child, &ctx).await;
    let item = tokio::time::timeout(Duration::from_secs(1), data_processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "default/policy");
    data_processor.workqueue().done(&item.key);
    handler.on_change(&policy, &ctx).await;
    let item = tokio::time::timeout(Duration::from_secs(1), plugin_processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "default/plugin");
    plugin_processor.workqueue().done(&item.key);

    let mut replacement = resource();
    replacement.metadata.name = Some("policy".into());
    replacement.spec.data = edgion_resources::resources::edgion_config_data::ConfigEntry::Unsupported;
    replacement.metadata.generation = Some(2);
    let ProcessResult::Continue(mut unknown) = handler.parse(replacement, &ctx).await else {
        panic!("unknown type must continue to replace stale state");
    };
    let errors = unknown.validate_pre_parse();
    handler.update_status(&mut unknown, &ctx, &errors);
    let accepted = unknown
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .find(|c| c.type_ == "Accepted")
        .unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "Invalid");
    assert_eq!(accepted.observed_generation, Some(2));
    assert!(runtime.edgion_config_data_refs.get_refs("default/rules").is_empty());
    assert!(matches!(
        runtime
            .edgion_config_data_store
            .get(Some("default"), "policy")
            .unwrap()
            .spec
            .data,
        edgion_resources::resources::edgion_config_data::ConfigEntry::Unsupported
    ));
    handler.on_change(&unknown, &ctx).await;
    let item = tokio::time::timeout(Duration::from_secs(1), plugin_processor.workqueue().dequeue())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.key, "default/plugin");
    plugin_processor.workqueue().done(&item.key);
    let mut recovered = policy;
    recovered.metadata.generation = Some(3);
    let ProcessResult::Continue(mut recovered) = handler.parse(recovered, &ctx).await else {
        panic!("must recover")
    };
    let errors = recovered.validate_pre_parse();
    handler.update_status(&mut recovered, &ctx, &errors);
    assert_eq!(
        runtime.edgion_config_data_refs.get_refs("default/rules"),
        vec!["EdgionConfigData:default/policy"]
    );
    let accepted = recovered
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .find(|c| c.type_ == "Accepted")
        .unwrap();
    assert_eq!(accepted.status, "True");
    assert_eq!(accepted.observed_generation, Some(3));
}

#[test]
fn waf_policy_required_failure_sets_resolved_refs_without_partial_warning() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime);
    let handler = EdgionConfigDataHandler::new();
    let mut policy = waf_policy();
    handler.update_status(&mut policy, &ctx, &[]);
    let conditions = &policy.status.as_ref().unwrap().conditions;
    let refs = conditions
        .iter()
        .find(|condition| condition.type_ == condition_types::RESOLVED_REFS)
        .unwrap();
    assert_eq!(refs.status, "False");
    assert!(refs.message.contains("WafRuleBundle default/rules is unavailable"));
    let partial = conditions
        .iter()
        .find(|condition| condition.type_ == condition_types::PARTIALLY_INVALID)
        .unwrap();
    assert_eq!(partial.status, "False");

    policy.spec.data = resource().spec.data;
    handler.update_status(&mut policy, &ctx, &[]);
    assert!(policy
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .all(|condition| condition.type_ != condition_types::PARTIALLY_INVALID));
}

#[test]
fn unsupported_bundle_cannot_satisfy_strict_waf_policy_reference() {
    let runtime = Arc::new(ControllerRuntime::default());
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    let mut target = resource();
    target.metadata.name = Some("rules".into());
    target.spec.data = edgion_resources::resources::edgion_config_data::ConfigEntry::Unsupported;
    runtime
        .edgion_config_data_store
        .update(HashMap::from([("default/rules".into(), target)]), &HashSet::new());
    let mut policy = waf_policy();
    EdgionConfigDataHandler::new().update_status(&mut policy, &ctx, &[]);
    let refs = policy
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .find(|c| c.type_ == "ResolvedRefs")
        .unwrap();
    assert_eq!(refs.status, "False");
    assert!(refs.message.contains("has the wrong type"));
}
