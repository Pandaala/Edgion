//! CSRF signing-key resolution from the actual owning field.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_bound_secret, BoundFetchOutcome};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::refs::plugin_fields::{BoundPluginEntryMut, PreparedPluginReferences};

use super::read_secret_utf8;

/// Rebuild material using one prepared owner snapshot shared by the resolvers.
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

fn resolve_entry(mut entry: BoundPluginEntryMut<'_>, owner: &ResourceRef, ctx: &HandlerContext) {
    let EdgionPlugin::Csrf(config) = entry.config_mut() else {
        return;
    };
    config.resolved_key = None;
    if !entry.is_enabled() {
        return;
    }
    let resolved_key = {
        let Ok(references) = entry.references() else {
            return;
        };
        let Some(token) = references.csrf_secret() else {
            return;
        };
        match fetch_bound_secret(ctx, owner, token) {
            BoundFetchOutcome::Ok(authorized) => read_secret_utf8(&authorized.secret, &["key"]).map(str::to_owned),
            BoundFetchOutcome::Pending(_) | BoundFetchOutcome::Invalid(_) => None,
        }
    };
    if let EdgionPlugin::Csrf(config) = entry.config_mut() {
        config.resolved_key = resolved_key;
    }
}
