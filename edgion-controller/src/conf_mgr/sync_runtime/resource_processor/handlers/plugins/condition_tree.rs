//! One Condition-tree resolver for an `EdgionPlugins` resource.
//!
//! Two ordered passes (do not swap):
//! 1. **ConfigData expansion** writes RAP `resolved_profiles` / `resolved_candidates`
//!    and fills `IpAccessMatch.resolved_ips` from `ipRefs` everywhere (entry-level
//!    skip/run, body, dye, RAP sidecar, and RequestRestriction ConditionSets).
//! 2. **Credential pass** runs after and reads those snapshots, filling
//!    `resolved_values` / `resolved_credentials`. HMAC `secretGroups` that exist
//!    only on expanded URL candidates are invisible if this pass runs first.
//!
//! Entry-level / body / dye / RequestRestriction write `resolved*` in place on
//! the `Condition` objects. RAP keeps operator `profiles` pristine via the
//! `resolved_profiles` sidecar; in-place `resolved*` lands on conditions inside
//! that sidecar.

use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigDataRef};
use edgion_resources::resources::edgion_plugins::conditions::{ConditionSet, PluginConditions};
use edgion_resources::resources::edgion_plugins::dye::DyeSuite;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    Condition, IpAccessMatch, RequestAccessPolicyConfig, RequestAccessRuleType, SecretMatchCondition,
    MAX_REFERENCED_IP_RULES,
};
use edgion_resources::resources::edgion_plugins::{
    EdgionPlugin, RequestFilterEntry, UpstreamResponseBodyFilterEntry, UpstreamResponseEntry,
    UpstreamResponseFilterEntry,
};
use edgion_resources::ResourceKind;

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::handlers::config_data_ref_mark::{mark_one_ref, ExpectedEntry};
use crate::conf_mgr::sync_runtime::resource_processor::handlers::plugins::request_access_policy_data;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::plugins::resolve_credential as cred;
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};

/// How a missing `ipRefs` target is reported.
#[derive(Clone, Copy)]
enum IpRefFailure {
    /// RAP ConfigData content: keep siblings, record `PartiallyInvalid`.
    PartialGood,
    /// Operator-authored entry-level / RR / body / dye: fail-closed status.
    FailClosed,
}

struct ResolveCtx<'a> {
    owner_namespace: &'a str,
    owner_key: &'a str,
    resource_ref: &'a ResourceRef,
    ctx: &'a HandlerContext,
}

/// Resolve every Condition tree on `ep`. Expansion always runs before credentials.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    expand_config_data(ep, resource_ref, ctx);
    resolve_credentials(ep, resource_ref, ctx);
}

/// Pass 1: RAP sidecars + `ipRefs` → `resolved_ips` on every Condition tree.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn expand_config_data(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    request_access_policy_data::expand(ep, resource_ref, ctx);
    let owner_namespace = ep.metadata.namespace.as_deref().unwrap_or("default").to_string();
    let owner_key = format!(
        "{}:{}/{}",
        ResourceKind::EdgionPlugins.as_str(),
        owner_namespace,
        ep.metadata.name.as_deref().unwrap_or("")
    );
    let resolve_ctx = ResolveCtx {
        owner_namespace: &owner_namespace,
        owner_key: &owner_key,
        resource_ref,
        ctx,
    };
    walk_all_ip_refs(ep, &resolve_ctx);
    enforce_ip_match_cap(ep);
}

/// Pass 2: `secretRef` / HMAC credentials. Must run after [`expand_config_data`].
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve_credentials(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let owner_namespace = ep.metadata.namespace.clone();
    walk_all_credentials(ep, owner_namespace.as_ref(), resource_ref, ctx);
}

fn enforce_ip_match_cap(ep: &mut EdgionPlugins) {
    if ep.ip_match_count() <= MAX_REFERENCED_IP_RULES {
        return;
    }
    let warning = format!("IP match conditions cannot exceed {MAX_REFERENCED_IP_RULES} after ConfigData expansion");
    if let Some(entries) = ep.spec.request_plugins.as_mut() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            if let EdgionPlugin::RequestAccessPolicy(config) = &mut entry.plugin {
                request_access_policy_data::push_warning(&mut config.resolution_warnings, warning.clone());
            }
        }
    }
}

