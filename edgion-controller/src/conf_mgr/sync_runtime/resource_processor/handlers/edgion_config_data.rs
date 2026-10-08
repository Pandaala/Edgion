//! EdgionConfigData Handler
//!
//! Handles EdgionConfigData resources.
//!
//! Responsibilities:
//! - Maintain EdgionConfigDataStore in ControllerRuntime (parse + on_delete)
//! - Register WafPolicy bundle references so child changes requeue the policy.
//! - On change: requeue referencing Gateways, plugins, and WafPolicy resources
//!   so they pick up updated data.
//! - On update_status: set `Accepted` (from preparse errors) and `ResolvedRefs`
//!   (from ref resolution) conditions per Gateway API conventions.

use crate::conf_mgr::sync_runtime::resource_processor::ref_grant::decide_edgion_config_data_ref;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, partially_invalid_condition, update_condition,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    EdgionConfigDataStore, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::observe::unknown_fields_registry::bounded_unknown_field_paths;
use edgion_resources::prelude_resources::EdgionConfigData;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigDataStatus};
use edgion_resources::resources::refs::waf_fields::{WafPolicyReferences, WafReference};
use edgion_resources::ResourceKind;

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
        format!("{}:{}", SELF_KIND, Self::resource_ref(ed).resource_key())
    }

    fn resource_ref(ed: &EdgionConfigData) -> ResourceRef {
        ResourceRef::new(
            ResourceKind::EdgionConfigData,
            ed.metadata.namespace.clone(),
            ed.metadata.name.clone().unwrap_or_default(),
        )
    }
}

fn register_policy_refs(ed: &EdgionConfigData, ctx: &HandlerContext) {
    let owner = EdgionConfigDataHandler::resource_ref(ed);
    ctx.runtime()
        .edgion_config_data_refs
        .clear_refs_from(&EdgionConfigDataHandler::self_ref_key(ed));
    ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&owner);
    let ConfigEntry::WafPolicy(policy) = &ed.spec.data else {
        return;
    };
    if policy.validate_shape().is_err() {
        return;
    }
    let Ok(fields) = WafPolicyReferences::from_policy(ed) else {
        return;
    };
    let namespace = fields.identity().scope().namespace().expect("namespaced owner");
    let own_key = EdgionConfigDataStore::make_key(Some(namespace), fields.identity().name());
    for (_, _, _, field) in fields.fields() {
        let Ok(field) = field else {
            continue;
        };
        let target_ns = field.target().scope().namespace().expect("namespaced target");
        let target_key = field.key();
        if target_key != own_key {
            ctx.runtime()
                .edgion_config_data_refs
                .add_ref(target_key, EdgionConfigDataHandler::self_ref_key(ed));
        }
        if target_ns != namespace {
            ctx.runtime()
                .cross_ns_ref_mgr
                .add_ref(target_ns.to_string(), owner.clone());
        }
    }
}

fn policy_ref_error(field: &WafReference<'_>, expected_digest: Option<&str>, ctx: &HandlerContext) -> Option<String> {
    let target_ns = field.target().scope().namespace().expect("namespaced target");
    let name = field.target().name();
    // Names are bounded here because they originate in operator-authored data.
    let label = format!(
        "{}/{}",
        target_ns.chars().take(64).collect::<String>(),
        name.chars().take(64).collect::<String>()
    );
    let Some(target) = ctx.runtime().edgion_config_data_store.get(Some(target_ns), name) else {
        return Some(format!("WafRuleBundle {label} is unavailable"));
    };
    if !target.spec.enable {
        return Some(format!("WafRuleBundle {label} is disabled"));
    }
    if !matches!(&target.spec.data, ConfigEntry::WafRuleBundle(_)) {
        return Some(format!("WafRuleBundle {label} has the wrong type"));
    }
    let query = field.query();
    if !decide_edgion_config_data_ref(
        crate::config::is_reference_grant_validation_enabled(),
        query.source().scope() == query.target().scope(),
        target.spec.visibility,
        ctx.runtime().ref_grant_store.check_query(query),
    ) {
        return Some(format!("WafRuleBundle {label} is not permitted"));
    }
    if !target.validate_pre_parse().is_empty() {
        return Some(format!("WafRuleBundle {label} is invalid"));
    }
    match &target.spec.data {
        ConfigEntry::WafRuleBundle(bundle) => {
            if bundle.validate().is_err() {
                return Some(format!("WafRuleBundle {label} is invalid"));
            }
            if expected_digest.is_some_and(|digest| digest != bundle.compute_digest()) {
                return Some(format!("WafRuleBundle {label} has a digest mismatch"));
            }
        }
        _ => unreachable!(),
    }
    None
}

fn bounded_ref_errors(errors: &[String]) -> String {
    let mut message = errors.iter().take(8).cloned().collect::<Vec<_>>().join("; ");
    if errors.len() > 8 {
        message.push_str(&format!("; {} more reference errors", errors.len() - 8));
    }
    message
}

