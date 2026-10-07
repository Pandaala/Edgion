//! JWT credential resolution.

use std::collections::HashMap;

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_bound_secret, BoundFetchOutcome, PendingReason,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    JwtIssuerGroup, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::refs::plugin_fields::{
    BoundPluginEntryMut, PreparedPluginReferences, SecretGroupReferences,
};

/// Resolve all Secret refs for a single JwtIssuerGroup. Snapshot-merges
/// `algorithm` + `audiences` from group override or plugin-level default.
/// Returns `None` if no credentials could be parsed (logged as warn).
fn resolve_group(
    config: &edgion_resources::resources::edgion_plugins::plugin_configs::JwtAuthConfig,
    group: &JwtIssuerGroup,
    fields: &SecretGroupReferences<'_>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) -> Option<ResolvedJwtIssuerGroup> {
    let mut credentials: HashMap<String, ResolvedJwtCredential> = HashMap::new();

    for token in fields.references() {
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
                    group = %group.name,
                    "JwtAuth: Secret not found yet, will be reprocessed when Secret arrives"
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
                    group = %group.name,
                    "JwtAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                );
                continue;
            }
        };

        let Some(data) = &secret.data else {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group.name,
                "JwtAuth: Secret has no data"
            );
            continue;
        };

        // JwtAuth Secret format: each Secret carries ONE credential with a
        // `key` field (kid string) and one of `secret` (HS*) or `publicKey`
        // (RS*/ES*).
        let kid = data
            .get("key")
            .and_then(|v| std::str::from_utf8(&v.0).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let secret_str = data
            .get("secret")
            .and_then(|v| std::str::from_utf8(&v.0).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let public_key_str = data
            .get("publicKey")
            .and_then(|v| std::str::from_utf8(&v.0).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let Some(kid) = kid else {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group.name,
                "JwtAuth: Secret missing 'key' field"
            );
            continue;
        };

        if secret_str.is_none() && public_key_str.is_none() {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group.name,
                "JwtAuth: Secret missing both 'secret' and 'publicKey' fields"
            );
            continue;
        }

        if credentials.contains_key(&kid) {
            // Intra-group duplicate kid — secret_key identifies the dropped
            // entry; the first occurrence is kept. No secret value in the
            // warn to avoid credential leakage.
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group.name,
                "JwtAuth: Duplicate kid within group, skipping"
            );
            continue;
        }

        credentials.insert(
            kid.clone(),
            ResolvedJwtCredential {
                key: Some(kid),
                secret: secret_str,
                public_key: public_key_str,
            },
        );
    }

    if credentials.is_empty() {
        tracing::warn!(
            edgion_plugins = %resource_ref.key(),
            group = %group.name,
            "JwtAuth: group has no credentials resolved"
        );
        return None;
    }

    // Snapshot merge: group override or plugin-level default.
    // `or_else` preserves `Some(vec![])` as explicit "no aud check" override.
    Some(ResolvedJwtIssuerGroup {
        group: group.name.clone(),
        algorithm: group.algorithm.unwrap_or(config.algorithm),
        audiences: group.audiences.clone().or_else(|| config.audiences.clone()),
        credentials,
    })
}

/// Resolve JWT credentials from Secrets and register references to SecretRefManager.
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
    let EdgionPlugin::JwtAuth(config) = entry.config_mut() else {
        return;
    };
    config.resolved_groups_by_iss = None;
    if !entry.is_enabled() {
        return;
    }
    let mut by_iss = HashMap::new();
    {
        let EdgionPlugin::JwtAuth(config) = entry.config() else {
            return;
        };
        let Ok(references) = entry.references() else {
            return;
        };
        for (group, fields) in config
            .issuer_groups
            .as_ref()
            .into_iter()
            .flatten()
            .zip(references.secret_groups())
        {
            let Some(resolved) = resolve_group(config, group, &fields, resource_ref, ctx) else {
                continue;
            };
            for iss in &group.issuers {
                by_iss.insert(iss.clone(), resolved.clone());
            }
        }
    }
    if let EdgionPlugin::JwtAuth(config) = entry.config_mut() {
        config.resolved_groups_by_iss = (!by_iss.is_empty()).then_some(by_iss);
    }
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/jwt_test.rs"]
mod tests;
