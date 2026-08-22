//! Resolve Secret-backed RequestAccessPolicy conditions.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    AccessCondition, AccessConditionGroup, SecretMatchCondition, SourceRules,
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

fn resolve_groups(
    groups: Option<&mut Vec<AccessConditionGroup>>,
    owner_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Some(groups) = groups else { return };
    for group in groups {
        for condition in &mut group.all_of {
            match condition {
                AccessCondition::SecretMatch(condition) => {
                    resolve_secret_match(condition, owner_namespace, resource_ref, ctx);
                }
                AccessCondition::HmacAuth(config) => {
                    hmac::resolve_config(config, owner_namespace, resource_ref, ctx);
                }
                _ => {}
            }
        }
    }
}

fn resolve_source(
    source: &mut SourceRules,
    owner_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    for rule in &mut source.ip_rules {
        resolve_groups(rule.any_of.as_mut(), owner_namespace, resource_ref, ctx);
    }
    for rule in &mut source.url_rules {
        resolve_groups(rule.any_of.as_mut(), owner_namespace, resource_ref, ctx);
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
        for profile in config.profiles.values_mut() {
            if let Some(source) = &mut profile.source_match.direct_peer_ip {
                resolve_source(source, owner_namespace.as_ref(), resource_ref, ctx);
            }
            if let Some(source) = &mut profile.source_match.remote_ip {
                resolve_source(source, owner_namespace.as_ref(), resource_ref, ctx);
            }
        }
    }
}
