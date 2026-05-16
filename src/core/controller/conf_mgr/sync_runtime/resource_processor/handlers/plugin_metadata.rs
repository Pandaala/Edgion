//! PluginMetadata Handler
//!
//! Handles PluginMetaData resources.
//!
//! Responsibilities:
//! - Maintain PluginMetaDataStore in ControllerRuntime (parse + on_delete)
//! - Maintain RegionRoute tables (global + service) on change/delete
//! - Register cross-PM references (ServiceRegionRoute.clusterRef → ClusterRegionRoute PM)
//!   so a change to a ClusterRegionRoute PM requeues the dependent ServiceRegionRoute PMs.
//! - On change: requeue referencing EdgionPlugins/EdgionStreamPlugins (and dependent
//!   ServiceRegionRoute PMs) so they pick up updated failover / match_map.
//! - On update_status: set `Accepted` (from preparse errors) and `ResolvedRefs`
//!   (from ServiceRegionRoute.clusterRef resolution) conditions per Gateway API conventions.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::region_route_table::{
    ClusterRegionRouteEntry, ServiceRegionRouteEntry,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, update_condition, ResolvedRefsError,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, PluginMetaDataStore, ProcessResult, ProcessorHandler,
};
use crate::types::prelude_resources::PluginMetaData;
use crate::types::resources::plugin_metadata::{MetaDataEntry, PluginMetaDataStatus};

/// Kind string used when a PluginMetaData registers a cross-PM reference in the shared
/// `plugin_metadata_refs` manager. The existing `requeue_pm_referrers` splits refs on `:`
/// and uses the prefix as the kind for `ctx.requeue`, which matches the PluginMetaData
/// processor registered in the processor registry.
const SELF_KIND: &str = "PluginMetaData";

pub struct PluginMetadataHandler;

impl PluginMetadataHandler {
    pub fn new() -> Self {
        Self
    }

    fn self_ref_key(pm: &PluginMetaData) -> String {
        let ns = pm.metadata.namespace.as_deref().unwrap_or("");
        let name = pm.metadata.name.as_deref().unwrap_or("");
        format!("{}:{}/{}", SELF_KIND, ns, name)
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
        MetaDataEntry::ClusterRegionRoute(cfg) => {
            ctx.runtime().cluster_region_route_table.upsert(
                pm_key,
                ClusterRegionRouteEntry {
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
                    cluster_pm_ref: cfg.cluster_ref.clone(),
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
    ctx.runtime().cluster_region_route_table.remove(&pm_key);
    ctx.runtime().service_region_route_table.remove(&pm_key);
}

/// Register this PM's cross-PM references in the shared `plugin_metadata_refs` manager.
///
/// Currently only `ServiceRegionRoute.clusterRef → ClusterRegionRoute PM` is tracked.
/// Entry stored: `cluster_pm_key -> {"PluginMetaData:ns/service_pm_name", ...}`.
/// When the cluster PM changes, `requeue_pm_referrers` picks the service PM back up
/// and re-runs its `update_status`, refreshing `ResolvedRefs`.
fn register_cross_pm_refs(pm: &PluginMetaData, ctx: &HandlerContext) {
    if let MetaDataEntry::ServiceRegionRoute(cfg) = &pm.spec.metadata {
        if let Some(cref) = &cfg.cluster_ref {
            let pm_ns = pm.metadata.namespace.as_deref().unwrap_or("");
            let cluster_ns = cref.namespace.as_deref().unwrap_or(pm_ns);
            let cluster_pm_key = format!("{}/{}", cluster_ns, cref.name);
            ctx.runtime()
                .plugin_metadata_refs
                .add_ref(cluster_pm_key, PluginMetadataHandler::self_ref_key(pm));
        }
    }
}

/// Requeue resources that reference this PluginMetaData.
///
/// Historically: EdgionPlugins / EdgionStreamPlugins referencing regions, etc.
/// Newly: other PluginMetaData (ServiceRegionRoute whose clusterRef points here).
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
    fn preparse(&self, pm: &mut PluginMetaData, _ctx: &HandlerContext) -> Vec<String> {
        pm.validate_pre_parse()
    }

    async fn parse(&self, pm: PluginMetaData, ctx: &HandlerContext) -> ProcessResult<PluginMetaData> {
        let key = PluginMetaDataStore::make_key(
            pm.metadata.namespace.as_deref(),
            pm.metadata.name.as_deref().unwrap_or(""),
        );
        ctx.runtime()
            .plugin_metadata_store
            .update([(key, pm.clone())].into_iter().collect(), &Default::default());

        update_region_route_tables(&pm, ctx);

        // Rebuild this PM's outgoing cross-PM refs from scratch (covers the case where
        // a ServiceRegionRoute's clusterRef was edited between revisions).
        ctx.runtime()
            .plugin_metadata_refs
            .clear_refs_from(&Self::self_ref_key(&pm));
        register_cross_pm_refs(&pm, ctx);

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
            .update(Default::default(), &[key.clone()].into_iter().collect());

        remove_from_region_route_tables(pm, ctx);
        ctx.runtime()
            .plugin_metadata_refs
            .clear_refs_from(&Self::self_ref_key(pm));
        requeue_pm_referrers(pm, ctx).await;
        ctx.runtime().plugin_metadata_refs.remove_pm(&key);
    }

    fn update_status(&self, pm: &mut PluginMetaData, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = pm.metadata.generation;

        // Compute both conditions first (both need immutable access to `pm` / `ctx`),
        // then apply them in a single mutable-borrow scope on `pm.status`.
        let accepted = if validation_errors.is_empty() {
            condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Resource accepted",
                generation,
            )
        } else {
            condition_false(
                condition_types::ACCEPTED,
                condition_reasons::INVALID,
                validation_errors.join("; "),
                generation,
            )
        };

        let resolved_errors = resolve_refs(pm, ctx);
        let resolved_refs = if resolved_errors.is_empty() {
            condition_true(
                condition_types::RESOLVED_REFS,
                condition_reasons::RESOLVED_REFS,
                "All references resolved",
                generation,
            )
        } else {
            let reason = resolved_errors[0].reason();
            let message = resolved_errors
                .iter()
                .map(|e| e.message())
                .collect::<Vec<_>>()
                .join("; ");
            condition_false(condition_types::RESOLVED_REFS, reason, message, generation)
        };

        let status = pm.status.get_or_insert_with(PluginMetaDataStatus::default);
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
    }
}

/// Resolve cross-PM references.
///
/// Today: only `ServiceRegionRoute.clusterRef` must point at an existing
/// `ClusterRegionRoute` PluginMetaData. Returns an empty vec for variants without
/// external refs.
fn resolve_refs(pm: &PluginMetaData, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    if let MetaDataEntry::ServiceRegionRoute(cfg) = &pm.spec.metadata {
        if let Some(cref) = &cfg.cluster_ref {
            let pm_ns = pm.metadata.namespace.as_deref().unwrap_or("");
            let cluster_ns = cref.namespace.as_deref().unwrap_or(pm_ns);
            let target = ctx.runtime().plugin_metadata_store.get(Some(cluster_ns), &cref.name);
            let resolved = target
                .as_ref()
                .is_some_and(|t| matches!(&t.spec.metadata, MetaDataEntry::ClusterRegionRoute(_)));
            if !resolved {
                errors.push(ResolvedRefsError::PluginMetaDataNotFound {
                    namespace: cluster_ns.to_string(),
                    name: cref.name.clone(),
                    expected_type: "ClusterRegionRoute".to_string(),
                });
            }
        }
    }
    errors
}
