//! OpenID Connect resolution from actual owning fields.

use super::read_secret_utf8;
use crate::conf_mgr::sync_runtime::resource_processor::credential::ca_refs::resolve_bound_tls;
use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_bound_secret, BoundFetchOutcome};
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
    let EdgionPlugin::OpenidConnect(config) = entry.config_mut() else {
        return;
    };
    config.resolved_oidc_client_secret = None;
    config.resolved_session_secret = None;
    config.resolved_ca_certificates = None;
    config.resolved_client_certificate = None;
    let (client, session, tls) = {
        let Ok(references) = entry.references() else { return };
        let read = |token, keys: &[&str]| match fetch_bound_secret(ctx, owner, token) {
            BoundFetchOutcome::Ok(authorized) => read_secret_utf8(&authorized.secret, keys).map(str::to_owned),
            BoundFetchOutcome::Pending(_) | BoundFetchOutcome::Invalid(_) => None,
        };
        (
            references
                .oidc_client_secret()
                .and_then(|token| read(token, &["clientSecret", "client_secret", "secret"])),
            references
                .oidc_session_secret()
                .and_then(|token| read(token, &["sessionSecret", "session_secret", "secret"])),
            references.tls().map(|tls| resolve_bound_tls(ctx, owner, tls)),
        )
    };
    if let EdgionPlugin::OpenidConnect(config) = entry.config_mut() {
        config.resolved_oidc_client_secret = client;
        config.resolved_session_secret = session;
        if let Some(material) = tls {
            config.resolved_ca_certificates = material.ca_certificates;
            config.resolved_client_certificate = material.client_certificate;
        }
    }
}