impl Default for EdgionConfigDataHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Requeue resources that reference this EdgionConfigData.
///
/// Every registered consumer is requeued so it picks up changed data.
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
            if matches!(
                ed.spec.data,
                ConfigEntry::ProxyProtocolTrust(_) | ConfigEntry::WafPolicy(_)
            ) && ed
                .spec
                .unknown_fields
                .iter()
                .any(|path| path == "data.config" || path.starts_with("data.config."))
            {
                tracing::warn!(
                    component = "conf_mgr",
                    event = "invalid_security_config_fields_detected",
                    kind = "EdgionConfigData",
                    namespace = %ed.metadata.namespace.as_deref().unwrap_or("default"),
                    name = %ed.metadata.name.as_deref().unwrap_or(""),
                    paths = ?paths,
                    "Security-sensitive ConfigData contains unknown fields and is rejected"
                );
            } else {
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
        }
        ed.validate_pre_parse()
    }

    async fn parse(&self, ed: EdgionConfigData, ctx: &HandlerContext) -> ProcessResult<EdgionConfigData> {
        register_policy_refs(&ed, ctx);
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

        // Clear outgoing policy refs, then requeue all resources that referenced this object.
        ctx.runtime()
            .edgion_config_data_refs
            .clear_refs_from(&EdgionConfigDataHandler::self_ref_key(ed));
        ctx.runtime()
            .cross_ns_ref_mgr
            .clear_value_refs(&EdgionConfigDataHandler::resource_ref(ed));
        requeue_edgion_config_data_referrers(ed, ctx).await;
        // Keep incoming refs: a deleted child may be recreated without its
        // parent changing, and that create must requeue the parent again.
    }

    fn update_status(&self, ed: &mut EdgionConfigData, ctx: &HandlerContext, validation_errors: &[String]) {
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

        let mut required_errors = Vec::new();
        let mut optional_errors = Vec::new();
        if let ConfigEntry::WafPolicy(policy) = &ed.spec.data {
            if policy.validate_shape().is_ok() {
                match WafPolicyReferences::from_policy(ed) {
                    Err(error) => required_errors.push(format!("Invalid WafPolicy owner: {error}")),
                    Ok(fields) => {
                        for (_, _, reference, field) in fields.fields() {
                            let error = match field {
                                Ok(field) => policy_ref_error(&field, reference.expected_digest.as_deref(), ctx),
                                Err(error) => Some(format!("Invalid WafRuleBundle reference: {error}")),
                            };
                            if let Some(error) = error {
                                if reference.optional {
                                    optional_errors.push(error);
                                } else {
                                    required_errors.push(error);
                                }
                            }
                        }
                    }
                }
            }
        }
        let resolved_refs = if required_errors.is_empty() {
            condition_true(
                condition_types::RESOLVED_REFS,
                condition_reasons::RESOLVED_REFS,
                "All required references resolved",
                generation,
            )
        } else {
            let reason = if required_errors.iter().any(|error| error.ends_with("is not permitted")) {
                condition_reasons::REF_NOT_PERMITTED
            } else {
                condition_reasons::INVALID_EDGION_CONFIG_DATA_REF
            };
            condition_false(
                condition_types::RESOLVED_REFS,
                reason,
                bounded_ref_errors(&required_errors),
                generation,
            )
        };

        // IpList reports invalid entries; WafPolicy reports skipped optional
        // references. Other variants have no partial-good
        // status.
        let partially_invalid = if let ConfigEntry::IpList(ip_list) = &ed.spec.data {
            let entry_errors = edgion_resources::matcher::ip_cidr_helpers::collect_entry_errors(&ip_list.items);
            if entry_errors.is_empty() {
                Some(partially_invalid_condition(None, generation))
            } else {
                let msg = edgion_resources::matcher::ip_cidr_helpers::format_entry_errors(&entry_errors);
                Some(partially_invalid_condition(Some(msg), generation))
            }
        } else if matches!(ed.spec.data, ConfigEntry::WafPolicy(_)) {
            Some(partially_invalid_condition(
                (!optional_errors.is_empty()).then(|| bounded_ref_errors(&optional_errors)),
                generation,
            ))
        } else {
            None
        };
        let status = ed.status.get_or_insert_with(EdgionConfigDataStatus::default);
        update_condition(&mut status.conditions, accepted);
        update_condition(&mut status.conditions, resolved_refs);
        if let Some(partially_invalid) = partially_invalid {
            update_condition(&mut status.conditions, partially_invalid);
        } else {
            status
                .conditions
                .retain(|condition| condition.type_ != condition_types::PARTIALLY_INVALID);
        }
        super::super::unknown_fields_stamp::apply_unknown_fields_condition(
            &mut status.conditions,
            &ed.spec.unknown_fields,
            generation,
        );
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_config_data_test/unknown_fields_tests.rs"]
mod unknown_fields_tests;

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_config_data_test/dependency_requeue_tests.rs"]
mod dependency_requeue_tests;
