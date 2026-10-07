//! BasicAuth credential resolution.

use std::collections::HashMap;

use super::read_secret_utf8;
use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_bound_secret, BoundFetchOutcome, PendingReason,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::ResolvedBasicAuthUser;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::refs::plugin_fields::{
    BoundPluginEntryMut, PreparedPluginReferences, SecretGroupReferences,
};

/// Pure helper for first-wins duplicate-username detection.
///
/// Inserts `username -> user` into `all_users` (and tracks the group in
/// `seen`) only if `seen` does not already contain `username`. Returns
/// `true` if inserted, `false` if rejected (first-wins: existing entry
/// kept).
fn first_wins_insert(
    seen: &mut HashMap<String, String>,
    all_users: &mut HashMap<String, ResolvedBasicAuthUser>,
    username: String,
    user: ResolvedBasicAuthUser,
    group: String,
) -> bool {
    if seen.contains_key(&username) {
        return false;
    }
    seen.insert(username.clone(), group);
    all_users.insert(username, user);
    true
}

/// Resolve all Secret refs for a single group. Each Secret is K8s
/// `type: kubernetes.io/basic-auth` with flat `username` + `password`
/// fields — 1 Secret = 1 user. Cross-group / intra-group duplicate
/// usernames follow first-wins.
#[allow(clippy::too_many_arguments)]
fn resolve_group(
    group: &SecretGroupReferences<'_>,
    all_users: &mut HashMap<String, ResolvedBasicAuthUser>,
    seen: &mut HashMap<String, String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let group_name = group.name();
    for token in group.references() {
        let Ok(reference) = token.validated() else {
            continue;
        };
        let target = reference.target();
        let secret_key = format!("{}/{}", target.scope().namespace().unwrap_or_default(), target.name());
        let secret = match fetch_bound_secret(ctx, resource_ref, token) {
            BoundFetchOutcome::Ok(authorized) => authorized.secret,
            BoundFetchOutcome::Invalid(_) => continue,
            BoundFetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                tracing::info!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    "BasicAuth: Secret not found yet, will be reprocessed when Secret arrives"
                );
                continue;
            }
            BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
                ref from_ns,
                ref to_ns,
                ref secret_name,
            }) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    from_ns = %from_ns,
                    to_ns = %to_ns,
                    secret_name = %secret_name,
                    group = %group_name,
                    "BasicAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                );
                continue;
            }
        };

        let username = read_secret_utf8(&secret, &["username"])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let password = read_secret_utf8(&secret, &["password"])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let (Some(username), Some(password)) = (username, password) else {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group_name,
                "BasicAuth: Secret missing or empty 'username'/'password' field"
            );
            continue;
        };

        let user = ResolvedBasicAuthUser {
            password,
            group: group_name.to_string(),
        };

        let inserted = first_wins_insert(seen, all_users, username.clone(), user, group_name.to_string());

        if !inserted {
            let prev_group = seen.get(&username).cloned().unwrap_or_default();
            if prev_group != group_name {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    existing_group = %prev_group,
                    incoming_group = %group_name,
                    "BasicAuth: duplicate username across groups; first-wins kept"
                );
            } else {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    "BasicAuth: Duplicate username within group, skipping"
                );
            }
        }
    }
}

/// Resolve BasicAuth users from Secrets and register references to SecretRefManager.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    prepared: &PreparedPluginReferences,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Ok(mut bindings) = prepared.bind_mut(ep) else {
        return;
    };
    for entry in bindings.request_entries() {
        resolve_entry(entry, resource_ref, ctx);
    }
    for entry in bindings.response_filter_entries() {
        resolve_entry(entry, resource_ref, ctx);
    }
    for entry in bindings.response_body_filter_entries() {
        resolve_entry(entry, resource_ref, ctx);
    }
    for entry in bindings.response_entries() {
        resolve_entry(entry, resource_ref, ctx);
    }
}

fn resolve_entry(mut entry: BoundPluginEntryMut<'_>, resource_ref: &ResourceRef, ctx: &HandlerContext) {
    let EdgionPlugin::BasicAuth(config) = entry.config_mut() else {
        return;
    };
    config.resolved_users = None;
    if !entry.is_enabled() {
        return;
    }
    let mut all_users = HashMap::new();
    let mut seen = HashMap::new();
    {
        let Ok(references) = entry.references() else {
            return;
        };
        for group in references.secret_groups() {
            resolve_group(&group, &mut all_users, &mut seen, resource_ref, ctx);
        }
    }
    if let EdgionPlugin::BasicAuth(config) = entry.config_mut() {
        config.resolved_users = (!all_users.is_empty()).then_some(all_users);
    }
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/basic_auth_test.rs"]
mod tests;
