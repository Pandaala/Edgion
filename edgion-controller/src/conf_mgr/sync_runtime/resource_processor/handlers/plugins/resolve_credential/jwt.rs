//! JWT credential resolution.

use std::collections::HashMap;

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    JwtIssuerGroup, ResolvedJwtCredential, ResolvedJwtIssuerGroup,
};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Resolve all Secret refs for a single JwtIssuerGroup. Snapshot-merges
/// `algorithm` + `audiences` from group override or plugin-level default.
/// Returns `None` if no credentials could be parsed (logged as warn).
fn resolve_group(
    config: &edgion_resources::resources::edgion_plugins::plugin_configs::JwtAuthConfig,
    group: &JwtIssuerGroup,
    ep_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) -> Option<ResolvedJwtIssuerGroup> {
    let mut credentials: HashMap<String, ResolvedJwtCredential> = HashMap::new();

    for sor in &group.secret_refs {
        let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_namespace), &sor.name);
        let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", sor) {
            FetchOutcome::Ok(s) => s,
            FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                tracing::info!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group.name,
                    "JwtAuth: Secret not found yet, will be reprocessed when Secret arrives"
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
            .and_then(|v| String::from_utf8(v.0.clone()).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let secret_str = data
            .get("secret")
            .and_then(|v| String::from_utf8(v.0.clone()).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let public_key_str = data
            .get("publicKey")
            .and_then(|v| String::from_utf8(v.0.clone()).ok())
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
                kid = %kid,
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
                kid = %kid,
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
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::JwtAuth(ref mut config) = entry.plugin {
                let Some(ref groups) = config.issuer_groups else {
                    continue;
                };

                let mut by_iss: HashMap<String, ResolvedJwtIssuerGroup> = HashMap::new();
                for group in groups {
                    let Some(resolved) =
                        resolve_group(config, group, ep.metadata.namespace.as_ref(), resource_ref, ctx)
                    else {
                        continue;
                    };
                    // Index by every iss this group claims. Cross-group iss
                    // uniqueness is already enforced at preparse, so no
                    // collision here. Each iss carries its own clone of the
                    // resolved group to keep serde simple.
                    for iss in &group.issuers {
                        by_iss.insert(iss.clone(), resolved.clone());
                    }
                }

                if !by_iss.is_empty() {
                    tracing::info!(
                        edgion_plugins = %resource_ref.key(),
                        group_count = groups.len(),
                        iss_count = by_iss.len(),
                        "JwtAuth: Resolved issuer groups"
                    );
                    config.resolved_groups_by_iss = Some(by_iss);
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        "JwtAuth: No issuer groups resolved"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use edgion_resources::resources::edgion_plugins::plugin_configs::{JwtAlgorithm, JwtIssuerGroup};
    use edgion_resources::resources::gateway::SecretObjectReference;

    fn make_group_with_overrides(
        name: &str,
        algorithm: Option<JwtAlgorithm>,
        audiences: Option<Vec<String>>,
    ) -> JwtIssuerGroup {
        JwtIssuerGroup {
            name: name.into(),
            description: None,
            issuers: vec!["https://x.example".into()],
            secret_refs: vec![SecretObjectReference {
                group: None,
                kind: None,
                name: "s1".into(),
                namespace: None,
            }],
            algorithm,
            audiences,
        }
    }

    // Tiny pure helper that mirrors the snapshot-merge core decision in
    // `resolve_group` without requiring fetch_secret/HandlerContext.
    fn snapshot_merge(
        config_algorithm: JwtAlgorithm,
        config_audiences: Option<Vec<String>>,
        group: &JwtIssuerGroup,
    ) -> (JwtAlgorithm, Option<Vec<String>>) {
        (
            group.algorithm.unwrap_or(config_algorithm),
            group.audiences.clone().or(config_audiences),
        )
    }

    #[test]
    fn snapshot_uses_group_algorithm_when_set() {
        let group = make_group_with_overrides("g", Some(JwtAlgorithm::RS256), None);
        let (alg, _) = snapshot_merge(JwtAlgorithm::HS256, None, &group);
        assert_eq!(alg, JwtAlgorithm::RS256, "group override must win");
    }

    #[test]
    fn snapshot_falls_back_to_plugin_algorithm() {
        let group = make_group_with_overrides("g", None, None);
        let (alg, _) = snapshot_merge(JwtAlgorithm::HS384, None, &group);
        assert_eq!(alg, JwtAlgorithm::HS384, "plugin-level fallback");
    }

    #[test]
    fn snapshot_preserves_explicit_empty_audiences_override() {
        // Group sets `Some(vec![])` (= "no aud check for this group").
        // Must NOT fall back to plugin-level audiences.
        let group = make_group_with_overrides("g", None, Some(vec![]));
        let plugin_auds = Some(vec!["plugin-aud".to_string()]);
        let (_, auds) = snapshot_merge(JwtAlgorithm::HS256, plugin_auds, &group);
        assert_eq!(
            auds,
            Some(vec![]),
            "Some(vec![]) must be preserved as explicit no-aud override"
        );
    }
}
