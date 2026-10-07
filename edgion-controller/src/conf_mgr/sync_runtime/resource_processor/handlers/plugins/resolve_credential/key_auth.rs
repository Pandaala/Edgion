//! KeyAuth credential resolution.

use std::collections::{hash_map::Entry, HashMap, HashSet};

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_bound_secret, BoundFetchOutcome, PendingReason,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::KeyMetadata;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::refs::plugin_fields::{
    BoundPluginEntryMut, PreparedPluginReferences, SecretGroupReferences,
};

/// Resolve all Secret refs for a single group, appending entries to
/// `all_keys`. Cross-group duplicate keys follow **first-wins**: if a
/// key already exists in `all_keys`, the new entry is dropped
/// and a warn is logged with both groups.
#[allow(clippy::too_many_arguments)]
fn resolve_group(
    config_key_field: &str,
    whitelist: &HashSet<&str>,
    group: &SecretGroupReferences<'_>,
    all_keys: &mut HashMap<String, KeyMetadata>,
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
                    "KeyAuth: Secret not found yet, will be reprocessed when Secret arrives"
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
                    "KeyAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                );
                continue;
            }
        };

        let Some(data) = &secret.data else {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group_name,
                "KeyAuth: Secret has no data"
            );
            continue;
        };
        let Some(keys_yaml_bytes) = data.get("keys.yaml") else {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group_name,
                "KeyAuth: Secret missing 'keys.yaml' field"
            );
            continue;
        };
        let keys_yaml_str = match std::str::from_utf8(&keys_yaml_bytes.0) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    error = %e,
                    "KeyAuth: Failed to decode keys.yaml as UTF-8"
                );
                continue;
            }
        };

        let keys_list: Vec<HashMap<String, String>> = match serde_yaml::from_str(keys_yaml_str) {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    line = e.location().map(|location| location.line()),
                    column = e.location().map(|location| location.column()),
                    "KeyAuth: Failed to parse keys.yaml"
                );
                continue;
            }
        };

        for mut key_entry in keys_list {
            let Some(key_value) = key_entry.remove(config_key_field) else {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    key_field = %config_key_field,
                    "KeyAuth: Key entry missing key field"
                );
                continue;
            };

            let headers = key_entry
                .into_iter()
                .filter(|(field, _)| whitelist.contains(field.as_str()))
                .collect();

            if let Err(prev_group) = first_wins_insert(all_keys, key_value, headers, group_name) {
                // first-wins rejected this entry; warn with both groups (or the
                // single group for an intra-group duplicate). secret_key keeps
                // operators able to locate the offending Secret entry.
                if prev_group != group_name {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        existing_group = %prev_group,
                        incoming_group = %group_name,
                        "KeyAuth: duplicate API key across groups; first-wins kept"
                    );
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        group = %group_name,
                        "KeyAuth: Duplicate API key within group, skipping"
                    );
                }
            }
        }
    }
}

/// Resolve KeyAuth keys from Secrets and register references to SecretRefManager.
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
    let EdgionPlugin::KeyAuth(config) = entry.config_mut() else {
        return;
    };
    config.resolved_keys = None;
    if !entry.is_enabled() {
        return;
    }
    let mut all_keys = HashMap::new();
    {
        let EdgionPlugin::KeyAuth(config) = entry.config() else {
            return;
        };
        let whitelist: HashSet<&str> = config.upstream_header_fields.iter().map(String::as_str).collect();
        let Ok(references) = entry.references() else {
            return;
        };
        for group in references.secret_groups() {
            resolve_group(&config.key_field, &whitelist, &group, &mut all_keys, resource_ref, ctx);
        }
    }
    if let EdgionPlugin::KeyAuth(config) = entry.config_mut() {
        config.resolved_keys = (!all_keys.is_empty()).then_some(all_keys);
    }
}

/// Pure helper for first-wins duplicate detection.
///
/// Inserts `key` into `all_keys` with the given `headers` and `group` only if
/// `all_keys` does not already contain `key`. Returns the retained group on a
/// duplicate (first-wins: original entry is kept), without copying its name.
fn first_wins_insert<'a>(
    all_keys: &'a mut HashMap<String, KeyMetadata>,
    key: String,
    headers: HashMap<String, String>,
    group: &str,
) -> Result<(), &'a str> {
    match all_keys.entry(key) {
        Entry::Occupied(entry) => Err(&entry.into_mut().group),
        Entry::Vacant(entry) => {
            entry.insert(KeyMetadata {
                headers,
                group: group.to_string(),
            });
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/key_auth_test.rs"]
mod tests;
