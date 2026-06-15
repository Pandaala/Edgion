//! EdgionData Handler
//!
//! Handles EdgionData resources.
//!
//! Responsibilities:
//! - Maintain EdgionDataStore in ControllerRuntime (parse + on_delete)
//! - Maintain RegionRoute tables (global + service) on change/delete
//! - Register cross-EdgionData references (ServiceRegionRoute.clusterRef → ClusterRegionRoute EdgionData)
//!   so a change to a ClusterRegionRoute EdgionData requeues the dependent ServiceRegionRoute EdgionData.
//! - On change: requeue referencing EdgionPlugins/EdgionStreamPlugins (and dependent
//!   ServiceRegionRoute EdgionData) so they pick up updated failover / match_map.
//! - On update_status: set `Accepted` (from preparse errors) and `ResolvedRefs`
//!   (from ServiceRegionRoute.clusterRef resolution) conditions per Gateway API conventions.

use crate::conf_mgr::sync_runtime::resource_processor::region_route_table::{
    ClusterRegionRouteEntry, ServiceRegionRouteEntry,
};
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, update_condition, ResolvedRefsError,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, PluginMetaDataStore, ProcessResult, ProcessorHandler,
};
use edgion_resources::prelude_resources::EdgionData;
use edgion_resources::resources::data::{DataEntry, EdgionDataStatus};

/// Kind string used when an EdgionData registers a cross-EdgionData reference in the shared
/// `plugin_metadata_refs` manager. The existing `requeue_pm_referrers` splits refs on `:`
/// and uses the prefix as the kind for `ctx.requeue`, which matches the EdgionData
/// processor registered in the processor registry.
const SELF_KIND: &str = "EdgionData";

pub struct DataHandler;

impl DataHandler {
    pub fn new() -> Self {
        Self
    }

    fn self_ref_key(ed: &EdgionData) -> String {
        let ns = ed.metadata.namespace.as_deref().unwrap_or("");
        let name = ed.metadata.name.as_deref().unwrap_or("");
        format!("{}:{}/{}", SELF_KIND, ns, name)
    }
}

