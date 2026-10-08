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

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_bound_secret, BoundFetchOutcome};
use crate::conf_mgr::sync_runtime::resource_processor::handlers::ip_list_refs::{
    resolve_ip_reference, validated_ip_reference,
};
use crate::conf_mgr::sync_runtime::resource_processor::handlers::plugins::request_access_policy_data;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::plugins::resolve_credential as cred;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError, ResourceRef};
use crate::config::is_reference_grant_validation_enabled;
#[cfg(test)]
use edgion_resources::prelude_resources::EdgionConfigData;
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::plugin_configs::Condition;
use edgion_resources::resources::refs::condition_fields::{
    BoundConditionMut, BoundIpAccessMut, IpReferenceConsumer, PreparedConditionReferences,
};

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
    let prepared = PreparedConditionReferences::from_plugins(ep);
    // The owner was just prepared; callbacks mutate only Controller-owned payloads.
    let _ = prepared.visit_ip_mut(
        ep,
        |leaf, warning| resolve_ip_match(leaf, resource_ref, ctx, warning),
        request_access_policy_data::push_warning,
    );
}

/// Pass 2: `secretRef` / HMAC credentials. Must run after [`expand_config_data`].
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve_credentials(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let prepared = PreparedConditionReferences::from_plugins(ep);
    if let Err(error) = prepared.visit_mut(ep, |leaf| match leaf.config() {
        Condition::HmacAuth(_) => cred::resolve_condition_hmac(leaf, resource_ref, ctx),
        Condition::SecretMatch(_) => resolve_secret_match(leaf, resource_ref, ctx),
        _ => {}
    }) {
        tracing::warn!(component="condition_tree",error=?error,"Condition reference binding failed");
    }
}

fn resolve_ip_match(
    mut leaf: BoundIpAccessMut<'_>,
    owner: &ResourceRef,
    ctx: &HandlerContext,
    warning: &mut dyn FnMut(String),
) {
    if leaf.config().ip_refs.is_empty() {
        leaf.config_mut().resolved_ips = None;
        return;
    }
    let partial = leaf.consumer() == IpReferenceConsumer::RequestAccessPolicy;
    let mut resolved = leaf.config().ips.clone();
    let mut failed = false;
    let markers: Option<Vec<_>> = match leaf.references() {
        Ok(tokens) => Some(tokens.map(|token| {
            if let Ok(bound) = validated_ip_reference(&token) {
                let target = bound.target();
                let namespace = target.scope().namespace().unwrap_or_default();
                ctx.runtime().edgion_config_data_refs.add_ref(format!("{namespace}/{}", target.name()), format!("{}:{}", owner.kind_str(), owner.resource_key()));
                if !bound.is_same_namespace() { ctx.runtime().cross_ns_ref_mgr.add_ref(namespace.into(), owner.clone()); }
            }
            let outcome = resolve_ip_reference(token, is_reference_grant_validation_enabled(), |ns, name| ctx.runtime().edgion_config_data_store.get(Some(ns), name), |query| ctx.runtime().ref_grant_store.check_query(query));
            failed |= outcome.error.is_some();
            if let Some(data) = outcome.data {
                if let ConfigEntry::IpList(ip) = data.spec.data {
                    if partial && !edgion_resources::matcher::ip_cidr_helpers::collect_entry_errors(&ip.items).is_empty() {
                        if let Ok(bound) = validated_ip_reference(&token) {
                            warning(format!("RequestAccessPolicy IP data ref {}/{} contains invalid entries; valid entries were retained", bound.target().scope().namespace().unwrap_or_default(), bound.target().name()));
                        }
                    }
                    resolved.extend(ip.items.into_iter().flat_map(|group| group.cidrs));
                }
            }
            if partial { if let Some(message) = outcome.warning { warning(message); } }
            outcome.denied
        }).collect()),
        Err(_) => None,
    };
    let Some(markers) = markers else {
        leaf.config_mut().resolved_ips = None;
        return;
    };
    let config = leaf.config_mut();
    config.resolved_ips = None;
    for (reference, marker) in config.ip_refs.iter_mut().zip(markers) {
        reference.ref_denied = marker;
    }
    if !config.ip_refs.is_empty() && (partial || !failed) {
        config.resolved_ips = Some(resolved);
    }
}

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn ip_status_errors(
    ep: &EdgionPlugins,
    ctx: &HandlerContext,
) -> Vec<ResolvedRefsError> {
    let prepared = PreparedConditionReferences::from_plugins(ep);
    let mut errors = Vec::new();
    let result = prepared.visit_ip(ep, |leaf| {
        if leaf.consumer() != IpReferenceConsumer::Ordinary {
            return;
        }
        match leaf.references() {
            Ok(tokens) => {
                for token in tokens {
                    let result = resolve_ip_reference(
                        token,
                        is_reference_grant_validation_enabled(),
                        |ns, name| ctx.runtime().edgion_config_data_store.get(Some(ns), name),
                        |query| ctx.runtime().ref_grant_store.check_query(query),
                    );
                    errors.extend(result.error);
                }
            }
            Err(error) => errors.push(ResolvedRefsError::InvalidReference { error }),
        }
    });
    if let Err(error) = result {
        errors.push(ResolvedRefsError::InvalidReference { error });
    }
    errors
}

