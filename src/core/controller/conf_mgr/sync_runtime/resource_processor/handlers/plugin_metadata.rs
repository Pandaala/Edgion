//! PluginMetadata Handler
//!
//! Handles PluginMetaData resources.
//!
//! Responsibilities:
//! - Maintain PluginMetaDataStore in ControllerRuntime (parse + on_delete)
//! - Maintain RegionRoute tables (global + service) on change/delete
//! - On change: requeue referencing EdgionPlugins/EdgionStreamPlugins so they
//!   pick up updated failover / match_map from the PluginMetaData.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::region_route_table::{
    GlobalRegionRouteEntry, ServiceRegionRouteEntry,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, PluginMetaDataStore, ProcessResult, ProcessorHandler,
};
use crate::types::prelude_resources::PluginMetaData;
use crate::types::resources::plugin_metadata::MetaDataEntry;

pub struct PluginMetadataHandler;

impl PluginMetadataHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PluginMetadataHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Update the RegionRoute tables based on PluginMetaData content.
fn update_region_route_tables(pm: &PluginMetaData, ctx: &HandlerContext) {
    let ns = pm.metadata.namespace.as_deref().unwrap_or("");
    let name = pm.metadata.name.as_deref().unwrap_or("");
    let pm_key = PluginMetaDataStore::make_key(Some(ns), name);

    match &pm.spec.metadata {
        MetaDataEntry::GlobalRegionRoute(cfg) => {
            ctx.runtime().global_region_route_table.upsert(
                pm_key,
                GlobalRegionRouteEntry {
                    pm_namespace: ns.to_string(),
                    pm_name: name.to_string(),
                    my_region: cfg.my_region.clone(),
                    regions: cfg.regions.clone(),
                    key_get: cfg.key_get.clone(),
                    hash_key_get: cfg.hash_key_get.clone(),
                    hash_calc: cfg.hash_calc.clone(),
                    route_rules: cfg.route_rules.clone(),
                    route_by_key_conf_match: cfg.route_by_key_conf_match.clone(),
                },
            );
        }
        MetaDataEntry::ServiceRegionRoute(cfg) => {
            ctx.runtime().service_region_route_table.upsert_from_pm(
                pm_key,
                ServiceRegionRouteEntry {
                    pm_namespace: ns.to_string(),
                    pm_name: name.to_string(),
                    global_pm_ref: cfg.global_ref.clone(),
                    regions: cfg.regions.clone(),
                    ref_plugins: vec![],
                },
            );
        }
        _ => {}
    }
}

/// Remove entries from RegionRoute tables for a deleted PluginMetaData.
/// Unconditionally removes from both tables — the PluginMetaData's spec may have been
/// modified before deletion, so we can't rely on its current plugin list.
fn remove_from_region_route_tables(pm: &PluginMetaData, ctx: &HandlerContext) {
    let pm_key = PluginMetaDataStore::make_key(
        pm.metadata.namespace.as_deref(),
        pm.metadata.name.as_deref().unwrap_or(""),
    );
    ctx.runtime().global_region_route_table.remove(&pm_key);
    ctx.runtime().service_region_route_table.remove(&pm_key);
}

/// Requeue all EdgionPlugins/EdgionStreamPlugins that reference the given PluginMetaData.
async fn requeue_pm_referrers(pm: &PluginMetaData, ctx: &HandlerContext) {
    let ns = pm.metadata.namespace.as_deref().unwrap_or("");
    let name = pm.metadata.name.as_deref().unwrap_or("");
    let pm_key = PluginMetaDataStore::make_key(Some(ns), name);

    let refs = ctx.runtime().plugin_metadata_refs.get_refs(&pm_key);
    for ep_key in refs {
        if let Some((kind_str, key)) = ep_key.split_once(':') {
            ctx.requeue(kind_str, key.to_string()).await;
        }
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<PluginMetaData> for PluginMetadataHandler {
    async fn parse(&self, pm: PluginMetaData, ctx: &HandlerContext) -> ProcessResult<PluginMetaData> {
        let key = PluginMetaDataStore::make_key(
            pm.metadata.namespace.as_deref(),
            pm.metadata.name.as_deref().unwrap_or(""),
        );
        ctx.runtime()
            .plugin_metadata_store
            .update([(key, pm.clone())].into_iter().collect(), &Default::default());

        update_region_route_tables(&pm, ctx);

        ProcessResult::Continue(pm)
    }

    async fn on_change(&self, pm: &PluginMetaData, ctx: &HandlerContext) {
        requeue_pm_referrers(pm, ctx).await;
    }

    async fn on_delete(&self, pm: &PluginMetaData, ctx: &HandlerContext) {
        let key = PluginMetaDataStore::make_key(
            pm.metadata.namespace.as_deref(),
            pm.metadata.name.as_deref().unwrap_or(""),
        );
        ctx.runtime()
            .plugin_metadata_store
            .update(Default::default(), &[key].into_iter().collect());

        remove_from_region_route_tables(pm, ctx);
        requeue_pm_referrers(pm, ctx).await;
    }
}
