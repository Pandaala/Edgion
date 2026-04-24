//! KeyAuth credential resolution.

use std::collections::{HashMap, HashSet};

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, PendingReason, ResolveOutcome,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ResourceRef,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::common::SecretReference;
use crate::types::resources::edgion_plugins::plugin_configs::KeyMetadata;
use crate::types::resources::edgion_plugins::EdgionPlugin;

/// Resolve KeyAuth keys from Secrets and register references to SecretRefManager.
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    // Process request plugins
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::KeyAuth(ref mut config) = entry.plugin {
                let Some(ref secret_refs) = config.secret_refs else {
                    continue;
                };

                let whitelist: HashSet<&str> = config.upstream_header_fields.iter().map(|s| s.as_str()).collect();
                let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();

                for sor in secret_refs {
                    let sref = SecretReference {
                        name: sor.name.clone(),
                        namespace: sor.namespace.clone(),
                    };
                    let secret_key =
                        format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                    let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sref) {
                        ResolveOutcome::Ok(s) => s,
                        ResolveOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "KeyAuth: Secret not found yet, will be reprocessed when Secret arrives"
                            );
                            continue;
                        }
                        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                            ref from_ns,
                            ref to_ns,
                            ref secret_name,
                        }) => {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                from_ns = %from_ns,
                                to_ns = %to_ns,
                                secret_name = %secret_name,
                                "KeyAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                            );
                            continue;
                        }
                        ResolveOutcome::Invalid(_) => continue,
                    };

                    // Try to get keys.yaml from data (base64 decoded) or string_data (plain text)
                    let keys_yaml_str: String = if let Some(data) = &secret.data {
                        // Try data field first (K8s stores decoded bytes here)
                        if let Some(keys_yaml_bytes) = data.get("keys.yaml") {
                            match String::from_utf8(keys_yaml_bytes.0.clone()) {
                                Ok(s) => s,
                                Err(e) => {
                                    tracing::warn!(
                                        edgion_plugins = %resource_ref.key(),
                                        secret_key = %secret_key,
                                        error = %e,
                                        "KeyAuth: Failed to decode keys.yaml as UTF-8"
                                    );
                                    continue;
                                }
                            }
                        } else if let Some(string_data) = &secret.string_data {
                            // Fallback to string_data if data doesn't have the key
                            if let Some(s) = string_data.get("keys.yaml") {
                                s.clone()
                            } else {
                                tracing::warn!(
                                    edgion_plugins = %resource_ref.key(),
                                    secret_key = %secret_key,
                                    "KeyAuth: Secret missing 'keys.yaml' field in both data and string_data"
                                );
                                continue;
                            }
                        } else {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "KeyAuth: Secret missing 'keys.yaml' field"
                            );
                            continue;
                        }
                    } else if let Some(string_data) = &secret.string_data {
                        // No data field, try string_data (used in local file testing)
                        if let Some(s) = string_data.get("keys.yaml") {
                            s.clone()
                        } else {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "KeyAuth: Secret missing 'keys.yaml' field in string_data"
                            );
                            continue;
                        }
                    } else {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "KeyAuth: Secret has no data or string_data"
                        );
                        continue;
                    };

                    // Parse YAML list of key entries
                    let keys_list: Vec<HashMap<String, String>> = match serde_yaml::from_str(&keys_yaml_str) {
                        Ok(list) => list,
                        Err(e) => {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                error = %e,
                                "KeyAuth: Failed to parse keys.yaml"
                            );
                            continue;
                        }
                    };

                    // Process each key entry
                    for key_entry in keys_list {
                        let Some(key_value) = key_entry.get(&config.key_field) else {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                key_field = %config.key_field,
                                "KeyAuth: Key entry missing key field"
                            );
                            continue;
                        };

                        // Extract whitelisted headers
                        let mut metadata = KeyMetadata::default();
                        for (field, value) in &key_entry {
                            if field != &config.key_field && whitelist.contains(field.as_str()) {
                                metadata.headers.insert(field.clone(), value.clone());
                            }
                        }

                        // Check for duplicate keys
                        if all_keys.contains_key(key_value) {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "KeyAuth: Duplicate API key found, skipping"
                            );
                            continue;
                        }

                        all_keys.insert(key_value.clone(), metadata);
                    }
                }

                if !all_keys.is_empty() {
                    tracing::info!(
                        edgion_plugins = %resource_ref.key(),
                        key_count = all_keys.len(),
                        "KeyAuth: Resolved API keys from Secrets"
                    );
                    config.resolved_keys = Some(all_keys);
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        "KeyAuth: No API keys resolved from Secrets"
                    );
                }
            }
        }
    }
}
