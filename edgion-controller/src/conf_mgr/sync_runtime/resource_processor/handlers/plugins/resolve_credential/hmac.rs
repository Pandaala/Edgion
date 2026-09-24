//! HMAC credential resolution.

use std::collections::{HashMap, HashSet};

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::HmacCredential;
use edgion_resources::resources::edgion_plugins::plugin_configs::HmacMatchConfig;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Pure helper for first-wins duplicate-username detection.
///
/// Inserts `username -> credential` into `all_credentials` (and tracks the
/// group in `seen`) only if `seen` does not already contain `username`.
/// Returns `true` if inserted, `false` if rejected (first-wins: existing
/// entry kept).
fn first_wins_insert(
    seen: &mut HashMap<String, String>,
    all_credentials: &mut HashMap<String, HmacCredential>,
    username: String,
    credential: HmacCredential,
    group: String,
) -> bool {
    if seen.contains_key(&username) {
        return false;
    }
    seen.insert(username.clone(), group);
    all_credentials.insert(username, credential);
    true
}

/// Resolve all Secret refs for a single group; append entries to
/// `all_credentials`. Cross-group duplicate usernames follow **first-wins**
/// (the entry kept is the one from the group appearing earlier in the
/// `secret_groups` array). `min_secret_length` warn is emitted only for
/// entries that successfully insert.
#[allow(clippy::too_many_arguments)]
fn resolve_group(
    config: &HmacMatchConfig,
    metadata_whitelist: &HashSet<&str>,
    group_name: &str,
    secret_refs: &[edgion_resources::resources::gateway::SecretObjectReference],
    all_credentials: &mut HashMap<String, HmacCredential>,
    seen: &mut HashMap<String, String>,
    ep_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    // Hoist YAML keys out of the per-entry loop to avoid cloning config strings
    // for every credential entry.
    let username_key = serde_yaml::Value::String(config.username_field.clone());
    let secret_key_yaml = serde_yaml::Value::String(config.secret_field.clone());

    for sor in secret_refs {
        let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_namespace), &sor.name);
        let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", sor) {
            FetchOutcome::Ok(s) => s,
            FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                tracing::info!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    "HmacAuth: Secret not found yet, will be reprocessed when Secret arrives"
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
                    "HmacAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                );
                continue;
            }
        };

        let Some(data) = &secret.data else {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group_name,
                "HmacAuth: Secret has no data"
            );
            continue;
        };
        let Some(credentials_yaml_bytes) = data.get("credentials.yaml") else {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group_name,
                "HmacAuth: Secret missing 'credentials.yaml' field"
            );
            continue;
        };
        let credentials_yaml_str = match std::str::from_utf8(&credentials_yaml_bytes.0) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    error = %e,
                    "HmacAuth: Failed to decode credentials.yaml as UTF-8"
                );
                continue;
            }
        };

        let entries: Vec<serde_yaml::Value> = match serde_yaml::from_str(credentials_yaml_str) {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    line = e.location().map(|location| location.line()),
                    column = e.location().map(|location| location.column()),
                    "HmacAuth: Failed to parse credentials.yaml"
                );
                continue;
            }
        };

        for entry in entries {
            let serde_yaml::Value::Mapping(entry_map) = &entry else {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    "HmacAuth: credential entry is not a mapping"
                );
                continue;
            };

            // Reject empty/whitespace-only username and secret — without
            // trim+filter, `username: ""` or `secret: " "` would silently
            // insert as live credentials.
            let username = entry_map
                .get(&username_key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty());
            let secret_value = entry_map
                .get(&secret_key_yaml)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty());

            let (Some(username), Some(secret_value)) = (username, secret_value) else {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    username_field = %config.username_field,
                    secret_field = %config.secret_field,
                    group = %group_name,
                    "HmacAuth: credential entry missing or empty username/secret field"
                );
                continue;
            };

            // Flatten nested headers: subtable (filtered by whitelist).
            let mut headers: HashMap<String, String> = HashMap::new();
            if let Some(serde_yaml::Value::Mapping(map)) = entry_map.get("headers") {
                for (k, v) in map {
                    let Some(header_name) = k.as_str() else {
                        continue;
                    };
                    if !metadata_whitelist.contains(header_name) {
                        continue;
                    }
                    let header_value = if let Some(value) = v.as_str() {
                        value.to_string()
                    } else {
                        match v {
                            serde_yaml::Value::Bool(b) => b.to_string(),
                            serde_yaml::Value::Number(n) => n.to_string(),
                            _ => continue,
                        }
                    };
                    headers.insert(header_name.to_string(), header_value);
                }
            }

            let credential = HmacCredential {
                secret: secret_value.as_bytes().to_vec(),
                headers,
                group: group_name.to_string(),
            };

            let inserted = first_wins_insert(
                seen,
                all_credentials,
                username.to_string(),
                credential,
                group_name.to_string(),
            );

            if inserted {
                // Only warn on length for entries actually retained — avoids
                // noise for first-wins-rejected duplicates.
                if secret_value.len() < config.min_secret_length {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        group = %group_name,
                        secret_len = secret_value.len(),
                        min_len = config.min_secret_length,
                        "HmacAuth: secret length is below configured minimum"
                    );
                }
            } else {
                // `secret_key` in the warns below identifies the Secret whose
                // entry was *dropped* (the latter occurrence); the originally
                // retained entry lives in the Secret backing `existing_group`
                // for cross-group dups, or earlier in the same Secret for
                // intra-group dups. Operators triaging dup warnings should
                // treat `secret_key` as the "loser" pointer.
                let prev_group = seen.get(username).cloned().unwrap_or_default();
                if prev_group != group_name {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        existing_group = %prev_group,
                        incoming_group = %group_name,
                        "HmacAuth: duplicate username across groups; first-wins kept"
                    );
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        group = %group_name,
                        "HmacAuth: Duplicate username within group, skipping"
                    );
                }
            }
        }
    }
}

