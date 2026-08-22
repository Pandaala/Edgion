//! Resolve Secret-backed RequestAccessPolicy conditions after data expansion.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    AccessCondition, RequestAccessRuleType, SecretMatchCondition,
};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

use super::hmac;

fn resolve_secret_match(
    condition: &mut SecretMatchCondition,
    owner_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    condition.resolved_values = None;
    let reference = condition.secret_ref.clone();
    let secret_key = format_secret_key(reference.namespace.as_ref().or(owner_namespace), &reference.name);
    let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &reference) {
        FetchOutcome::Ok(secret) => secret,
        FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
            tracing::info!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                "RequestAccessPolicy: Secret not found yet"
            );
            return;
        }
        FetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
            ref from_ns,
            ref to_ns,
            ref secret_name,
        }) => {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                from_ns = %from_ns,
                to_ns = %to_ns,
                secret_name = %secret_name,
                "RequestAccessPolicy: cross-namespace Secret reference denied"
            );
            return;
        }
    };
    let values: Vec<String> = secret
        .data
        .iter()
        .flatten()
        .filter_map(|(_, value)| String::from_utf8(value.0.clone()).ok())
        .filter(|value| !value.trim().is_empty())
        .collect();
    if values.is_empty() {
        tracing::warn!(
            edgion_plugins = %resource_ref.key(),
            secret_key = %secret_key,
            "RequestAccessPolicy: referenced Secret has no usable string values"
        );
    } else {
        condition.resolved_values = Some(values);
    }
}

fn resolve_conditions(
    conditions: &mut [AccessCondition],
    owner_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    for condition in conditions {
        match condition {
            AccessCondition::SecretMatch(condition) => {
                resolve_secret_match(condition, owner_namespace, resource_ref, ctx);
            }
            AccessCondition::HmacAuth(config) => {
                hmac::resolve_config(config, hmac::MetadataPolicy::Ignore, owner_namespace, resource_ref, ctx);
            }
            AccessCondition::KeyExist(_)
            | AccessCondition::KeyMatch(_)
            | AccessCondition::DirectPeerIpMatch(_)
            | AccessCondition::RemoteIpMatch(_) => {}
        }
    }
}

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let owner_namespace = ep.metadata.namespace.clone();
    let Some(plugins) = ep.spec.request_plugins.as_mut() else {
        return;
    };
    for entry in plugins {
        let EdgionPlugin::RequestAccessPolicy(config) = &mut entry.plugin else {
            continue;
        };
        let Some(profiles) = config.resolved_profiles.as_mut() else {
            continue;
        };
        for profile in profiles.values_mut() {
            for group in &mut profile.required_rule_groups {
                for rule in &mut group.any_of_rules {
                    match &mut rule.rule {
                        RequestAccessRuleType::DirectPeerIp(config) | RequestAccessRuleType::RemoteIp(config) => {
                            resolve_conditions(&mut config.all_of, owner_namespace.as_ref(), resource_ref, ctx);
                        }
                        RequestAccessRuleType::Url(config) => {
                            if let Some(candidates) = &mut config.resolved_candidates {
                                for candidate in candidates {
                                    resolve_conditions(
                                        &mut candidate.all_of,
                                        owner_namespace.as_ref(),
                                        resource_ref,
                                        ctx,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
