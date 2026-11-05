use super::*;
use crate::conf_mgr::sync_runtime::{
    resource_processor::{handlers::EdgionConfigDataHandler, HandlerContext, ProcessorHandler},
    workqueue::TriggerChain,
};
use crate::conf_mgr::{ControllerRuntime, ResourceProcessor};
use serde_json::json;
use std::sync::Arc;

fn source() -> EdgionConfigData {
    serde_json::from_value(json!({"apiVersion":"edgion.io/v1","kind":"EdgionConfigData",
        "metadata":{"namespace":"apps","name":"regional","uid":"first","resourceVersion":"1"},
        "spec":{"enable":false,"data":{"type":"ClusterRegionSetting","config":{
            "clusterName":"fleet","regionName":"east","regions":[]}}}}))
    .unwrap()
}
fn plugins(alias: &str) -> EdgionPlugins {
    serde_json::from_value(json!({"apiVersion":"edgion.io/v1","kind":"EdgionPlugins",
        "metadata":{"namespace":"apps","name":"plugins"},
        "spec":{"requestPlugins":[{"type":"RegionRoute","alias":alias,"config":{"clusterRegionRef":{"name":"regional"}}}]}})).unwrap()
}

#[tokio::test]
async fn real_handler_keeps_disabled_invalid_source_then_replaces_identity_and_type() {
    let runtime = Arc::new(ControllerRuntime::new());
    let processor = ResourceProcessor::new(
        "EdgionConfigData",
        8,
        Arc::new(EdgionConfigDataHandler::new()),
        runtime.clone(),
    );
    processor.on_init_apply(source(), None).await;
    processor.on_init_done();
    let initial = runtime.center_watch.snapshot().unwrap();
    assert_eq!(initial.records.len(), 1);
    let projection::Record::Setting(row) = &*initial.records[0] else {
        panic!("setting");
    };
    assert!(!row.enabled);
    assert_eq!(row.region_name.as_deref(), Some("east"));
    let mut changed = source();
    changed.metadata.uid = Some("second".into());
    if let edgion_resources::resources::edgion_config_data::ConfigEntry::ClusterRegionSetting(config) =
        &mut changed.spec.data
    {
        config.region_name = "west".into();
    }
    processor
        .process_work_item("apps/regional", Some(changed), None, TriggerChain::default())
        .await;
    let changed = runtime.center_watch.snapshot().unwrap();
    let projection::Record::Setting(row) = &*changed.records[0] else {
        panic!("setting");
    };
    assert_eq!(row.region_name.as_deref(), Some("west"));
    assert_eq!(row.identity.uid.as_deref(), Some("second"));
    let mut other = serde_json::to_value(source()).unwrap();
    other["spec"]["data"] = json!({"type":"KeyList","config":{"items":[]}});
    processor
        .process_work_item(
            "apps/regional",
            Some(serde_json::from_value(other).unwrap()),
            None,
            TriggerChain::default(),
        )
        .await;
    assert!(runtime.center_watch.snapshot().unwrap().records.is_empty());
}

struct NoGatewayPublication;
#[async_trait::async_trait]
impl ProcessorHandler<EdgionConfigData> for NoGatewayPublication {
    fn should_publish(&self, _: &EdgionConfigData, _: &HandlerContext) -> bool {
        false
    }
}

#[tokio::test]
async fn deletion_from_replacement_list_cleans_view_even_without_gateway_cache_entry() {
    use crate::conf_mgr::sync_runtime::resource_processor::raw_store::{KubeRawStore, RawStore};
    use kube::runtime::{reflector::store::Writer, watcher::Event};
    let runtime = Arc::new(ControllerRuntime::new());
    let processor = ResourceProcessor::new("EdgionConfigData", 8, Arc::new(NoGatewayPublication), runtime.clone());
    let mut writer = Writer::<EdgionConfigData>::default();
    let raw = Arc::new(KubeRawStore::new(writer.as_reader()));
    processor.set_raw_store(raw.clone());
    writer.apply_watcher_event(&Event::Init);
    writer.apply_watcher_event(&Event::InitApply(source()));
    writer.apply_watcher_event(&Event::InitDone);
    processor
        .on_init_apply((*raw.get("apps/regional").unwrap()).clone(), None)
        .await;
    processor.on_init_done();
    assert!(processor.get("apps/regional").is_none());
    assert_eq!(runtime.center_watch.snapshot().unwrap().records.len(), 1);
    writer.apply_watcher_event(&Event::Init);
    writer.apply_watcher_event(&Event::InitDone);
    assert!(raw.get("apps/regional").is_none());
    processor
        .process_work_item("apps/regional", None, None, TriggerChain::default())
        .await;
    assert!(runtime.center_watch.snapshot().unwrap().records.is_empty());
}

#[tokio::test]
async fn namespace_loss_and_old_processor_after_reset_cannot_republish() {
    let runtime = Arc::new(ControllerRuntime::new());
    let processor = ResourceProcessor::new("EdgionConfigData", 8, Arc::new(NoGatewayPublication), runtime.clone());
    processor.on_init_apply(source(), None).await;
    processor.on_init_done();
    processor.set_namespace_filter(vec!["other".into()]);
    processor
        .process_work_item("apps/regional", Some(source()), None, TriggerChain::default())
        .await;
    assert!(runtime.center_watch.snapshot().unwrap().records.is_empty());
    processor.set_namespace_filter(vec!["apps".into()]);
    runtime.reset();
    processor
        .process_work_item("apps/regional", Some(source()), None, TriggerChain::default())
        .await;
    assert!(runtime.center_watch.snapshot().unwrap().records.is_empty());
}

#[test]
fn invalid_projection_recovers_by_fix_or_delete_without_runtime_reset() {
    let runtime = ControllerRuntime::new();
    let generation = runtime.center_watch.generation();
    apply(&runtime.center_watch, &generation, &plugins("valid"), true);
    let before = runtime.center_watch.snapshot().unwrap();
    apply(&runtime.center_watch, &generation, &plugins(&"x".repeat(257)), true);
    assert_eq!(runtime.center_watch.snapshot().unwrap_err(), StoreError::Projection);
    // The fixed projection equals the preserved record; clearing the fault still matters.
    apply(&runtime.center_watch, &generation, &plugins("valid"), true);
    assert_eq!(runtime.center_watch.snapshot().unwrap().version, before.version);
    assert_eq!(runtime.center_watch.generation(), generation);
    apply(&runtime.center_watch, &generation, &plugins(&"x".repeat(257)), true);
    delete(&runtime.center_watch, &generation, "EdgionPlugins", "apps/plugins");
    assert!(runtime.center_watch.snapshot().unwrap().records.is_empty());
    assert_eq!(runtime.center_watch.generation(), generation);
}
