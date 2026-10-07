//! HeaderCertAuth CA resolution from actual active owning fields.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_bound_secret, BoundFetchOutcome};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::resources::edgion_plugins::{CertSourceMode, EdgionPlugin};
use edgion_resources::resources::refs::plugin_fields::{BoundPluginEntryMut, PreparedPluginReferences};
use edgion_resources::EdgionPlugins;

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
    let EdgionPlugin::HeaderCertAuth(config) = entry.config_mut() else {
        return;
    };
    config.resolved_ca_secrets = None;
    if config.mode != CertSourceMode::Header || !entry.is_enabled() {
        return;
    }
    let mut resolved = Vec::new();
    {
        let Ok(references) = entry.references() else {
            return;
        };
        for token in references.header_ca_secrets() {
            let BoundFetchOutcome::Ok(authorized) = fetch_bound_secret(ctx, owner, token) else {
                continue;
            };
            if authorized
                .secret
                .data
                .as_ref()
                .is_some_and(|data| data.contains_key("ca.crt"))
            {
                resolved.push(authorized.secret);
            }
        }
    }
    if let EdgionPlugin::HeaderCertAuth(config) = entry.config_mut() {
        config.resolved_ca_secrets = (!resolved.is_empty()).then_some(resolved);
    }
}
