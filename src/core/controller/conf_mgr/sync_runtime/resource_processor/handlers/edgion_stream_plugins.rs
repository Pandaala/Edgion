//! EdgionStreamPlugins Handler
//!
//! Handles EdgionStreamPlugins resources with Gateway API standard status management.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_reasons, ResolvedRefsError,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_types, HandlerContext, PluginMetaDataStore, ProcessResult, ProcessorHandler,
};
use crate::types::prelude_resources::{EdgionStreamPlugins, PluginMetaData};
use crate::types::resources::edgion_stream_plugins::{EdgionStreamPlugin, EdgionStreamPluginsStatus};
use crate::types::resources::plugin_metadata::MetaDataEntry;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use k8s_openapi::chrono::Utc;

const SELF_KIND: &str = "EdgionStreamPlugins";

/// Prefix used to mark preparse errors that indicate a legacy-schema upgrade issue.
/// When present, `update_status` maps the condition reason to `LegacySchemaDeprecated`
/// (instead of the generic `Invalid`) so operators can grep status and find migration hints.
const LEGACY_SCHEMA_MARKER: &str = "[LEGACY_SCHEMA_DEPRECATED] ";

/// Condition reason emitted when an upgrade path collides with a breaking schema change.
const REASON_LEGACY_SCHEMA_DEPRECATED: &str = "LegacySchemaDeprecated";

/// Single source of truth for the migration doc path referenced from status conditions.
/// Keep this in sync with the actual file location; renaming the doc without updating this
/// constant leaves operators clicking a broken link.
const MIGRATION_DOC_PATH: &str = "docs/migrations/ec4e000-ip-restriction-stream-schema.md";

/// EdgionStreamPlugins handler
pub struct EdgionStreamPluginsHandler;

impl EdgionStreamPluginsHandler {
    pub fn new() -> Self {
        Self
    }

    fn self_ref_key(esp: &EdgionStreamPlugins) -> String {
        let ns = esp.metadata.namespace.as_deref().unwrap_or("");
        let name = esp.metadata.name.as_deref().unwrap_or("");
        format!("{}:{}/{}", SELF_KIND, ns, name)
    }

    /// Iterate each GlobalConnectionIpRestriction plugin and yield (pm_namespace, pm_name).
    fn iter_gcir_refs<'a>(esp: &'a EdgionStreamPlugins) -> impl Iterator<Item = (String, String)> + 'a {
        let esp_ns = esp.metadata.namespace.as_deref().unwrap_or("").to_string();
        esp.spec
            .plugins
            .as_ref()
            .into_iter()
            .flat_map(|v| v.iter())
            .filter_map(move |entry| match &entry.plugin {
                EdgionStreamPlugin::GlobalConnectionIpRestriction(gcir) => {
                    let pm_ns = gcir
                        .plugin_metadata_ref
                        .namespace
                        .clone()
                        .unwrap_or_else(|| esp_ns.clone());
                    Some((pm_ns, gcir.plugin_metadata_ref.name.clone()))
                }
                EdgionStreamPlugin::IpRestriction(_) => None,
            })
    }
}

impl Default for EdgionStreamPluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionStreamPlugins> for EdgionStreamPluginsHandler {
    fn preparse(&self, esp: &mut EdgionStreamPlugins, _ctx: &HandlerContext) -> Vec<String> {
        let Some(plugins) = esp.spec.plugins.as_ref() else {
            return Vec::new();
        };

        let mut errors = Vec::new();
        for (idx, entry) in plugins.iter().enumerate() {
            if let EdgionStreamPlugin::IpRestriction(rules) = &entry.plugin {
                let legacy = rules.legacy_http_fields();
                if !legacy.is_empty() {
                    errors.push(format!(
                        "{}IpRestriction[{}]: legacy HTTP fields {:?} detected on L4 config; \
                         remove them and use only allow/deny/defaultAction (see {})",
                        LEGACY_SCHEMA_MARKER, idx, legacy, MIGRATION_DOC_PATH
                    ));
                }
            }
        }
        errors
    }

