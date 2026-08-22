//! EdgionConfigData Handler
//!
//! Handles EdgionConfigData resources.
//!
//! Responsibilities:
//! - Maintain EdgionConfigDataStore in ControllerRuntime (parse + on_delete)
//! - Register cross-EdgionConfigData references (overrideRef → RegionRouteOverride for EdgionPlugins;
//!   activeProfileRef → Selector for EdgionStreamPlugins GlobalConnectionIpRestriction)
//!   so a change to a referenced EdgionConfigData requeues the dependent EdgionPlugins/EdgionStreamPlugins.
//! - On change: requeue referencing EdgionPlugins/EdgionStreamPlugins so they pick up updated data.
//! - On update_status: set `Accepted` (from preparse errors) and `ResolvedRefs`
//!   (from ref resolution) conditions per Gateway API conventions.

use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, partially_invalid_condition, update_condition,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    EdgionConfigDataStore, HandlerContext, ProcessResult, ProcessorHandler,
};
use crate::observe::unknown_fields_registry::bounded_unknown_field_paths;
use edgion_resources::prelude_resources::EdgionConfigData;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigDataStatus};

/// Kind string used when an EdgionConfigData registers a cross-EdgionConfigData reference in the shared
/// `edgion_config_data_refs` manager. The existing `requeue_edgion_config_data_referrers` splits refs on `:`
/// and uses the prefix as the kind for `ctx.requeue`, which matches the EdgionConfigData
/// processor registered in the processor registry.
const SELF_KIND: &str = "EdgionConfigData";

pub struct EdgionConfigDataHandler;

impl EdgionConfigDataHandler {
    pub fn new() -> Self {
        Self
    }

    fn self_ref_key(ed: &EdgionConfigData) -> String {
        let ns = ed.metadata.namespace.as_deref().unwrap_or("");
        let name = ed.metadata.name.as_deref().unwrap_or("");
        format!("{}:{}/{}", SELF_KIND, ns, name)
    }
}

