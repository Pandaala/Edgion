//! Controller-side RequestAccessPolicy ConfigData sidecar expansion.
//!
//! Pass 1 of the Condition-tree resolver. Writes `resolved_profiles` /
//! `resolved_candidates` only. IP-list `ipRefs` and Secret/HMAC credentials
//! are filled by the generic walker in `condition_tree` — this module must
//! not walk those leaves itself.

use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::refs::config_data_fields::{ConfigDataField, HttpConfigDataEntry};
use edgion_resources::refs::ReferenceError;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    RequestAccessRuleType, UrlAccessCandidate, UrlAccessRuleConfig,
};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::ResourceKind;

use crate::conf_mgr::sync_runtime::resource_processor::handlers::config_data_ref_mark::resolve_field;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};

const MAX_RESOLUTION_WARNINGS: usize = 64;

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn expand(
    ep: &mut EdgionPlugins,
    ctx: &HandlerContext,
) {
    let resource_ref = ResourceRef::new(
        ResourceKind::EdgionPlugins,
        ep.metadata.namespace.clone(),
        ep.metadata.name.clone().unwrap_or_default(),
    );
    let owner_key = format!("{}:{}", resource_ref.kind_str(), resource_ref.resource_key());
    let count = ep.spec.request_plugins.as_ref().map_or(0, Vec::len);
    for index in 0..count {
        if !ep.spec.request_plugins.as_ref().expect("current entry")[index].is_enabled() {
            continue;
        }
        let Some(entry) = HttpConfigDataEntry::from_plugins(ep, index) else {
            continue;
        };
        let EdgionPlugin::RequestAccessPolicy(config) = entry.plugin() else {
            continue;
        };
        // This is the existing one authored-profile copy. Imported candidates
        // are copied only after the actual URL field passes resolution.
        let owner_validation = entry.validate_owner();
        let mut profiles = config.profiles.clone();
        let mut warnings = Vec::new();
        for (name, profile) in &mut profiles {
            for (group_index, group) in profile.required_rule_groups.iter_mut().enumerate() {
                for (rule_index, rule) in group.any_of_rules.iter_mut().enumerate() {
                    if let RequestAccessRuleType::Url(url) = &mut rule.rule {
                        expand_url_rule(
                            &rule.name,
                            url,
                            |reference| {
                                owner_validation?;
                                entry
                                    .url_field(name, group_index, rule_index, reference)
                                    .ok_or(ReferenceError::StaleBinding)
                            },
                            &owner_key,
                            &resource_ref,
                            ctx,
                            &mut warnings,
                        );
                    }
                }
            }
        }
        let EdgionPlugin::RequestAccessPolicy(config) =
            &mut ep.spec.request_plugins.as_mut().expect("current entry")[index].plugin
        else {
            unreachable!("actual entry")
        };
        config.resolved_profiles = Some(profiles);
        config.resolution_warnings = warnings;
    }
}

fn expand_url_rule<'a>(
    rule_name: &str,
    config: &mut UrlAccessRuleConfig,
    field_at: impl Fn(usize) -> Result<ConfigDataField<'a>, ReferenceError>,
    owner_key: &str,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
    warnings: &mut Vec<String>,
) {
    let mut candidates = Vec::new();
    if !config.hosts.is_empty() || !config.paths.is_empty() {
        candidates.push(UrlAccessCandidate {
            name: rule_name.to_string(),
            hosts: config.hosts.clone(),
            paths: config.paths.clone(),
            conditions: config.conditions.clone(),
        });
    }

    for (index, reference) in config.config_refs.iter_mut().enumerate() {
        let data_ref = &mut reference.reference;
        data_ref.ref_denied = None;
        let field = match field_at(index) {
            Ok(field) => field,
            Err(error) => {
                push_warning(
                    warnings,
                    format!("RequestAccessPolicy URL data ref is invalid: {error}"),
                );
                continue;
            }
        };
        let query = match field.query() {
            Ok(query) => query,
            Err(error) => {
                push_warning(
                    warnings,
                    format!("RequestAccessPolicy URL data ref is invalid: {error}"),
                );
                continue;
            }
        };
        register_ref(&field, owner_key, resource_ref, ctx);
        let target_namespace = query.target().scope().namespace().expect("namespaced target");
        let resolution = resolve_field(&field, ctx);
        data_ref.ref_denied = resolution.denied;
        let Some(target) = resolution.target else {
            push_warning(
                warnings,
                format!(
                    "RequestAccessPolicy URL data ref {target_namespace}/{} {}",
                    query.target().name(),
                    resolution.failure
                ),
            );
            continue;
        };
        let ConfigEntry::RequestAccessUrlAllowList(data) = &target.spec.data else {
            unreachable!("fixed URL role")
        };
        let ip_rule_count = data.items.iter().map(UrlAccessCandidate::ip_rule_count).sum::<usize>();
        let invalid = data.items.len() > reference.validate.max_url_rules
            || ip_rule_count > reference.validate.max_ip_rules
            || data
                .items
                .iter()
                .any(|item| item.hosts.is_empty() && !reference.validate.allow_no_hosts);
        if invalid {
            push_warning(
                warnings,
                format!(
                    "RequestAccessPolicy URL data ref {target_namespace}/{} violates its validate limits",
                    data_ref.name
                ),
            );
            continue;
        }
        if let Err(error) = data.validate() {
            push_warning(
                warnings,
                format!(
                    "RequestAccessPolicy URL data ref {target_namespace}/{} is invalid: {error}",
                    data_ref.name
                ),
            );
            continue;
        }
        candidates.extend(data.items.clone());
    }
    config.resolved_candidates = Some(candidates);
}

fn register_ref(field: &ConfigDataField<'_>, owner_key: &str, resource_ref: &ResourceRef, ctx: &HandlerContext) {
    let Ok(query) = field.query() else {
        return;
    };
    let target_namespace = query.target().scope().namespace().expect("namespaced target");
    ctx.runtime()
        .edgion_config_data_refs
        .add_ref(field.key().expect("validated field"), owner_key.to_string());
    if query.source().scope() != query.target().scope() {
        ctx.runtime()
            .cross_ns_ref_mgr
            .add_ref(target_namespace.to_string(), resource_ref.clone());
    }
}

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn push_warning(
    warnings: &mut Vec<String>,
    warning: String,
) {
    if warnings.len() < MAX_RESOLUTION_WARNINGS {
        warnings.push(warning);
    }
}
