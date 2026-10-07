//! Owning request-template and local TLS resolution for managed HTTP plugins.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    ca_refs::resolve_bound_tls, resolve_request_secrets,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::resources::common::{ResolvedSecrets, TlsConfig};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::resources::refs::plugin_fields::{BoundPluginEntryMut, PreparedPluginReferences};
use edgion_resources::EdgionPlugins;

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

fn fields(plugin: &mut EdgionPlugin) -> Option<(&mut Option<ResolvedSecrets>, Option<&mut TlsConfig>)> {
    match plugin {
        EdgionPlugin::ForwardAuth(config) => Some((&mut config.resolved_secrets, config.conn.tls.as_mut())),
        EdgionPlugin::Guardrail(config) => Some((&mut config.resolved_secrets, config.conn.tls.as_mut())),
        EdgionPlugin::AiGuard(config) => config
            .evaluator
            .as_mut()
            .map(|config| (&mut config.resolved_secrets, config.conn.tls.as_mut())),
        _ => None,
    }
}

fn resolve_entry(mut entry: BoundPluginEntryMut<'_>, owner: &ResourceRef, ctx: &HandlerContext) {
    let Some((secrets, tls)) = fields(entry.config_mut()) else {
        return;
    };
    *secrets = None;
    if let Some(tls) = tls {
        tls.resolved_ca_certificates = None;
        tls.resolved_client_certificate = None;
    }
    let (resolved, tls) = {
        let Ok(references) = entry.references() else { return };
        let (resolved, _) = resolve_request_secrets(ctx, owner, references.request_secrets());
        let tls = references.tls().map(|tls| resolve_bound_tls(ctx, owner, tls));
        (resolved, tls)
    };
    if let Some((secrets, field)) = fields(entry.config_mut()) {
        *secrets = (!resolved.is_empty()).then_some(ResolvedSecrets(resolved));
        if let Some((field, material)) = field.zip(tls) {
            field.resolved_ca_certificates = material.ca_certificates;
            field.resolved_client_certificate = material.client_certificate;
        }
    }
}
