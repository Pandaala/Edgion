//! Controller-side RequestAccessPolicy ConfigData sidecar expansion.
//!
//! Pass 1 of the Condition-tree resolver. Writes `resolved_profiles` /
//! `resolved_candidates` only. IP-list `ipRefs` and Secret/HMAC credentials
//! are filled by the generic walker in `condition_tree` — this module must
//! not walk those leaves itself.

use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigDataRef};
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    RequestAccessPolicyConfig, RequestAccessRuleType, UrlAccessCandidate, UrlAccessRuleConfig,
};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::ResourceKind;

use crate::conf_mgr::sync_runtime::resource_processor::handlers::config_data_ref_mark::{mark_one_ref, ExpectedEntry};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};

const MAX_RESOLUTION_WARNINGS: usize = 64;

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn expand(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let owner_namespace = ep.metadata.namespace.as_deref().unwrap_or("default").to_string();
    let owner_key = format!(
        "{}:{}/{}",
        ResourceKind::EdgionPlugins.as_str(),
        owner_namespace,
        ep.metadata.name.as_deref().unwrap_or("")
    );
    let Some(entries) = ep.spec.request_plugins.as_mut() else {
        return;
    };
    for entry in entries {
        let EdgionPlugin::RequestAccessPolicy(config) = &mut entry.plugin else {
            continue;
        };
        expand_config(config, &owner_namespace, &owner_key, resource_ref, ctx);
    }
}

fn expand_config(
    config: &mut RequestAccessPolicyConfig,
    owner_namespace: &str,
    owner_key: &str,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    config.resolution_warnings.clear();
    let mut profiles = config.profiles.clone();
    for profile in profiles.values_mut() {
        for group in &mut profile.required_rule_groups {
            for rule in &mut group.any_of_rules {
                if let RequestAccessRuleType::Url(url) = &mut rule.rule {
                    expand_url_rule(
                        &rule.name,
                        url,
                        owner_namespace,
                        owner_key,
                        resource_ref,
                        ctx,
                        &mut config.resolution_warnings,
                    );
                }
            }
        }
    }
    config.resolved_profiles = Some(profiles);
}

fn expand_url_rule(
    rule_name: &str,
    config: &mut UrlAccessRuleConfig,
    owner_namespace: &str,
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
            all_of: config.all_of.clone(),
        });
    }

    for reference in &mut config.config_refs {
        let data_ref = &mut reference.reference;
        register_ref(data_ref, owner_namespace, owner_key, resource_ref, ctx);
        mark_one_ref(
            data_ref,
            owner_namespace,
            "EdgionPlugins",
            ExpectedEntry::RequestAccessUrlAllowList,
            ctx,
        );
        if data_ref.ref_denied.is_some() {
            let target_namespace = data_ref.namespace.as_deref().unwrap_or(owner_namespace);
            push_warning(
                warnings,
                format!(
                    "RequestAccessPolicy URL data ref {target_namespace}/{} is not permitted",
                    data_ref.name
                ),
            );
            continue;
        }
        let target_namespace = data_ref.namespace.as_deref().unwrap_or(owner_namespace);
        let Some(target) = ctx
            .runtime()
            .edgion_config_data_store
            .get(Some(target_namespace), &data_ref.name)
        else {
            push_warning(
                warnings,
                format!(
                    "RequestAccessPolicy URL data ref {target_namespace}/{} is unavailable",
                    data_ref.name
                ),
            );
            continue;
        };
        if !target.spec.enable {
            push_warning(
                warnings,
                format!(
                    "RequestAccessPolicy URL data ref {target_namespace}/{} is disabled",
                    data_ref.name
                ),
            );
            continue;
        }
        let ConfigEntry::RequestAccessUrlAllowList(data) = &target.spec.data else {
            push_warning(
                warnings,
                format!(
                    "RequestAccessPolicy URL data ref {target_namespace}/{} has the wrong type",
                    data_ref.name
                ),
            );
            continue;
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

fn register_ref(
    reference: &EdgionConfigDataRef,
    owner_namespace: &str,
    owner_key: &str,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let target_namespace = reference.namespace.as_deref().unwrap_or(owner_namespace);
    let target_key = format!("{target_namespace}/{}", reference.name);
    ctx.runtime()
        .edgion_config_data_refs
        .add_ref(target_key, owner_key.to_string());
    if target_namespace != owner_namespace {
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