    async fn parse(&self, esp: EdgionStreamPlugins, ctx: &HandlerContext) -> ProcessResult<EdgionStreamPlugins> {
        // Rebuild outgoing PluginMetaData refs from scratch so that an edit that removes
        // a GCIR entry correctly drops its old ref.
        let self_ref = Self::self_ref_key(&esp);
        ctx.runtime().plugin_metadata_refs.clear_refs_from(&self_ref);
        for (pm_ns, pm_name) in Self::iter_gcir_refs(&esp) {
            let pm_key = PluginMetaDataStore::make_key(Some(&pm_ns), &pm_name);
            ctx.runtime().plugin_metadata_refs.add_ref(pm_key, self_ref.clone());
        }
        ProcessResult::Continue(esp)
    }

    async fn on_delete(&self, esp: &EdgionStreamPlugins, ctx: &HandlerContext) {
        ctx.runtime()
            .plugin_metadata_refs
            .clear_refs_from(&Self::self_ref_key(esp));
    }

    fn update_status(&self, esp: &mut EdgionStreamPlugins, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = esp.metadata.generation;

        // Accepted: preparse/validate errors (with legacy-schema reason mapping).
        let accepted = if validation_errors.is_empty() {
            k8s_condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Resource accepted",
                generation,
            )
        } else {
            let has_legacy = validation_errors.iter().any(|e| e.starts_with(LEGACY_SCHEMA_MARKER));
            let reason = if has_legacy {
                REASON_LEGACY_SCHEMA_DEPRECATED
            } else {
                condition_reasons::INVALID
            };
            let message = validation_errors
                .iter()
                .map(|e| e.strip_prefix(LEGACY_SCHEMA_MARKER).unwrap_or(e))
                .collect::<Vec<_>>()
                .join("; ");
            k8s_condition_false(condition_types::ACCEPTED, reason, &message, generation)
        };

        // ResolvedRefs: each GlobalConnectionIpRestriction plugin_metadata_ref must
        // resolve to an existing PluginMetaData of the matching variant type.
        let resolved_errors = resolve_refs(esp, ctx);
        let resolved_refs = if resolved_errors.is_empty() {
            k8s_condition_true(
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
            k8s_condition_false(condition_types::RESOLVED_REFS, reason, &message, generation)
        };

        let status = esp
            .status
            .get_or_insert_with(|| EdgionStreamPluginsStatus { conditions: vec![] });
        update_k8s_condition(&mut status.conditions, accepted);
        update_k8s_condition(&mut status.conditions, resolved_refs);
    }
}

/// Resolve each GCIR plugin_metadata_ref against the PluginMetaData store.
/// Returns one `PluginMetaDataNotFound` error per unresolved / wrong-type reference.
fn resolve_refs(esp: &EdgionStreamPlugins, ctx: &HandlerContext) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    for (pm_ns, pm_name) in EdgionStreamPluginsHandler::iter_gcir_refs(esp) {
        let target: Option<PluginMetaData> = ctx.runtime().plugin_metadata_store.get(Some(&pm_ns), &pm_name);
        let resolved = target
            .as_ref()
            .is_some_and(|t| matches!(&t.spec.metadata, MetaDataEntry::GlobalConnectionIpRestriction(_)));
        if !resolved {
            errors.push(ResolvedRefsError::PluginMetaDataNotFound {
                namespace: pm_ns,
                name: pm_name,
                expected_type: "GlobalConnectionIpRestriction".to_string(),
            });
        }
    }
    errors
}

/// Create a k8s_openapi Condition with True status
fn k8s_condition_true(type_: &str, reason: &str, message: &str, observed_generation: Option<i64>) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "True".to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

