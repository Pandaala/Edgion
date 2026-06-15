//! EdgionData Handler
//!
//! Handles EdgionData resources.
//!
//! Responsibilities:
//! - Maintain EdgionDataStore in ControllerRuntime (parse + on_delete)
//! - Register cross-EdgionData references (override_ref for RegionRouteOverride / GlobalConnIpRestriction)
//!   so a change to a referenced EdgionData requeues the dependent EdgionPlugins/EdgionStreamPlugins.
//! - On change: requeue referencing EdgionPlugins/EdgionStreamPlugins so they pick up updated overlay data.
//! - On update_status: set `Accepted` (from preparse errors) and `ResolvedRefs`
//!   (from override_ref resolution) conditions per Gateway API conventions.

use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, update_condition, ResolvedRefsError,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, PluginMetaDataStore, ProcessResult, ProcessorHandler,
};
use edgion_resources::prelude_resources::EdgionData;
use edgion_resources::resources::data::EdgionDataStatus;

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

/// Requeue resources that reference this EdgionData.
///
/// EdgionPlugins / EdgionStreamPlugins referencing this EdgionData via `overrideRef` or
/// `pluginMetadataRef` are requeued so they pick up any changed overlay data.
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

        // Clear any outgoing refs this EdgionData registered (none currently for data-side),
        // then requeue all resources that referenced this EdgionData.
        ctx.runtime()
            .plugin_metadata_refs
            .clear_refs_from(&DataHandler::self_ref_key(ed));
        requeue_pm_referrers(ed, ctx).await;
        ctx.runtime().plugin_metadata_refs.remove_pm(&key);
    }

    fn update_status(&self, ed: &mut EdgionData, _ctx: &HandlerContext, validation_errors: &[String]) {
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

        // ResolvedRefs: EdgionData resources generally carry no cross-resource refs.
        // All variants resolve to True (no external references to validate at the EdgionData level).
        // Consumers (EdgionPlugins/EdgionStreamPlugins) validate their `overrideRef` in their own handlers.
        let resolved_refs = condition_true(
            condition_types::RESOLVED_REFS,
            condition_reasons::RESOLVED_REFS,
            "All references resolved",
            generation,
        );

        let status = ed.status.get_or_insert_with(EdgionDataStatus::default);
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
    }
}

/// Resolve cross-EdgionData references for update_status.
///
/// After S3: EdgionData resources no longer carry external refs (the old
/// `ServiceRegionRoute.clusterRef` chain is gone). Always returns empty.
#[allow(dead_code)]
fn resolve_refs(_ed: &EdgionData, _ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    vec![]
}
