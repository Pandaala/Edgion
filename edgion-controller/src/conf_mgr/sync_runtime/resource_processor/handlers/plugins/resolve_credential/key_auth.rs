//! KeyAuth credential resolution.

use std::collections::{HashMap, HashSet};

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::KeyMetadata;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Resolve all Secret refs for a single group, appending entries to
/// `all_keys`. Cross-group duplicate keys follow **first-wins**: if a
/// key already exists in `seen_groups_for_key`, the new entry is dropped
/// and a warn is logged with both groups.
#[allow(clippy::too_many_arguments)]
fn resolve_group(
    config_key_field: &str,
    whitelist: &HashSet<&str>,
    group_name: &str,
    secret_refs: &[edgion_resources::resources::gateway::SecretObjectReference],
    all_keys: &mut HashMap<String, KeyMetadata>,
    seen_groups_for_key: &mut HashMap<String, String>,
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
                    "KeyAuth: Secret not found yet, will be reprocessed when Secret arrives"
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
        let keys_yaml_str = match String::from_utf8(keys_yaml_bytes.0.clone()) {
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

        let keys_list: Vec<HashMap<String, String>> = match serde_yaml::from_str(&keys_yaml_str) {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    error = %e,
                    "KeyAuth: Failed to parse keys.yaml"
                );
                continue;
            }
        };

        for key_entry in keys_list {
            let Some(key_value) = key_entry.get(config_key_field) else {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    key_field = %config_key_field,
                    "KeyAuth: Key entry missing key field"
                );
                continue;
            };

            let mut headers: HashMap<String, String> = HashMap::new();
            for (field, value) in &key_entry {
                if field != config_key_field && whitelist.contains(field.as_str()) {
                    headers.insert(field.clone(), value.clone());
                }
            }

            let inserted = first_wins_insert(
                seen_groups_for_key,
                all_keys,
                key_value.clone(),
                headers,
                group_name.to_string(),
            );

            if !inserted {
                // first-wins rejected this entry; warn with both groups (or the
                // single group for an intra-group duplicate). secret_key keeps
                // operators able to locate the offending Secret entry.
                let prev_group = seen_groups_for_key.get(key_value).cloned().unwrap_or_default();
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
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::KeyAuth(ref mut config) = entry.plugin {
                // This field is Controller-owned transport state. Never trust
                // a value deserialized from the operator-facing resource, and
                // do not retain credentials from a previous resolution pass.
                config.resolved_keys = None;

                let Some(ref groups) = config.secret_groups else {
                    continue;
                };

                let whitelist: HashSet<&str> = config.upstream_header_fields.iter().map(|s| s.as_str()).collect();
                let key_field = config.key_field.clone();

                let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();
                let mut seen: HashMap<String, String> = HashMap::new();

                for g in groups {
                    let group_name: String = g.name.clone();
                    resolve_group(
                        &key_field,
                        &whitelist,
                        &group_name,
                        &g.secret_refs,
                        &mut all_keys,
                        &mut seen,
                        ep.metadata.namespace.as_ref(),
                        resource_ref,
                        ctx,
                    );
                }

                if !all_keys.is_empty() {
                    tracing::info!(
                        edgion_plugins = %resource_ref.key(),
                        key_count = all_keys.len(),
                        group_count = groups.len(),
                        "KeyAuth: Resolved API keys from Secret groups"
                    );
                    config.resolved_keys = Some(all_keys);
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        "KeyAuth: No API keys resolved from Secret groups"
                    );
                }
            }
        }
    }
}

/// Pure helper for first-wins duplicate detection.
///
/// Inserts `key` into `all_keys` with the given `headers` and `group` only if
/// `seen` does not already contain `key`. Returns `true` if inserted, `false`
/// if the key was already present (first-wins: original entry is kept).
fn first_wins_insert(
    seen: &mut HashMap<String, String>,
    all_keys: &mut HashMap<String, KeyMetadata>,
    key: String,
    headers: HashMap<String, String>,
    group: String,
) -> bool {
    if seen.contains_key(&key) {
        return false;
    }
    seen.insert(key.clone(), group.clone());
    all_keys.insert(key, KeyMetadata { headers, group });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_wins_second_group_rejected() {
        let mut seen: HashMap<String, String> = HashMap::new();
        let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();

        let group_a: String = "partner-a".to_string();
        let group_b: String = "partner-b".to_string();

        // First group inserts the key — should succeed.
        let inserted = first_wins_insert(
            &mut seen,
            &mut all_keys,
            "dup-key".to_string(),
            HashMap::new(),
            group_a.clone(),
        );
        assert!(inserted, "first insert must succeed");

        // Second group tries the same key — must be rejected.
        let inserted_again =
            first_wins_insert(&mut seen, &mut all_keys, "dup-key".to_string(), HashMap::new(), group_b);
        assert!(!inserted_again, "second insert must be rejected (first-wins)");

        // Verify first-wins: the stored group is partner-a, not partner-b.
        assert_eq!(
            all_keys.get("dup-key").unwrap().group.as_str(),
            "partner-a",
            "first group must win"
        );

        // Only one entry in seen (the second was rejected).
        assert_eq!(seen.len(), 1, "seen must track exactly one entry");
        assert_eq!(seen.get("dup-key").unwrap().as_str(), "partner-a");
    }

    #[test]
    fn first_wins_distinct_keys_both_inserted() {
        let mut seen: HashMap<String, String> = HashMap::new();
        let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();

        let group_a: String = "partner-a".to_string();
        let group_b: String = "partner-b".to_string();

        assert!(first_wins_insert(
            &mut seen,
            &mut all_keys,
            "key-a".to_string(),
            HashMap::new(),
            group_a,
        ));
        assert!(first_wins_insert(
            &mut seen,
            &mut all_keys,
            "key-b".to_string(),
            HashMap::new(),
            group_b,
        ));

        assert_eq!(all_keys.len(), 2);
        assert_eq!(all_keys.get("key-a").unwrap().group.as_str(), "partner-a");
        assert_eq!(all_keys.get("key-b").unwrap().group.as_str(), "partner-b");
    }
}
