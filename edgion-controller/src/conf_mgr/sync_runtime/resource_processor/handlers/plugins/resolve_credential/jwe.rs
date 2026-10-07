//! JWE decrypt credential resolution.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_bound_secret, BoundFetchOutcome};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResolvedRefsError, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::ResolvedJweCredential;
use edgion_resources::resources::edgion_plugins::{EdgionPlugin, ResolvedJweCredentialError};
use edgion_resources::resources::refs::plugin_fields::{BoundPluginEntryMut, PreparedPluginReferences};

/// Resolve JWE decrypt credentials from Secrets and register references.
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
    let EdgionPlugin::JweDecrypt(config) = entry.config_mut() else {
        return;
    };
    config.resolved_credential = None;
    if !entry.is_enabled() {
        return;
    }
    let resolved = {
        let Ok(references) = entry.references() else {
            return;
        };
        let Some(token) = references.jwe_secret() else {
            return;
        };
        let BoundFetchOutcome::Ok(authorized) = fetch_bound_secret(ctx, resource_ref, token) else {
            return;
        };
        // A present but unusable Secret remains visible to the content classifier.
        ResolvedJweCredential {
            secret: authorized
                .secret
                .data
                .as_ref()
                .and_then(|data| data.get("secret"))
                .map(|bytes| STANDARD.encode(&bytes.0)),
        }
    };
    if let EdgionPlugin::JweDecrypt(config) = entry.config_mut() {
        config.resolved_credential = Some(resolved);
    }
}

/// Report present-but-unusable JWE Secret material without re-reading a Secret
/// or registering dependencies during status derivation. Missing and denied
/// references leave `resolved_credential` absent and are covered by the shared
/// dependency classifier.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn status_errors(
    ep: &EdgionPlugins,
) -> Vec<ResolvedRefsError> {
    let mut errors = Vec::new();
    let Some(entries) = ep.spec.request_plugins.as_ref() else {
        return errors;
    };
    for entry in entries {
        if !entry.is_enabled() {
            continue;
        }
        let EdgionPlugin::JweDecrypt(config) = &entry.plugin else {
            continue;
        };
        if config.resolved_credential.is_none() {
            continue;
        }
        match config.resolved_key_material() {
            Ok(_) => {}
            Err(ResolvedJweCredentialError::Missing) => {
                errors.push(ResolvedRefsError::MissingKeys { keys: vec!["secret"] });
            }
            Err(error) => errors.push(ResolvedRefsError::DecodeError {
                key: "secret",
                detail: error.status_detail(),
            }),
        }
    }
    errors
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/jwe_test.rs"]
mod tests;