fn walk_all_ip_refs(ep: &mut EdgionPlugins, resolve_ctx: &ResolveCtx<'_>) {
    if let Some(entries) = ep.spec.request_plugins.as_mut() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            walk_request_entry_ip_refs(entry, resolve_ctx);
        }
    }
    if let Some(entries) = ep.spec.upstream_response_filter_plugins.as_mut() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            walk_upstream_filter_entry_ip_refs(entry, resolve_ctx);
        }
    }
    if let Some(entries) = ep.spec.upstream_response_body_filter_plugins.as_mut() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            walk_upstream_body_entry_ip_refs(entry, resolve_ctx);
        }
    }
    if let Some(entries) = ep.spec.upstream_response_plugins.as_mut() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            walk_upstream_response_entry_ip_refs(entry, resolve_ctx);
        }
    }
}

fn walk_request_entry_ip_refs(entry: &mut RequestFilterEntry, resolve_ctx: &ResolveCtx<'_>) {
    walk_plugin_conditions_ip(entry.conditions.as_mut(), resolve_ctx, None);
    walk_plugin_conditions_ip(
        entry.body.as_mut().and_then(|body| body.conditions.as_mut()),
        resolve_ctx,
        None,
    );
    walk_dye_ip(entry.dye.as_mut(), resolve_ctx);
    match &mut entry.plugin {
        EdgionPlugin::RequestAccessPolicy(config) => walk_rap_resolved_ip(config, resolve_ctx),
        EdgionPlugin::RequestRestriction(config) => {
            walk_conditions_ip(&mut config.conditions, resolve_ctx, None, IpRefFailure::FailClosed);
        }
        _ => {}
    }
}

fn walk_upstream_filter_entry_ip_refs(entry: &mut UpstreamResponseFilterEntry, resolve_ctx: &ResolveCtx<'_>) {
    walk_plugin_conditions_ip(entry.conditions.as_mut(), resolve_ctx, None);
    walk_dye_ip(entry.dye.as_mut(), resolve_ctx);
}

fn walk_upstream_body_entry_ip_refs(entry: &mut UpstreamResponseBodyFilterEntry, resolve_ctx: &ResolveCtx<'_>) {
    walk_plugin_conditions_ip(entry.conditions.as_mut(), resolve_ctx, None);
    walk_dye_ip(entry.dye.as_mut(), resolve_ctx);
}

fn walk_upstream_response_entry_ip_refs(entry: &mut UpstreamResponseEntry, resolve_ctx: &ResolveCtx<'_>) {
    walk_plugin_conditions_ip(entry.conditions.as_mut(), resolve_ctx, None);
    walk_plugin_conditions_ip(
        entry.body.as_mut().and_then(|body| body.conditions.as_mut()),
        resolve_ctx,
        None,
    );
    walk_dye_ip(entry.dye.as_mut(), resolve_ctx);
}

fn walk_dye_ip(dye: Option<&mut DyeSuite>, resolve_ctx: &ResolveCtx<'_>) {
    let Some(dye) = dye else {
        return;
    };
    for rule in dye.request.iter_mut().chain(dye.response.iter_mut()) {
        walk_plugin_conditions_ip(rule.conditions.as_mut(), resolve_ctx, None);
    }
}

fn walk_plugin_conditions_ip(
    conditions: Option<&mut PluginConditions>,
    resolve_ctx: &ResolveCtx<'_>,
    warnings: Option<&mut Vec<String>>,
) {
    let Some(conditions) = conditions else {
        return;
    };
    if let Some(warnings) = warnings {
        if let Some(skip) = conditions.skip.as_mut() {
            walk_conditions_ip(skip, resolve_ctx, Some(warnings), IpRefFailure::FailClosed);
        }
        if let Some(run) = conditions.run.as_mut() {
            walk_conditions_ip(run, resolve_ctx, Some(warnings), IpRefFailure::FailClosed);
        }
        return;
    }
    if let Some(skip) = conditions.skip.as_mut() {
        walk_conditions_ip(skip, resolve_ctx, None, IpRefFailure::FailClosed);
    }
    if let Some(run) = conditions.run.as_mut() {
        walk_conditions_ip(run, resolve_ctx, None, IpRefFailure::FailClosed);
    }
}

