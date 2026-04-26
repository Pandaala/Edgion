//! HMAC credential resolution.

use std::collections::{HashMap, HashSet};

use super::read_secret_utf8;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, PendingReason, ResolveOutcome,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ResourceRef,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::edgion_plugins::plugin_configs::HmacCredential;
use crate::types::resources::edgion_plugins::EdgionPlugin;

/// Resolve HMAC credentials from Secrets and register references to SecretRefManager.
pub(in crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::HmacAuth(ref mut config) = entry.plugin {
                config.resolved_credentials = None;

                let Some(secret_refs) = config.secret_refs.clone() else {
                    continue;
                };
                if secret_refs.is_empty() {
                    continue;
                }

                let whitelist: HashSet<&str> = config.upstream_header_fields.iter().map(|v| v.as_str()).collect();
                let mut resolved: HashMap<String, HmacCredential> = HashMap::new();

                for sor in &secret_refs {
                    let secret_key =
                        format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                    let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", sor) {
                        ResolveOutcome::Ok(s) => s,
                        ResolveOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "HmacAuth: Secret not found yet, will be reprocessed when Secret arrives"
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
                                "HmacAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                            );
                            continue;
                        }
                        ResolveOutcome::Invalid(_) => continue,
                    };

                    let Some(credentials_yaml) = read_secret_utf8(&secret, &["credentials.yaml"]) else {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "HmacAuth: Secret missing credentials.yaml"
                        );
                        continue;
                    };

                    let entries: Vec<HashMap<String, serde_yaml::Value>> = match serde_yaml::from_str(&credentials_yaml)
                    {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                error = %e,
                                "HmacAuth: Failed to parse credentials.yaml"
                            );
                            continue;
                        }
                    };

                    for entry_map in entries {
                        let username = entry_map
                            .get(&config.username_field)
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|v| !v.is_empty());
                        let secret_value = entry_map
                            .get(&config.secret_field)
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|v| !v.is_empty());

                        let (Some(username), Some(secret_value)) = (username, secret_value) else {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                username_field = %config.username_field,
                                secret_field = %config.secret_field,
                                "HmacAuth: credential entry missing username/secret field"
                            );
                            continue;
                        };

                        if resolved.contains_key(username) {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                username = %username,
                                "HmacAuth: duplicate username found in credentials, skipping"
                            );
                            continue;
                        }

                        if secret_value.len() < config.min_secret_length {
                            tracing::warn!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                username = %username,
                                secret_len = secret_value.len(),
                                min_len = config.min_secret_length,
                                "HmacAuth: secret length is below configured minimum"
                            );
                        }

                        let mut headers = HashMap::new();
                        if let Some(serde_yaml::Value::Mapping(map)) = entry_map.get("headers") {
                            for (k, v) in map {
                                let Some(header_name) = k.as_str() else {
                                    continue;
                                };
                                if !whitelist.contains(header_name) {
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

                        resolved.insert(
                            username.to_string(),
                            HmacCredential {
                                secret: secret_value.as_bytes().to_vec(),
                                headers,
                            },
                        );
                    }
                }

                if !resolved.is_empty() {
                    tracing::info!(
                        edgion_plugins = %resource_ref.key(),
                        credential_count = resolved.len(),
                        "HmacAuth: Resolved credentials from Secrets"
                    );
                    config.resolved_credentials = Some(resolved);
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        "HmacAuth: No credentials resolved from Secrets"
                    );
                }
            }
        }
    }
}
