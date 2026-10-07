//! LdapAuth TLS resolution from actual owning fields.

use crate::conf_mgr::sync_runtime::resource_processor::credential::ca_refs::resolve_bound_tls;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::refs::plugin_fields::{BoundPluginEntryMut, PreparedPluginReferences};

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
    let EdgionPlugin::LdapAuth(config) = entry.config_mut() else {
        return;
    };
    config.resolved_ca_certificates = None;
    config.resolved_client_certificate = None;
    let material = {
        let Ok(references) = entry.references() else { return };
        let Some(tls) = references.tls() else { return };
        resolve_bound_tls(ctx, owner, tls)
    };
    if let EdgionPlugin::LdapAuth(config) = entry.config_mut() {
        config.resolved_ca_certificates = material.ca_certificates;
        config.resolved_client_certificate = material.client_certificate;
    }
}