fn walk_rap_resolved_ip(config: &mut RequestAccessPolicyConfig, resolve_ctx: &ResolveCtx<'_>) {
    let Some(profiles) = config.resolved_profiles.as_mut() else {
        return;
    };
    for profile in profiles.values_mut() {
        for group in &mut profile.required_rule_groups {
            for rule in &mut group.any_of_rules {
                match &mut rule.rule {
                    RequestAccessRuleType::DirectPeerIp(ip) | RequestAccessRuleType::RemoteIp(ip) => {
                        resolve_ip_match(
                            &mut ip.ip,
                            resolve_ctx,
                            Some(&mut config.resolution_warnings),
                            IpRefFailure::PartialGood,
                        );
                        walk_optional_conditions_ip(
                            ip.conditions.as_mut(),
                            resolve_ctx,
                            Some(&mut config.resolution_warnings),
                            IpRefFailure::PartialGood,
                        );
                    }
                    RequestAccessRuleType::Url(url) => {
                        if let Some(candidates) = url.resolved_candidates.as_mut() {
                            for candidate in candidates {
                                walk_optional_conditions_ip(
                                    candidate.conditions.as_mut(),
                                    resolve_ctx,
                                    Some(&mut config.resolution_warnings),
                                    IpRefFailure::PartialGood,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

fn walk_optional_conditions_ip(
    conditions: Option<&mut ConditionSet>,
    resolve_ctx: &ResolveCtx<'_>,
    warnings: Option<&mut Vec<String>>,
    failure: IpRefFailure,
) {
    let Some(conditions) = conditions else {
        return;
    };
    walk_conditions_ip(conditions, resolve_ctx, warnings, failure);
}

fn walk_conditions_ip(
    conditions: &mut ConditionSet,
    resolve_ctx: &ResolveCtx<'_>,
    mut warnings: Option<&mut Vec<String>>,
    failure: IpRefFailure,
) {
    for condition in conditions.iter_conditions_mut() {
        match condition {
            Condition::DirectPeerIpMatch(ip) | Condition::RemoteIpMatch(ip) => {
                resolve_ip_match(ip, resolve_ctx, warnings.as_deref_mut(), failure);
            }
            Condition::KeyExist(_)
            | Condition::KeyMatch(_)
            | Condition::TimeRange(_)
            | Condition::Probability(_)
            | Condition::SecretMatch(_)
            | Condition::HmacAuth(_)
            | Condition::JwtClaimMatch(_)
            | Condition::MtlsMatch(_) => {}
        }
    }
}

fn resolve_ip_match(
    config: &mut IpAccessMatch,
    resolve_ctx: &ResolveCtx<'_>,
    mut warnings: Option<&mut Vec<String>>,
    failure: IpRefFailure,
) {
    if config.ip_refs.is_empty() {
        return;
    }
    let mut resolved = config.ips.clone();
    let mut missing_reference = false;
    for reference in &mut config.ip_refs {
        register_ref(reference, resolve_ctx);
        mark_one_ref(
            reference,
            resolve_ctx.owner_namespace,
            "EdgionPlugins",
            ExpectedEntry::IpList,
            resolve_ctx.ctx,
        );
        match lookup_ip_list(reference, resolve_ctx) {
            Ok((cidrs, content_warning)) => {
                resolved.extend(cidrs);
                if let (Some(warning), Some(warnings)) = (content_warning, warnings.as_deref_mut()) {
                    request_access_policy_data::push_warning(warnings, warning);
                }
            }
            Err(warning) => {
                missing_reference = true;
                if let Some(warnings) = warnings.as_deref_mut() {
                    request_access_policy_data::push_warning(warnings, warning);
                }
            }
        }
    }
    match failure {
        IpRefFailure::PartialGood => {
            config.resolved_ips = Some(resolved);
        }
        IpRefFailure::FailClosed => {
            // A missing *reference* is a status condition. Do not compile an
            // empty matcher from `resolvedIps: []` when operator-authored
            // ipRefs did not resolve. RAP partial-good `resolvedIps` is
            // ConfigData IpList *content*, not a missing reference.
            if !missing_reference {
                config.resolved_ips = Some(resolved);
            }
        }
    }
}

fn lookup_ip_list(
    reference: &EdgionConfigDataRef,
    resolve_ctx: &ResolveCtx<'_>,
) -> Result<(Vec<String>, Option<String>), String> {
    let target_namespace = reference.namespace.as_deref().unwrap_or(resolve_ctx.owner_namespace);
    if reference.ref_denied.is_some() {
        return Err(format!(
            "RequestAccessPolicy IP data ref {target_namespace}/{} is not permitted",
            reference.name
        ));
    }
    let Some(target) = resolve_ctx
        .ctx
        .runtime()
        .edgion_config_data_store
        .get(Some(target_namespace), &reference.name)
    else {
        return Err(format!(
            "RequestAccessPolicy IP data ref {target_namespace}/{} is unavailable",
            reference.name
        ));
    };
    if !target.spec.enable {
        return Err(format!(
            "RequestAccessPolicy IP data ref {target_namespace}/{} is disabled",
            reference.name
        ));
    }
    let ConfigEntry::IpList(data) = &target.spec.data else {
        return Err(format!(
            "RequestAccessPolicy IP data ref {target_namespace}/{} has the wrong type",
            reference.name
        ));
    };
    // Invalid CIDRs inside a present IpList are partial-good content, not a
    // missing reference. Callers keep the valid siblings.
    let content_warning = if !edgion_resources::matcher::ip_cidr_helpers::collect_entry_errors(&data.items).is_empty() {
        Some(format!(
            "RequestAccessPolicy IP data ref {target_namespace}/{} contains invalid entries; valid entries were retained",
            reference.name
        ))
    } else {
        None
    };
    Ok((
        data.items
            .iter()
            .flat_map(|group| group.cidrs.iter().cloned())
            .collect(),
        content_warning,
    ))
}

fn register_ref(reference: &EdgionConfigDataRef, resolve_ctx: &ResolveCtx<'_>) {
    let target_namespace = reference.namespace.as_deref().unwrap_or(resolve_ctx.owner_namespace);
    let target_key = format!("{target_namespace}/{}", reference.name);
    resolve_ctx
        .ctx
        .runtime()
        .edgion_config_data_refs
        .add_ref(target_key, resolve_ctx.owner_key.to_string());
    if target_namespace != resolve_ctx.owner_namespace {
        resolve_ctx
            .ctx
            .runtime()
            .cross_ns_ref_mgr
            .add_ref(target_namespace.to_string(), resolve_ctx.resource_ref.clone());
    }
}

fn walk_all_credentials(
    ep: &mut EdgionPlugins,
    owner_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(entries) = ep.spec.request_plugins.as_mut() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            walk_plugin_conditions_credentials(entry.conditions.as_mut(), owner_namespace, resource_ref, ctx);
            walk_plugin_conditions_credentials(
                entry.body.as_mut().and_then(|body| body.conditions.as_mut()),
                owner_namespace,
                resource_ref,
                ctx,
            );
            walk_dye_credentials(entry.dye.as_mut(), owner_namespace, resource_ref, ctx);
            match &mut entry.plugin {
                EdgionPlugin::RequestAccessPolicy(config) => {
                    walk_rap_resolved_credentials(config, owner_namespace, resource_ref, ctx);
                }
                EdgionPlugin::RequestRestriction(config) => {
                    walk_conditions_credentials(&mut config.conditions, owner_namespace, resource_ref, ctx);
                }
                _ => {}
            }
        }
    }
    if let Some(entries) = ep.spec.upstream_response_filter_plugins.as_mut() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            walk_plugin_conditions_credentials(entry.conditions.as_mut(), owner_namespace, resource_ref, ctx);
            walk_dye_credentials(entry.dye.as_mut(), owner_namespace, resource_ref, ctx);
        }
    }
    if let Some(entries) = ep.spec.upstream_response_body_filter_plugins.as_mut() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            walk_plugin_conditions_credentials(entry.conditions.as_mut(), owner_namespace, resource_ref, ctx);
            walk_dye_credentials(entry.dye.as_mut(), owner_namespace, resource_ref, ctx);
        }
    }
    if let Some(entries) = ep.spec.upstream_response_plugins.as_mut() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            walk_plugin_conditions_credentials(entry.conditions.as_mut(), owner_namespace, resource_ref, ctx);
            walk_plugin_conditions_credentials(
                entry.body.as_mut().and_then(|body| body.conditions.as_mut()),
                owner_namespace,
                resource_ref,
                ctx,
            );
            walk_dye_credentials(entry.dye.as_mut(), owner_namespace, resource_ref, ctx);
        }
    }
}

fn walk_dye_credentials(
    dye: Option<&mut DyeSuite>,
    owner_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Some(dye) = dye else {
        return;
    };
    for rule in dye.request.iter_mut().chain(dye.response.iter_mut()) {
        walk_plugin_conditions_credentials(rule.conditions.as_mut(), owner_namespace, resource_ref, ctx);
    }
}

fn walk_plugin_conditions_credentials(
    conditions: Option<&mut PluginConditions>,
    owner_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Some(conditions) = conditions else {
        return;
    };
    if let Some(skip) = conditions.skip.as_mut() {
        walk_conditions_credentials(skip, owner_namespace, resource_ref, ctx);
    }
    if let Some(run) = conditions.run.as_mut() {
        walk_conditions_credentials(run, owner_namespace, resource_ref, ctx);
    }
}

fn walk_rap_resolved_credentials(
    config: &mut RequestAccessPolicyConfig,
    owner_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Some(profiles) = config.resolved_profiles.as_mut() else {
        return;
    };
    for profile in profiles.values_mut() {
        for group in &mut profile.required_rule_groups {
            for rule in &mut group.any_of_rules {
                match &mut rule.rule {
                    RequestAccessRuleType::DirectPeerIp(ip) | RequestAccessRuleType::RemoteIp(ip) => {
                        if let Some(conditions) = ip.conditions.as_mut() {
                            walk_conditions_credentials(conditions, owner_namespace, resource_ref, ctx);
                        }
                    }
                    RequestAccessRuleType::Url(url) => {
                        if let Some(candidates) = url.resolved_candidates.as_mut() {
                            for candidate in candidates {
                                if let Some(conditions) = candidate.conditions.as_mut() {
                                    walk_conditions_credentials(conditions, owner_namespace, resource_ref, ctx);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn walk_conditions_credentials(
    conditions: &mut ConditionSet,
    owner_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    for condition in conditions.iter_conditions_mut() {
        match condition {
            Condition::SecretMatch(condition) => {
                resolve_secret_match(condition, owner_namespace, resource_ref, ctx);
            }
            Condition::HmacAuth(config) => {
                cred::resolve_hmac_config(config, owner_namespace, resource_ref, ctx);
            }
            Condition::KeyExist(_)
            | Condition::KeyMatch(_)
            | Condition::TimeRange(_)
            | Condition::Probability(_)
            | Condition::DirectPeerIpMatch(_)
            | Condition::RemoteIpMatch(_)
            | Condition::JwtClaimMatch(_)
            | Condition::MtlsMatch(_) => {}
        }
    }
}

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
                "Condition tree: Secret not found yet"
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
                "Condition tree: cross-namespace Secret reference denied"
            );
            return;
        }
    };
    let Some(value) = secret.data.as_ref().and_then(|data| data.get(&condition.secret_key)) else {
        tracing::warn!(
            edgion_plugins = %resource_ref.key(),
            secret_key = %secret_key,
            selected_key = %condition.secret_key,
            "Condition tree: referenced Secret is missing the selected data key"
        );
        return;
    };
    let Ok(value) = String::from_utf8(value.0.clone()) else {
        tracing::warn!(
            edgion_plugins = %resource_ref.key(),
            secret_key = %secret_key,
            selected_key = %condition.secret_key,
            "Condition tree: selected Secret data value is not UTF-8"
        );
        return;
    };
    if value.trim().is_empty() {
        tracing::warn!(
            edgion_plugins = %resource_ref.key(),
            secret_key = %secret_key,
            selected_key = %condition.secret_key,
            "Condition tree: selected Secret data value is empty"
        );
        return;
    }
    condition.resolved_values = Some(vec![value]);
}

/// Visit every SecretMatch condition that the credential pass resolves.
///
/// This read-only mirror feeds status classification after parse. Keep its
/// consumer coverage aligned with [`walk_all_credentials`].
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn for_each_secret_match(
    ep: &EdgionPlugins,
    mut visit: impl FnMut(&SecretMatchCondition),
) {
    if let Some(entries) = ep.spec.request_plugins.as_ref() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            visit_plugin_conditions_secret_matches(entry.conditions.as_ref(), &mut visit);
            visit_plugin_conditions_secret_matches(
                entry.body.as_ref().and_then(|body| body.conditions.as_ref()),
                &mut visit,
            );
            visit_dye_secret_matches(entry.dye.as_ref(), &mut visit);
            match &entry.plugin {
                EdgionPlugin::RequestAccessPolicy(config) => {
                    if let Some(profiles) = config.resolved_profiles.as_ref() {
                        for profile in profiles.values() {
                            for group in &profile.required_rule_groups {
                                for rule in &group.any_of_rules {
                                    match &rule.rule {
                                        RequestAccessRuleType::DirectPeerIp(ip)
                                        | RequestAccessRuleType::RemoteIp(ip) => {
                                            if let Some(conditions) = ip.conditions.as_ref() {
                                                visit_conditions_secret_matches(conditions, &mut visit);
                                            }
                                        }
                                        RequestAccessRuleType::Url(url) => {
                                            if let Some(candidates) = url.resolved_candidates.as_ref() {
                                                for candidate in candidates {
                                                    if let Some(conditions) = candidate.conditions.as_ref() {
                                                        visit_conditions_secret_matches(conditions, &mut visit);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                EdgionPlugin::RequestRestriction(config) => {
                    visit_conditions_secret_matches(&config.conditions, &mut visit);
                }
                _ => {}
            }
        }
    }
    if let Some(entries) = ep.spec.upstream_response_filter_plugins.as_ref() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            visit_plugin_conditions_secret_matches(entry.conditions.as_ref(), &mut visit);
            visit_dye_secret_matches(entry.dye.as_ref(), &mut visit);
        }
    }
    if let Some(entries) = ep.spec.upstream_response_body_filter_plugins.as_ref() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            visit_plugin_conditions_secret_matches(entry.conditions.as_ref(), &mut visit);
            visit_dye_secret_matches(entry.dye.as_ref(), &mut visit);
        }
    }
    if let Some(entries) = ep.spec.upstream_response_plugins.as_ref() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            visit_plugin_conditions_secret_matches(entry.conditions.as_ref(), &mut visit);
            visit_plugin_conditions_secret_matches(
                entry.body.as_ref().and_then(|body| body.conditions.as_ref()),
                &mut visit,
            );
            visit_dye_secret_matches(entry.dye.as_ref(), &mut visit);
        }
    }
}

fn visit_plugin_conditions_secret_matches(
    conditions: Option<&PluginConditions>,
    visit: &mut impl FnMut(&SecretMatchCondition),
) {
    let Some(conditions) = conditions else {
        return;
    };
    for set in [conditions.skip.as_ref(), conditions.run.as_ref()]
        .into_iter()
        .flatten()
    {
        visit_conditions_secret_matches(set, visit);
    }
}

fn visit_dye_secret_matches(dye: Option<&DyeSuite>, visit: &mut impl FnMut(&SecretMatchCondition)) {
    let Some(dye) = dye else {
        return;
    };
    for rule in dye.request.iter().chain(dye.response.iter()) {
        visit_plugin_conditions_secret_matches(rule.conditions.as_ref(), visit);
    }
}

fn visit_conditions_secret_matches(conditions: &ConditionSet, visit: &mut impl FnMut(&SecretMatchCondition)) {
    for condition in conditions.iter_conditions() {
        if let Condition::SecretMatch(condition) = condition {
            visit(condition);
        }
    }
}

/// Collect operator-authored (non-RAP-sidecar) `ipRefs` for `ResolvedRefs`.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn for_each_fail_closed_ip_ref(
    ep: &EdgionPlugins,
    mut visit: impl FnMut(&EdgionConfigDataRef),
) {
    if let Some(entries) = ep.spec.request_plugins.as_ref() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            visit_plugin_conditions_ip_refs(entry.conditions.as_ref(), &mut visit);
            visit_plugin_conditions_ip_refs(
                entry.body.as_ref().and_then(|body| body.conditions.as_ref()),
                &mut visit,
            );
            visit_dye_ip_refs(entry.dye.as_ref(), &mut visit);
            if let EdgionPlugin::RequestRestriction(config) = &entry.plugin {
                for condition in config.conditions.iter_conditions() {
                    visit_condition_ip_refs(condition, &mut visit);
                }
            }
        }
    }
    if let Some(entries) = ep.spec.upstream_response_filter_plugins.as_ref() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            visit_plugin_conditions_ip_refs(entry.conditions.as_ref(), &mut visit);
            visit_dye_ip_refs(entry.dye.as_ref(), &mut visit);
        }
    }
    if let Some(entries) = ep.spec.upstream_response_body_filter_plugins.as_ref() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            visit_plugin_conditions_ip_refs(entry.conditions.as_ref(), &mut visit);
            visit_dye_ip_refs(entry.dye.as_ref(), &mut visit);
        }
    }
    if let Some(entries) = ep.spec.upstream_response_plugins.as_ref() {
        for entry in entries {
            if !entry.is_enabled() || entry.policy_action.is_some() {
                continue;
            }
            visit_plugin_conditions_ip_refs(entry.conditions.as_ref(), &mut visit);
            visit_plugin_conditions_ip_refs(
                entry.body.as_ref().and_then(|body| body.conditions.as_ref()),
                &mut visit,
            );
            visit_dye_ip_refs(entry.dye.as_ref(), &mut visit);
        }
    }
}

fn visit_plugin_conditions_ip_refs(
    conditions: Option<&PluginConditions>,
    visit: &mut impl FnMut(&EdgionConfigDataRef),
) {
    let Some(conditions) = conditions else {
        return;
    };
    for set in [conditions.skip.as_ref(), conditions.run.as_ref()]
        .into_iter()
        .flatten()
    {
        for condition in set.iter_conditions() {
            visit_condition_ip_refs(condition, visit);
        }
    }
}

fn visit_dye_ip_refs(dye: Option<&DyeSuite>, visit: &mut impl FnMut(&EdgionConfigDataRef)) {
    let Some(dye) = dye else {
        return;
    };
    for rule in dye.request.iter().chain(dye.response.iter()) {
        visit_plugin_conditions_ip_refs(rule.conditions.as_ref(), visit);
    }
}

fn visit_condition_ip_refs(condition: &Condition, visit: &mut impl FnMut(&EdgionConfigDataRef)) {
    match condition {
        Condition::DirectPeerIpMatch(ip) | Condition::RemoteIpMatch(ip) => {
            for reference in &ip.ip_refs {
                visit(reference);
            }
        }
        _ => {}
    }
}