/// Create a k8s_openapi Condition with False status
fn k8s_condition_false(type_: &str, reason: &str, message: &str, observed_generation: Option<i64>) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "False".to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

/// Update or insert a k8s_openapi Condition
fn update_k8s_condition(conditions: &mut Vec<Condition>, new_condition: Condition) {
    if let Some(existing) = conditions.iter_mut().find(|c| c.type_ == new_condition.type_) {
        let status_changed = existing.status != new_condition.status;
        existing.status = new_condition.status;
        existing.reason = new_condition.reason;
        existing.message = new_condition.message;
        existing.observed_generation = new_condition.observed_generation;
        if status_changed {
            existing.last_transition_time = new_condition.last_transition_time;
        }
    } else {
        conditions.push(new_condition);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::SecretRefManager;
    use crate::core::controller::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::core::controller::conf_mgr::ControllerRuntime;
    use crate::types::prelude_resources::EdgionStreamPlugins;
    use std::sync::Arc;

    fn make_ctx() -> HandlerContext {
        HandlerContext::new(
            Arc::new(SecretRefManager::new()),
            None,
            None,
            TriggerChain::default(),
            3,
            20,
            Arc::new(ControllerRuntime::default()),
        )
    }

    fn legacy_yaml_esp() -> EdgionStreamPlugins {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: legacy
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8"]
        ipSource: RemoteAddr
        message: blocked
        status: 403
"#;
        serde_yaml::from_str(yaml).expect("legacy yaml must now parse (detection moved to preparse)")
    }

    fn clean_esp() -> EdgionStreamPlugins {
        let yaml = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: clean
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8"]
        defaultAction: deny
"#;
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn preparse_emits_legacy_marker_when_old_fields_present() {
        let handler = EdgionStreamPluginsHandler::new();
        let mut esp = legacy_yaml_esp();
        let errors = handler.preparse(&mut esp, &make_ctx());
        assert_eq!(errors.len(), 1);
        let e = &errors[0];
        assert!(
            e.starts_with(LEGACY_SCHEMA_MARKER),
            "preparse error must carry LEGACY_SCHEMA_DEPRECATED marker so update_status maps the reason; got: {e}"
        );
        assert!(e.contains("ipSource"));
        assert!(e.contains("message"));
        assert!(e.contains("status"));
        assert!(e.contains(MIGRATION_DOC_PATH));
    }

    #[test]
    fn preparse_is_quiet_on_clean_config() {
        let handler = EdgionStreamPluginsHandler::new();
        let mut esp = clean_esp();
        let errors = handler.preparse(&mut esp, &make_ctx());
        assert!(
            errors.is_empty(),
            "clean config should not produce preparse errors; got: {:?}",
            errors
        );
    }

    #[test]
    fn update_status_maps_legacy_marker_to_dedicated_reason() {
        let handler = EdgionStreamPluginsHandler::new();
        let mut esp = legacy_yaml_esp();
        let errors = handler.preparse(&mut esp, &make_ctx());
        handler.update_status(&mut esp, &make_ctx(), &errors);

        let status = esp.status.as_ref().expect("status populated");
        let accepted = status
            .conditions
            .iter()
            .find(|c| c.type_ == "Accepted")
            .expect("Accepted condition present");
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, REASON_LEGACY_SCHEMA_DEPRECATED);
        // Message must not leak the internal marker prefix.
        assert!(!accepted.message.contains(LEGACY_SCHEMA_MARKER));
        assert!(accepted.message.contains("legacy HTTP fields"));
    }

    #[test]
    fn update_status_uses_invalid_reason_for_non_legacy_errors() {
        let handler = EdgionStreamPluginsHandler::new();
        let mut esp = clean_esp();
        let errors = vec!["something else went wrong".to_string()];
        handler.update_status(&mut esp, &make_ctx(), &errors);

        let status = esp.status.as_ref().unwrap();
        let accepted = status.conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.reason, "Invalid");
    }
}