/// Resolve HMAC credentials from Secrets and register references to SecretRefManager.
pub(super) fn resolve_config(
    config: &mut HmacMatchConfig,
    metadata_policy: MetadataPolicy<'_>,
    ep_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    config.resolved_credentials = None;
    let Some(groups) = config.secret_groups.clone() else {
        return;
    };
    let mut all_credentials: HashMap<String, HmacCredential> = HashMap::new();
    let mut seen: HashMap<String, String> = HashMap::new();
    let metadata_whitelist: HashSet<&str> = match metadata_policy {
        MetadataPolicy::Allow(fields) => fields.iter().map(String::as_str).collect(),
        MetadataPolicy::Ignore => HashSet::new(),
    };
    for group in &groups {
        resolve_group(
            config,
            &metadata_whitelist,
            &group.name,
            &group.secret_refs,
            &mut all_credentials,
            &mut seen,
            ep_namespace,
            resource_ref,
            ctx,
        );
    }
    if !all_credentials.is_empty() {
        tracing::info!(
            edgion_plugins = %resource_ref.key(),
            credential_count = all_credentials.len(),
            group_count = groups.len(),
            "HmacAuth: Resolved credentials from Secret groups"
        );
        config.resolved_credentials = Some(all_credentials);
    } else {
        tracing::warn!(
            edgion_plugins = %resource_ref.key(),
            "HmacAuth: No credentials resolved from Secret groups"
        );
    }
}

#[derive(Clone, Copy)]
pub(super) enum MetadataPolicy<'a> {
    Allow(&'a [String]),
    Ignore,
}

/// Resolve top-level HMAC plugin credentials.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::HmacAuth(ref mut config) = entry.plugin {
                let fields = config.upstream_header_fields.clone();
                resolve_config(
                    &mut config.match_config,
                    MetadataPolicy::Allow(&fields),
                    ep.metadata.namespace.as_ref(),
                    resource_ref,
                    ctx,
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/hmac_test.rs"]
mod tests;