impl Default for DataHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Update the RegionRoute tables based on EdgionData content.
fn update_region_route_tables(ed: &EdgionData, ctx: &HandlerContext) {
    let ns = ed.metadata.namespace.as_deref().unwrap_or("");
    let name = ed.metadata.name.as_deref().unwrap_or("");
    let pm_key = PluginMetaDataStore::make_key(Some(ns), name);

    match &ed.spec.data {
        DataEntry::ClusterRegionRoute(cfg) => {
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
        DataEntry::ServiceRegionRoute(cfg) => {
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

/// Remove entries from RegionRoute tables for a deleted EdgionData.
/// Unconditionally removes from both tables — the EdgionData's spec may have been
/// modified before deletion, so we can't rely on its current plugin list.
fn remove_from_region_route_tables(ed: &EdgionData, ctx: &HandlerContext) {
    let pm_key = PluginMetaDataStore::make_key(
        ed.metadata.namespace.as_deref(),
        ed.metadata.name.as_deref().unwrap_or(""),
    );
    ctx.runtime().cluster_region_route_table.remove(&pm_key);
    ctx.runtime().service_region_route_table.remove(&pm_key);
}

/// Register this EdgionData's cross-EdgionData references in the shared `plugin_metadata_refs` manager.
///
/// Currently only `ServiceRegionRoute.clusterRef → ClusterRegionRoute EdgionData` is tracked.
/// Entry stored: `cluster_pm_key -> {"EdgionData:ns/service_pm_name", ...}`.
/// When the cluster EdgionData changes, `requeue_pm_referrers` picks the service EdgionData back up
/// and re-runs its `update_status`, refreshing `ResolvedRefs`.
fn register_cross_pm_refs(ed: &EdgionData, ctx: &HandlerContext) {
    if let DataEntry::ServiceRegionRoute(cfg) = &ed.spec.data {
        if let Some(cref) = &cfg.cluster_ref {
            let pm_ns = ed.metadata.namespace.as_deref().unwrap_or("");
            let cluster_ns = cref.namespace.as_deref().unwrap_or(pm_ns);
            let cluster_pm_key = format!("{}/{}", cluster_ns, cref.name);
            ctx.runtime()
                .plugin_metadata_refs
                .add_ref(cluster_pm_key, DataHandler::self_ref_key(ed));
        }
    }
}

/// Requeue resources that reference this EdgionData.
///
/// Historically: EdgionPlugins / EdgionStreamPlugins referencing regions, etc.
/// Newly: other EdgionData (ServiceRegionRoute whose clusterRef points here).
async fn requeue_pm_referrers(ed: &EdgionData, ctx: &HandlerContext) {
    let ns = ed.metadata.namespace.as_deref().unwrap_or("");
    let name = ed.metadata.name.as_deref().unwrap_or("");
    let pm_key = PluginMetaDataStore::make_key(Some(ns), name);

    let refs = ctx.runtime().plugin_metadata_refs.get_refs(&pm_key);
    for ep_key in refs {
        if let Some((kind_str, key)) = ep_key.split_once(':') {
            ctx.requeue(kind_str, key.to_string()).await;
        }
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionData> for DataHandler {
    fn preparse(&self, ed: &mut EdgionData, _ctx: &HandlerContext) -> Vec<String> {
        ed.validate_pre_parse()
    }

    async fn parse(&self, ed: EdgionData, ctx: &HandlerContext) -> ProcessResult<EdgionData> {
        let key = PluginMetaDataStore::make_key(
            ed.metadata.namespace.as_deref(),
            ed.metadata.name.as_deref().unwrap_or(""),
        );
        ctx.runtime()
            .plugin_metadata_store
            .update([(key, ed.clone())].into_iter().collect(), &Default::default());

        update_region_route_tables(&ed, ctx);

        // Rebuild this EdgionData's outgoing cross-EdgionData refs from scratch (covers the case
        // where a ServiceRegionRoute's clusterRef was edited between revisions).
        ctx.runtime()
            .plugin_metadata_refs
            .clear_refs_from(&Self::self_ref_key(&ed));
        register_cross_pm_refs(&ed, ctx);

        ProcessResult::Continue(ed)
    }

    async fn on_change(&self, ed: &EdgionData, ctx: &HandlerContext) {
        requeue_pm_referrers(ed, ctx).await;
    }

    async fn on_delete(&self, ed: &EdgionData, ctx: &HandlerContext) {
        let key = PluginMetaDataStore::make_key(
            ed.metadata.namespace.as_deref(),
            ed.metadata.name.as_deref().unwrap_or(""),
        );
        ctx.runtime()
            .plugin_metadata_store
            .update(Default::default(), &[key.clone()].into_iter().collect());

        remove_from_region_route_tables(ed, ctx);
        ctx.runtime()
            .plugin_metadata_refs
            .clear_refs_from(&Self::self_ref_key(ed));
        requeue_pm_referrers(ed, ctx).await;
        ctx.runtime().plugin_metadata_refs.remove_pm(&key);
    }

    fn update_status(&self, ed: &mut EdgionData, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ed.metadata.generation;

        // Compute both conditions first (both need immutable access to `ed` / `ctx`),
        // then apply them in a single mutable-borrow scope on `ed.status`.
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

        let resolved_errors = resolve_refs(ed, ctx);
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

        let status = ed.status.get_or_insert_with(EdgionDataStatus::default);
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
    }
}

/// Resolve cross-EdgionData references.
///
/// Today: only `ServiceRegionRoute.clusterRef` must point at an existing
/// `ClusterRegionRoute` EdgionData. Returns an empty vec for variants without
/// external refs.
fn resolve_refs(ed: &EdgionData, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    if let DataEntry::ServiceRegionRoute(cfg) = &ed.spec.data {
        if let Some(cref) = &cfg.cluster_ref {
            let pm_ns = ed.metadata.namespace.as_deref().unwrap_or("");
            let cluster_ns = cref.namespace.as_deref().unwrap_or(pm_ns);
            let target = ctx.runtime().plugin_metadata_store.get(Some(cluster_ns), &cref.name);
            let resolved = target
                .as_ref()
                .is_some_and(|t| matches!(&t.spec.data, DataEntry::ClusterRegionRoute(_)));
            if !resolved {
                errors.push(ResolvedRefsError::EdgionDataNotFound {
                    namespace: cluster_ns.to_string(),
                    name: cref.name.clone(),
                    expected_type: "ClusterRegionRoute".to_string(),
                });
            }
        }
    }
    errors
}