impl Default for EdgionConfigDataHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Requeue resources that reference this EdgionConfigData.
///
/// EdgionPlugins (via `overrideRef`) / EdgionStreamPlugins (via `activeProfileRef`) referencing this
/// EdgionConfigData are requeued so they pick up any changed data.
async fn requeue_edgion_config_data_referrers(ed: &EdgionConfigData, ctx: &HandlerContext) {
    let ns = ed.metadata.namespace.as_deref().unwrap_or("");
    let name = ed.metadata.name.as_deref().unwrap_or("");
    let ed_key = EdgionConfigDataStore::make_key(Some(ns), name);

    let refs = ctx.runtime().edgion_config_data_refs.get_refs(&ed_key);
    for ep_key in refs {
        if let Some((kind_str, key)) = ep_key.split_once(':') {
            ctx.requeue(kind_str, key.to_string()).await;
        }
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionConfigData> for EdgionConfigDataHandler {
    fn preparse(&self, ed: &mut EdgionConfigData, _ctx: &HandlerContext) -> Vec<String> {
        if !ed.spec.unknown_fields.is_empty() {
            let paths = bounded_unknown_field_paths(&ed.spec.unknown_fields);
            tracing::warn!(
                component = "conf_mgr",
                event = "unknown_config_fields_detected",
                kind = "EdgionConfigData",
                namespace = %ed.metadata.namespace.as_deref().unwrap_or("default"),
                name = %ed.metadata.name.as_deref().unwrap_or(""),
                paths = ?paths,
                "EdgionConfigData document has keys the typed schema does not recognize; they are ignored and config still syncs"
            );
        }
        ed.validate_pre_parse()
    }

    async fn parse(&self, ed: EdgionConfigData, ctx: &HandlerContext) -> ProcessResult<EdgionConfigData> {
        let key = EdgionConfigDataStore::make_key(
            ed.metadata.namespace.as_deref(),
            ed.metadata.name.as_deref().unwrap_or(""),
        );
        ctx.runtime()
            .edgion_config_data_store
            .update([(key, ed.clone())].into_iter().collect(), &Default::default());

        ProcessResult::Continue(ed)
    }

    async fn on_change(&self, ed: &EdgionConfigData, ctx: &HandlerContext) {
        requeue_edgion_config_data_referrers(ed, ctx).await;
    }

    async fn on_delete(&self, ed: &EdgionConfigData, ctx: &HandlerContext) {
        let key = EdgionConfigDataStore::make_key(
            ed.metadata.namespace.as_deref(),
            ed.metadata.name.as_deref().unwrap_or(""),
        );
        ctx.runtime()
            .edgion_config_data_store
            .update(Default::default(), &[key.clone()].into_iter().collect());

        // Clear any outgoing refs this EdgionConfigData registered (none currently for data-side),
        // then requeue all resources that referenced this EdgionConfigData.
        ctx.runtime()
            .edgion_config_data_refs
            .clear_refs_from(&EdgionConfigDataHandler::self_ref_key(ed));
        requeue_edgion_config_data_referrers(ed, ctx).await;
        ctx.runtime().edgion_config_data_refs.remove_edgion_config_data(&key);
    }

    fn update_status(&self, ed: &mut EdgionConfigData, _ctx: &HandlerContext, validation_errors: &[String]) {
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

        // ResolvedRefs: EdgionConfigData resources generally carry no cross-resource refs.
        // All variants resolve to True (no external references to validate at the EdgionConfigData level).
        // Consumers (EdgionPlugins/EdgionStreamPlugins) validate their `overrideRef` in their own handlers.
        let resolved_refs = condition_true(
            condition_types::RESOLVED_REFS,
            condition_reasons::RESOLVED_REFS,
            "All references resolved",
            generation,
        );

        // PartiallyInvalid: IpList ONLY (partial-good compilation applies to nothing else).
        // For an IpList we list every skipped/invalid entry; Accepted stays True and this
        // condition is the operator's signal that some entries dropped. For non-IpList
        // variants (KeyList/Selector/RegionRouteOverride) we write NO PartiallyInvalid
        // condition at all — there is no partial-good notion for them.
        let partially_invalid = if let ConfigEntry::IpList(ip_list) = &ed.spec.data {
            let entry_errors = edgion_resources::matcher::ip_cidr_helpers::collect_entry_errors(&ip_list.items);
            if entry_errors.is_empty() {
                Some(partially_invalid_condition(None, generation))
            } else {
                let msg = edgion_resources::matcher::ip_cidr_helpers::format_entry_errors(&entry_errors);
                Some(partially_invalid_condition(Some(msg), generation))
            }
        } else {
            None
        };
        let status = ed.status.get_or_insert_with(EdgionConfigDataStatus::default);
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
        if let Some(partially_invalid) = partially_invalid {
            update_condition(&mut status.conditions, partially_invalid);
        }
        super::super::unknown_fields_stamp::apply_unknown_fields_condition(
            &mut status.conditions,
            &ed.spec.unknown_fields,
            generation,
        );
    }
}

#[cfg(test)]
mod unknown_fields_tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
        condition_reasons, condition_true, condition_types,
    };
    use edgion_resources::resources::edgion_config_data::EdgionConfigDataSpec;
    use std::sync::Arc;

    fn resource(paths: Vec<String>) -> EdgionConfigData {
        let spec: EdgionConfigDataSpec = serde_json::from_value(serde_json::json!({
            "data": {"type": "Misc", "config": {}}
        }))
        .expect("minimal ConfigData spec");
        let mut resource = EdgionConfigData::new("config", spec);
        resource.metadata.namespace = Some("default".to_string());
        resource.metadata.generation = Some(7);
        resource.spec.unknown_fields = paths;
        resource
    }

    #[test]
    fn update_status_adds_and_clears_advisory_without_disturbing_other_conditions() {
        let handler = EdgionConfigDataHandler::new();
        let mut resource = resource(vec!["data.config.typo".to_string()]);
        let ctx = HandlerContext::new(
            None,
            None,
            Default::default(),
            5,
            20,
            Arc::new(crate::conf_mgr::ControllerRuntime::default()),
        );

        handler.update_status(&mut resource, &ctx, &[]);
        let conditions = &resource.status.as_ref().expect("status").conditions;
        assert!(conditions.iter().any(|condition| {
            condition.type_ == condition_types::UNKNOWN_CONFIG_FIELDS
                && condition.reason == condition_reasons::UNKNOWN_FIELDS_DETECTED
                && condition.status == "True"
        }));
        assert!(conditions
            .iter()
            .any(|condition| condition.type_ == condition_types::ACCEPTED && condition.status == "True"));

        resource.spec.unknown_fields.clear();
        resource
            .status
            .as_mut()
            .expect("status")
            .conditions
            .push(condition_true("Unrelated", "Stable", "keep", Some(7)));
        handler.update_status(&mut resource, &ctx, &[]);
        let conditions = &resource.status.as_ref().expect("status").conditions;
        assert!(conditions
            .iter()
            .all(|condition| condition.type_ != condition_types::UNKNOWN_CONFIG_FIELDS));
        assert!(conditions.iter().any(|condition| condition.type_ == "Unrelated"));
    }
}

#[cfg(test)]
mod dependency_requeue_tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::ResourceProcessor;
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::prelude_resources::EdgionPlugins;
    use edgion_resources::resources::edgion_config_data::EdgionConfigDataSpec;
    use std::sync::Arc;
    use std::time::Duration;

    struct PluginsNoop;

    #[async_trait::async_trait]
    impl ProcessorHandler<EdgionPlugins> for PluginsNoop {}

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
}
