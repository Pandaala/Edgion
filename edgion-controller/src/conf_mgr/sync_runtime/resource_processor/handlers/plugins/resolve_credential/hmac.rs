//! HMAC credential resolution.

use std::collections::{HashMap, HashSet};

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_bound_secret, BoundFetchOutcome};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
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

use edgion_resources::resources::edgion_plugins::conditions::Condition;
use edgion_resources::resources::refs::condition_fields::BoundConditionMut;
use edgion_resources::resources::refs::plugin_fields::{BoundPluginEntryMut, PreparedPluginReferences};
use edgion_resources::resources::refs::ReferenceToken;

/// Every caller supplies tokens from its actual owning HMAC groups.
#[allow(clippy::too_many_arguments)]
fn resolve_group<'a>(
    config: &HmacMatchConfig,
    metadata_whitelist: &HashSet<&str>,
    group_name: &str,
    references: impl Iterator<Item = ReferenceToken<'a>>,
    all_credentials: &mut HashMap<String, HmacCredential>,
    seen: &mut HashMap<String, String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    for token in references {
        let Ok(reference) = token.validated() else { continue };
        let target = reference.target();
        let secret_key = format!("{}/{}", target.scope().namespace().unwrap_or_default(), target.name());
        let BoundFetchOutcome::Ok(authorized) = fetch_bound_secret(ctx, resource_ref, token) else {
            continue;
        };
        decode_group_secret(
            config,
            metadata_whitelist,
            group_name,
            &authorized.secret,
            &secret_key,
            all_credentials,
            seen,
            resource_ref,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_group_secret(
    config: &HmacMatchConfig,
    metadata_whitelist: &HashSet<&str>,
    group_name: &str,
    secret: &k8s_openapi::api::core::v1::Secret,
    secret_key: &str,
    all_credentials: &mut HashMap<String, HmacCredential>,
    seen: &mut HashMap<String, String>,
    resource_ref: &ResourceRef,
) {
    let username_key = serde_yaml::Value::String(config.username_field.clone());
    let secret_key_yaml = serde_yaml::Value::String(config.secret_field.clone());
    let Some(data) = &secret.data else {
        tracing::warn!(
            edgion_plugins = %resource_ref.key(),
            secret_key = %secret_key,
            group = %group_name,
            "HmacAuth: Secret has no data"
        );
        return;
    };
    let Some(credentials_yaml_bytes) = data.get("credentials.yaml") else {
        tracing::warn!(
            edgion_plugins = %resource_ref.key(),
            secret_key = %secret_key,
            group = %group_name,
            "HmacAuth: Secret missing 'credentials.yaml' field"
        );
        return;
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
            return;
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
            return;
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

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    prepared: &PreparedPluginReferences,
    owner: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Ok(mut bindings) = prepared.bind_mut(ep) else {
        return;
    };
    for entry in bindings.request_entries() {
        resolve_entry(entry, owner, ctx);
    }
    for entry in bindings.response_filter_entries() {
        resolve_entry(entry, owner, ctx);
    }
    for entry in bindings.response_body_filter_entries() {
        resolve_entry(entry, owner, ctx);
    }
    for entry in bindings.response_entries() {
        resolve_entry(entry, owner, ctx);
    }
}

fn resolve_entry(mut entry: BoundPluginEntryMut<'_>, owner: &ResourceRef, ctx: &HandlerContext) {
    let EdgionPlugin::HmacAuth(config) = entry.config_mut() else {
        return;
    };
    config.match_config.resolved_credentials = None;
    let resolved = {
        let EdgionPlugin::HmacAuth(config) = entry.config() else {
            return;
        };
        let Ok(references) = entry.references() else { return };
        // Metadata policy comes only from this actual standalone placement.
        let whitelist = config.upstream_header_fields.iter().map(String::as_str).collect();
        let mut all_credentials = HashMap::new();
        let mut seen = HashMap::new();
        for group in references.secret_groups() {
            resolve_group(
                &config.match_config,
                &whitelist,
                group.name(),
                group.references(),
                &mut all_credentials,
                &mut seen,
                owner,
                ctx,
            );
        }
        (!all_credentials.is_empty()).then_some(all_credentials)
    };
    if let EdgionPlugin::HmacAuth(config) = entry.config_mut() {
        config.match_config.resolved_credentials = resolved;
    }
}

pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve_condition(
    mut leaf: BoundConditionMut<'_>,
    owner: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Condition::HmacAuth(config) = leaf.config_mut() else {
        return;
    };
    config.resolved_credentials = None;
    let resolved = {
        let Condition::HmacAuth(config) = leaf.config() else {
            return;
        };
        let Ok(references) = leaf.references() else { return };
        // Condition HMAC never publishes credential metadata into upstream headers.
        let whitelist = HashSet::new();
        let mut all_credentials = HashMap::new();
        let mut seen = HashMap::new();
        for group in references.hmac_secret_groups() {
            resolve_group(
                config,
                &whitelist,
                group.name(),
                group.references(),
                &mut all_credentials,
                &mut seen,
                owner,
                ctx,
            );
        }
        (!all_credentials.is_empty()).then_some(all_credentials)
    };
    if let Condition::HmacAuth(config) = leaf.config_mut() {
        config.resolved_credentials = resolved;
    }
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/hmac_test.rs"]
mod tests;
