//! ForwardAuth credential resolution.
//!
//! Resolves `${secretRef:ns/name:key}` template references embedded in the
//! outbound request spec and populates `ForwardAuthConfig::resolved_secrets`
//! for delivery to the gateway over conf_sync.
//!
//! `SecretNotFound` errors are surfaced automatically through the
//! `SecretRefManager` path: `fetch_secret` (called by `resolve_request_secrets`)
//! registers each referenced Secret regardless of resolution outcome, so
//! `EdgionPluginsHandler::resolve_secret_refs` in `update_status` finds and
//! reports missing secrets without additional wiring here.
//!
//! `DecodeError` and `RefNotPermitted` from `resolve_request_secrets` are NOT
//! yet threaded into the status condition. They require a stash channel between
//! `parse()` and `update_status()` (option-a transient field or HandlerContext
//! side-channel). See TODO(task-10) below.

use crate::conf_mgr::sync_runtime::resource_processor::credential::resolve_request_secrets;
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::common::ResolvedSecrets;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use edgion_resources::ResourceKind;

/// Resolve every `${secretRef:ns/name:key}` in each `ForwardAuth` plugin's
/// outbound request spec. Populates `cfg.resolved_secrets` with successfully
/// resolved values; clears it when no templates reference Secrets or none
/// resolve successfully.
///
/// Registers all referenced Secrets with `SecretRefManager` (via
/// `fetch_secret` inside `resolve_request_secrets`) so that
/// `EdgionPluginsHandler::resolve_secret_refs` in `update_status` can
/// surface `SecretNotFound` when a referenced Secret is absent.
///
/// # Status wiring gap (TODO task-10)
///
/// `resolve_request_secrets` may also return `DecodeError` (key absent in a
/// present Secret) or `RefNotPermitted` (cross-namespace Secret without a
/// `ReferenceGrant`). These error variants are discarded here because the
/// `SecretRefManager` path covers only `SecretNotFound`. Wiring the additional
/// variants into the `ResolvedRefs` status condition requires a stash channel
/// between `parse()` and `update_status()` and is left as a follow-on.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) async fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    let Some(ref mut plugins) = ep.spec.request_plugins else {
        return;
    };
    for entry in plugins.iter_mut() {
        if let EdgionPlugin::ForwardAuth(ref mut cfg) = entry.plugin {
            cfg.resolved_secrets = None;
            let names = cfg.request.collect_secret_ref_names();
            if names.is_empty() {
                continue;
            }
            let (resolved, _errs) =
                resolve_request_secrets(ctx, resource_ref, ResourceKind::EdgionPlugins, &names).await;
            // TODO(task-10): thread _errs (DecodeError, RefNotPermitted) into
            // the EdgionPlugins ResolvedRefs status condition via a stash field.
            cfg.resolved_secrets = if resolved.is_empty() {
                None
            } else {
                Some(ResolvedSecrets(resolved))
            };
        }
    }
}
