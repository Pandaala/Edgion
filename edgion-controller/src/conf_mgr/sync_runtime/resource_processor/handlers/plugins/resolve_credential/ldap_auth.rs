//! LdapAuth TLS and service-account password resolution from actual owning fields.

use super::read_secret_utf8;

use crate::conf_mgr::sync_runtime::resource_processor::credential::ca_refs::resolve_bound_tls;
use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_bound_secret, BoundFetchOutcome};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::LdapAuthConfig;
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
    // Clear every resolved field before any early return so forged or stale
    // material never survives a resolution pass.
    config.resolved_ca_certificates = None;
    config.resolved_client_certificate = None;
    config.resolved_bind_password = None;
    let (password, material) = {
        let Ok(references) = entry.references() else { return };
        let password = references
            .ldap_bind_secret()
            .and_then(|token| match fetch_bound_secret(ctx, owner, token) {
                BoundFetchOutcome::Ok(authorized) => {
                    read_secret_utf8(&authorized.secret, LdapAuthConfig::BIND_PASSWORD_KEYS).map(str::to_owned)
                }
                BoundFetchOutcome::Pending(_) | BoundFetchOutcome::Invalid(_) => None,
            });
        (password, references.tls().map(|tls| resolve_bound_tls(ctx, owner, tls)))
    };
    if let EdgionPlugin::LdapAuth(config) = entry.config_mut() {
        config.resolved_bind_password = password;
        if let Some(material) = material {
            config.resolved_ca_certificates = material.ca_certificates;
            config.resolved_client_certificate = material.client_certificate;
        }
    }
}