fn resolve_secret_match(mut leaf: BoundConditionMut<'_>, owner: &ResourceRef, ctx: &HandlerContext) {
    let Condition::SecretMatch(config) = leaf.config_mut() else {
        return;
    };
    config.resolved_values = None;
    let value = {
        let Condition::SecretMatch(config) = leaf.config() else {
            return;
        };
        let Ok(references) = leaf.references() else { return };
        let Some(token) = references.secret_match_secret() else {
            return;
        };
        let BoundFetchOutcome::Ok(authorized) = fetch_bound_secret(ctx, owner, token) else {
            return;
        };
        authorized
            .secret
            .data
            .as_ref()
            .and_then(|data| data.get(&config.secret_key))
            .and_then(|bytes| std::str::from_utf8(&bytes.0).ok())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
    };
    if let Condition::SecretMatch(config) = leaf.config_mut() {
        config.resolved_values = value.map(|value| vec![value]);
    }
}

/// Readonly condition status uses the same effective owning roots as parse.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn credential_status_errors(
    ep: &EdgionPlugins,
    owner: &ResourceRef,
    ctx: &HandlerContext,
) -> Vec<crate::conf_mgr::sync_runtime::resource_processor::ResolvedRefsError> {
    use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::{
        bound_secret_status_error, bound_secret_status_result,
    };
    use crate::conf_mgr::sync_runtime::resource_processor::ResolvedRefsError;
    let prepared = PreparedConditionReferences::from_plugins(ep);
    let mut errors = Vec::new();
    let result = prepared.visit(ep, |leaf| {
        let references = match leaf.references() {
            Ok(references) => references,
            Err(error) => {
                errors.push(ResolvedRefsError::InvalidReference { error });
                return;
            }
        };
        for group in references.hmac_secret_groups() {
            for token in group.references() {
                errors.extend(bound_secret_status_error(ctx, owner, token));
            }
        }
        let Some(token) = references.secret_match_secret() else {
            return;
        };
        let Condition::SecretMatch(config) = leaf.config() else {
            return;
        };
        let authorized = match bound_secret_status_result(ctx, owner, token) {
            Ok(authorized) => authorized,
            Err(error) => {
                errors.push(error);
                return;
            }
        };
        let reference = match token.validated() {
            Ok(reference) => reference,
            Err(error) => {
                errors.push(ResolvedRefsError::InvalidReference { error });
                return;
            }
        };
        let target = reference.target();
        let namespace = target.scope().namespace().unwrap_or_default().to_string();
        let name = target.name().to_string();
        let Some(value) = authorized
            .secret
            .data
            .as_ref()
            .and_then(|data| data.get(&config.secret_key))
        else {
            errors.push(ResolvedRefsError::SecretMatchKeyMissing {
                namespace,
                name,
                key: config.secret_key.clone(),
            });
            return;
        };
        let detail = match std::str::from_utf8(&value.0) {
            Err(_) => Some("value is not valid UTF-8"),
            Ok(value) if value.trim().is_empty() => Some("value is empty or whitespace-only"),
            Ok(_) => None,
        };
        if let Some(detail) = detail {
            errors.push(ResolvedRefsError::SecretMatchValueInvalid {
                namespace,
                name,
                key: config.secret_key.clone(),
                detail,
            });
        }
    });
    if let Err(error) = result {
        errors.push(ResolvedRefsError::InvalidReference { error });
    }
    errors
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/condition_tree_test.rs"]
mod tests;
