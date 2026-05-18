//! BasicAuth credential resolution.

use std::collections::HashMap;

use super::read_secret_utf8;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, FetchOutcome, PendingReason,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ResourceRef,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::edgion_plugins::plugin_configs::ResolvedBasicAuthUser;
use crate::types::resources::edgion_plugins::EdgionPlugin;

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
    group_name: &str,
    secret_refs: &[crate::types::resources::gateway::SecretObjectReference],
    all_users: &mut HashMap<String, ResolvedBasicAuthUser>,
    seen: &mut HashMap<String, String>,
    ep_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    for sor in secret_refs {
        let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_namespace), &sor.name);
        let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", sor) {
            FetchOutcome::Ok(s) => s,
            FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                tracing::info!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    "BasicAuth: Secret not found yet, will be reprocessed when Secret arrives"
                );
                continue;
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
                    username = %username,
                    existing_group = %prev_group,
                    incoming_group = %group_name,
                    "BasicAuth: duplicate username across groups; first-wins kept"
                );
            } else {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    username = %username,
                    group = %group_name,
                    "BasicAuth: Duplicate username within group, skipping"
                );
            }
        }
    }
}

/// Resolve BasicAuth users from Secrets and register references to SecretRefManager.
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::BasicAuth(ref mut config) = entry.plugin {
                config.resolved_users = None;

                let Some(ref groups) = config.secret_groups else {
                    continue;
                };

                let mut all_users: HashMap<String, ResolvedBasicAuthUser> = HashMap::new();
                let mut seen: HashMap<String, String> = HashMap::new();

                for g in groups {
                    resolve_group(
                        &g.name,
                        &g.secret_refs,
                        &mut all_users,
                        &mut seen,
                        ep.metadata.namespace.as_ref(),
                        resource_ref,
                        ctx,
                    );
                }

                if !all_users.is_empty() {
                    tracing::info!(
                        edgion_plugins = %resource_ref.key(),
                        user_count = all_users.len(),
                        group_count = groups.len(),
                        "BasicAuth: Resolved users from Secret groups"
                    );
                    config.resolved_users = Some(all_users);
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        "BasicAuth: No users resolved from Secret groups"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_user(password: &str, group: &str) -> ResolvedBasicAuthUser {
        ResolvedBasicAuthUser {
            password: password.to_string(),
            group: group.to_string(),
        }
    }

    #[test]
    fn first_wins_second_group_rejected() {
        let mut seen: HashMap<String, String> = HashMap::new();
        let mut all_users: HashMap<String, ResolvedBasicAuthUser> = HashMap::new();

        let inserted = first_wins_insert(
            &mut seen,
            &mut all_users,
            "alice".to_string(),
            make_user("pass-a", "partner-a"),
            "partner-a".to_string(),
        );
        assert!(inserted, "first insert must succeed");

        let inserted_again = first_wins_insert(
            &mut seen,
            &mut all_users,
            "alice".to_string(),
            make_user("pass-b", "partner-b"),
            "partner-b".to_string(),
        );
        assert!(!inserted_again, "second insert must be rejected (first-wins)");

        let stored = all_users.get("alice").unwrap();
        assert_eq!(stored.group.as_str(), "partner-a", "first group must win");
        assert_eq!(stored.password.as_str(), "pass-a", "partner-a password must be kept");

        assert_eq!(seen.len(), 1);
        assert_eq!(seen.get("alice").unwrap().as_str(), "partner-a");
    }

    #[test]
    fn first_wins_distinct_usernames_both_inserted() {
        let mut seen: HashMap<String, String> = HashMap::new();
        let mut all_users: HashMap<String, ResolvedBasicAuthUser> = HashMap::new();

        assert!(first_wins_insert(
            &mut seen,
            &mut all_users,
            "alice".to_string(),
            make_user("pass-a", "partner-a"),
            "partner-a".to_string(),
        ));
        assert!(first_wins_insert(
            &mut seen,
            &mut all_users,
            "bob".to_string(),
            make_user("pass-b", "partner-b"),
            "partner-b".to_string(),
        ));

        assert_eq!(all_users.len(), 2);
        assert_eq!(all_users.get("alice").unwrap().group.as_str(), "partner-a");
        assert_eq!(all_users.get("bob").unwrap().group.as_str(), "partner-b");
    }
}
