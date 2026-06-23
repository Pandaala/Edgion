//! GlobalAccessControl secret-header resolution.
//!
//! For each `RestrictionRule` inside a `GlobalAccessControl` plugin that carries a
//! `secret_ref`, read the referenced Secret and populate `resolved_secret_values` with
//! ALL non-empty string values in the Secret (supports credential rotation: any current
//! value matches). The values are matched exact/case-sensitive at the gateway, so the
//! RAW decoded value is used — a trailing newline in the Secret will NOT match a clean
//! header value (operator footgun; keep Secret values free of surrounding whitespace).

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Resolve GlobalAccessControl rule secret_refs into `resolved_secret_values`.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Some(ref mut plugins) = ep.spec.request_plugins else {
        return;
    };
    for entry in plugins.iter_mut() {
        let EdgionPlugin::GlobalAccessControl(ref mut config) = entry.plugin else {
            continue;
        };
        for profile in config.profiles.values_mut() {
            for group in profile.rule_groups.iter_mut() {
                for rule in group.rules.iter_mut() {
                    // Clear stale value so a removed secretRef drops its resolution.
                    rule.resolved_secret_values = None;

                    let Some(sor) = rule.secret_ref.clone() else {
                        continue;
                    };
                    let secret_key =
                        format_secret_key(sor.namespace.as_ref().or(ep.metadata.namespace.as_ref()), &sor.name);
                    let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", &sor) {
                        FetchOutcome::Ok(s) => s,
                        FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "GlobalAccessControl: Secret not found yet, will be reprocessed when Secret arrives"
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
                                "GlobalAccessControl: cross-namespace Secret reference denied (ReferenceGrant missing)"
                            );
                            continue;
                        }
                    };

                    // Collect every non-empty string value (rotation: any current value matches).
                    // The RAW decoded value is preserved; skip only values that are entirely whitespace.
                    let values: Vec<String> = secret
                        .data
                        .iter()
                        .flatten()
                        .filter_map(|(_k, v)| String::from_utf8(v.0.clone()).ok())
                        .filter(|s| !s.trim().is_empty())
                        .collect();

                    if values.is_empty() {
                        tracing::warn!(
                            edgion_plugins = %resource_ref.key(),
                            secret_key = %secret_key,
                            "GlobalAccessControl: referenced Secret has no usable string values"
                        );
                        continue;
                    }
                    rule.resolved_secret_values = Some(values);
                }
            }
        }
    }
}
