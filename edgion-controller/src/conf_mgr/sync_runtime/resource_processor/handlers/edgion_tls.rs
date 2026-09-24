//! EdgionTls Handler
//!
//! Handles EdgionTls resources with:
//! - Validation (warnings for missing secrets, parent_refs, etc.)
//! - Server Secret reference resolution (secret_ref -> spec.secret)
//! - CA Secret reference resolution for mTLS (client_auth.ca_secret_ref -> client_auth.ca_secret)
//! - SecretRefManager registration for cascading updates
//! - Gateway route index registration for port revalidation on Gateway changes
//! - Gateway API standard status management (per parent)

use super::{remove_from_gateway_route_index, update_gateway_route_index};
use crate::conf_mgr::sync_runtime::resource_processor::status_pipeline::filter_current_native_attachment_status;
use crate::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, set_parent_conditions_full, AcceptedError, HandlerContext, ProcessResult, ProcessorHandler,
    ResolvedRefsError, ResourceRef,
};
use edgion_resources::prelude_resources::EdgionTls;
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::edgion_tls::EdgionTlsStatus;
use edgion_resources::ResourceKind;

/// Returns `true` when `secret_ref` points at a different namespace than `owner_ns`,
/// ReferenceGrant validation is enabled, and no `ReferenceGrant` authorizes the
/// `EdgionTls` -> `Secret` reference. Mirrors the cross-namespace guard used by
/// `GatewayHandler` for `certificateRefs`; when validation is disabled this always
/// returns `false`, keeping behavior consistent with the other handlers.
fn cross_ns_secret_ref_denied(ctx: &HandlerContext, owner_ns: &str, secret_ref: &SecretObjectReference) -> bool {
    if !secret_ref.is_cross_namespace(owner_ns) || !crate::config::is_reference_grant_validation_enabled() {
        return false;
    }
    let target_ns = secret_ref.effective_namespace(owner_ns);
    !ctx.runtime().ref_grant_store.check_reference_allowed(
        owner_ns,
        "edgion.io",
        "EdgionTls",
        target_ns,
        "",
        "Secret",
        Some(&secret_ref.name),
    )
}

fn is_core_secret_ref(secret_ref: &SecretObjectReference) -> bool {
    secret_ref.group.as_deref().is_none_or(str::is_empty)
        && secret_ref.kind.as_deref().is_none_or(|kind| kind == "Secret")
}

fn invalid_secret_ref_warning(field: &str, secret_ref: &SecretObjectReference) -> Option<String> {
    (!is_core_secret_ref(secret_ref)).then(|| {
        format!(
            "{field} must reference the core Secret kind (group='', kind='Secret'); got group='{}', kind='{}'",
            secret_ref.group.as_deref().unwrap_or(""),
            secret_ref.kind.as_deref().unwrap_or("Secret")
        )
    })
}

/// EdgionTls handler
///
/// Features:
/// - validate: Check for parent_refs and secret existence
/// - parse: Parse secret_ref -> fill spec.secret
/// - parse: Parse client_auth.ca_secret_ref -> fill client_auth.ca_secret
/// - parse: Register Secret references to SecretRefManager
/// - on_change: Register in gateway_route_index so Gateway port/hostname changes trigger requeue
/// - on_delete: Clear SecretRefManager and gateway_route_index references
pub struct EdgionTlsHandler {
    controller_name: String,
}

impl EdgionTlsHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }
}

impl Default for EdgionTlsHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionTls> for EdgionTlsHandler {
    fn finalize_transport(&self, tls: &mut EdgionTls, _ctx: &HandlerContext) {
        filter_current_native_attachment_status(tls, &self.controller_name);
    }

    fn validate(&self, tls: &EdgionTls, ctx: &HandlerContext) -> Vec<String> {
        let mut warnings = Vec::new();
        let owner_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        if let Err(reason) =
            edgion_resources::resources::edgion_tls::validate_operator_attachment_dimensions(&tls.spec, owner_ns)
        {
            warnings.push(format!("Invalid EdgionTls attachment dimensions: {reason}"));
        }

        // Check parent_refs
        if tls.spec.parent_refs.is_none() || tls.spec.parent_refs.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
            warnings.push("EdgionTls has no parent_refs, it won't be applied to any Gateway".to_string());
        }

        // Check secret existence from global secret store. Cert-content checks
        // (expiry, SAN, key match) are deferred to update_status() via
        // validate_cert() and surfaced as ResolvedRefs/InvalidCertificate so
        // they appear on the per-parent status condition rather than as a generic
        // Accepted/Invalid warning.
        let secret_ns = tls
            .spec
            .secret_ref
            .namespace
            .as_ref()
            .or(tls.metadata.namespace.as_ref());

        if let Some(warning) = invalid_secret_ref_warning("spec.secretRef", &tls.spec.secret_ref) {
            warnings.push(warning);
        } else if !cross_ns_secret_ref_denied(ctx, owner_ns, &tls.spec.secret_ref)
            && ctx
                .runtime()
                .secret_store
                .get(secret_ns.map(|s| s.as_str()), &tls.spec.secret_ref.name)
                .is_none()
        {
            let secret_key = format_secret_key(secret_ns, &tls.spec.secret_ref.name);
            warnings.push(format!("Secret '{}' not found (may arrive later)", secret_key));
        }
        if let Some(ca_secret_ref) = tls
            .spec
            .client_auth
            .as_ref()
            .and_then(|auth| auth.ca_secret_ref.as_ref())
        {
            if let Some(warning) = invalid_secret_ref_warning("spec.clientAuth.caSecretRef", ca_secret_ref) {
                warnings.push(warning);
            }
        }

        warnings
    }

    async fn parse(&self, mut tls: EdgionTls, ctx: &HandlerContext) -> ProcessResult<EdgionTls> {
        // Controller-resolved transport fields are never authoritative input.
        // Clear them before any lookup, validation branch, or early return.
        tls.spec.secret = None;
        tls.spec.resolved_listeners = None;
        tls.spec.resolved_listener_attachments = None;
        tls.spec.resolved_listener_attachment_index = Default::default();
        if let Some(client_auth) = tls.spec.client_auth.as_mut() {
            client_auth.ca_secret = None;
        }

        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );

        let resource_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        let invalid_operator_dimensions =
            edgion_resources::resources::edgion_tls::validate_operator_attachment_dimensions(&tls.spec, resource_ns)
                .is_err();

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);
        // Clear old cross-namespace references too; they are re-registered below so a
        // later ReferenceGrant add/remove requeues this EdgionTls (mirrors GatewayHandler).
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);

        // Runtime defense for typed objects that bypass serde/CRD admission.
        // Revoke the previous parent set without traversing the oversized input,
        // then stop before any Secret or Gateway lookup.
        if invalid_operator_dimensions {
            let dropped_gateways = update_gateway_route_index(
                ResourceKind::EdgionTls,
                resource_ns,
                tls.metadata.name.as_deref().unwrap_or(""),
                None,
                ctx.runtime(),
            );
            for gateway_key in dropped_gateways {
                ctx.requeue(ResourceKind::Gateway.as_str(), gateway_key).await;
            }
            return ProcessResult::Continue(tls);
        }

        // Publish the complete parent dependency set before the first Gateway
        // lookup below. This is one atomic clear-and-replace operation in the
        // route index, so a concurrent Gateway publish/unpublish either sees
        // this owner and requeues it or the owner subsequently resolves the
        // already-published Gateway snapshot during this reconciliation.
        let resource_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        let dropped_gateways = update_gateway_route_index(
            ResourceKind::EdgionTls,
            resource_ns,
            tls.metadata.name.as_deref().unwrap_or(""),
            tls.spec.parent_refs.as_ref(),
            ctx.runtime(),
        );
        for gateway_key in dropped_gateways {
            ctx.requeue(ResourceKind::Gateway.as_str(), gateway_key).await;
        }

        // 1. Resolve server Secret from global secret store
        let secret_ns = tls
            .spec
            .secret_ref
            .namespace
            .as_ref()
            .or(tls.metadata.namespace.as_ref());
        let secret_key = format_secret_key(secret_ns, &tls.spec.secret_ref.name);

        // Enforce ReferenceGrant for cross-namespace Secret refs before reading the
        // Secret, mirroring GatewayHandler's certificateRefs handling (gateway.rs).
        // The controller ServiceAccount can read Secrets cluster-wide, so this is the
        // only tenant-isolation boundary preventing a low-privilege EdgionTls writer
        // from embedding another namespace's TLS private key into spec.secret.
        let owner_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        // Track the cross-namespace dependency so ReferenceGrant changes in the target
        // namespace requeue this EdgionTls (covers both grant-added and grant-removed).
        if is_core_secret_ref(&tls.spec.secret_ref) {
            // Register only a valid typed reference. Source events requeue the
            // owner, while owner reconciliation is the sole clear-before-add path.
            ctx.secret_ref_manager()
                .add_ref(secret_key.clone(), resource_ref.clone());
        }
        if is_core_secret_ref(&tls.spec.secret_ref) && tls.spec.secret_ref.is_cross_namespace(owner_ns) {
            ctx.runtime().cross_ns_ref_mgr.add_ref(
                tls.spec.secret_ref.effective_namespace(owner_ns).to_string(),
                resource_ref.clone(),
            );
        }
        if !is_core_secret_ref(&tls.spec.secret_ref) {
            tracing::info!(
                edgion_tls = %resource_ref.key(),
                "Invalid server Secret reference group/kind; leaving secret unresolved"
            );
        } else if cross_ns_secret_ref_denied(ctx, owner_ns, &tls.spec.secret_ref) {
            tracing::info!(
                edgion_tls = %resource_ref.key(),
                secret_key = %secret_key,
                "Cross-namespace Secret reference denied (no ReferenceGrant); leaving secret unresolved"
            );
        } else if let Some(secret) = ctx
            .runtime()
            .secret_store
            .get(secret_ns.map(|s| s.as_str()), &tls.spec.secret_ref.name)
        {
            tls.spec.secret = Some(secret);
            tracing::debug!(
                edgion_tls = %resource_ref.key(),
                secret_key = %secret_key,
                "Secret resolved and filled into EdgionTls"
            );
        } else {
            // Secret not found yet - this is normal if Secret arrives after EdgionTls
            // The SecretRefManager reference ensures we'll be requeued when Secret arrives
            tracing::info!(
                edgion_tls = %resource_ref.key(),
                secret_key = %secret_key,
                "Secret not found yet, will be reprocessed when Secret arrives"
            );
        }

        // 2. Resolve CA Secret (if mTLS is configured)
        if let Some(ref mut client_auth) = tls.spec.client_auth {
            if let Some(ref ca_secret_ref) = client_auth.ca_secret_ref {
                let ca_ns = ca_secret_ref.namespace.as_ref().or(tls.metadata.namespace.as_ref());
                let ca_secret_key = format_secret_key(ca_ns, &ca_secret_ref.name);

                // Same cross-namespace ReferenceGrant guard as the server cert above:
                // the mTLS CA Secret can also carry private key material.
                let ca_owner_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
                if is_core_secret_ref(ca_secret_ref) {
                    ctx.secret_ref_manager()
                        .add_ref(ca_secret_key.clone(), resource_ref.clone());
                }
                if is_core_secret_ref(ca_secret_ref) && ca_secret_ref.is_cross_namespace(ca_owner_ns) {
                    ctx.runtime().cross_ns_ref_mgr.add_ref(
                        ca_secret_ref.effective_namespace(ca_owner_ns).to_string(),
                        resource_ref.clone(),
                    );
                }
                if !is_core_secret_ref(ca_secret_ref) {
                    tracing::info!(
                        edgion_tls = %resource_ref.key(),
                        "Invalid CA Secret reference group/kind; leaving ca_secret unresolved"
                    );
                } else if cross_ns_secret_ref_denied(ctx, ca_owner_ns, ca_secret_ref) {
                    tracing::info!(
                        edgion_tls = %resource_ref.key(),
                        ca_secret_key = %ca_secret_key,
                        "Cross-namespace CA Secret reference denied (no ReferenceGrant); leaving ca_secret unresolved"
                    );
                } else if let Some(ca_secret) = ctx
                    .runtime()
                    .secret_store
                    .get(ca_ns.map(|s| s.as_str()), &ca_secret_ref.name)
                {
                    client_auth.ca_secret = Some(ca_secret);
                    tracing::debug!(
                        edgion_tls = %resource_ref.key(),
                        ca_secret_key = %ca_secret_key,
                        "CA Secret resolved and filled into EdgionTls.client_auth"
                    );
                } else {
                    tracing::info!(
                        edgion_tls = %resource_ref.key(),
                        ca_secret_key = %ca_secret_key,
                        "CA Secret not found yet, mTLS will be enabled when Secret arrives"
                    );
                }
            }
        }

        // 3. Resolve listener keys from parentRefs for per-listener route isolation.
        // Defensive filter: EdgionTls only makes sense on HTTPS/TLS listeners.
        // A Gateway is not expected to mix TLS-capable and non-TLS listeners, but
        // we keep this local check so the resolved spec stays clean even if upstream
        // Gateway validation ever lets a mixed configuration through.
        if let Some(parent_refs) = &tls.spec.parent_refs {
            let attachments: Vec<_> = super::route_utils::resolve_listener_attachments(
                parent_refs,
                resource_ns,
                tls.metadata.namespace.as_deref(),
                ctx.runtime(),
            )
            .into_iter()
            .filter(|attachment| matches!(attachment.listener_key.protocol(), "HTTPS" | "TLS"))
            .collect();
            if attachments.len() > edgion_resources::resources::edgion_tls::MAX_EDGION_TLS_RESOLVED_ATTACHMENTS {
                tracing::warn!(
                    edgion_tls = %resource_ref.key(),
                    attachment_count = attachments.len(),
                    max = edgion_resources::resources::edgion_tls::MAX_EDGION_TLS_RESOLVED_ATTACHMENTS,
                    "Resolved listener attachment limit exceeded; leaving transport unbound"
                );
                return ProcessResult::Continue(tls);
            }
            let mut listener_keys: Vec<_> = attachments
                .iter()
                .map(|attachment| attachment.listener_key.clone())
                .collect();
            listener_keys.sort_unstable();
            listener_keys.dedup();
            if !listener_keys.is_empty() {
                tracing::debug!(
                    edgion_tls = %resource_ref.key(),
                    resolved_listeners = ?listener_keys,
                    "Resolved listeners from parentRefs"
                );
                tls.spec.resolved_listeners = Some(listener_keys);
                tls.spec.resolved_listener_attachments = Some(attachments);
            }
        }

        ProcessResult::Continue(tls)
    }

    async fn on_change(&self, tls: &EdgionTls, ctx: &HandlerContext) {
        // Parent dependencies are intentionally installed at the start of
        // parse(), before any Gateway snapshot lookup. on_change runs too late
        // for that lookup/requeue barrier and therefore has no index work.
        let _ = (tls, ctx);
    }

    async fn on_delete(&self, tls: &EdgionTls, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);

        let tls_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        let tls_name = tls.metadata.name.as_deref().unwrap_or("");
        remove_from_gateway_route_index(ResourceKind::EdgionTls, tls_ns, tls_name, ctx.runtime());

        tracing::debug!(
            edgion_tls = %resource_ref.key(),
            "Cleared secret and gateway_route_index references on EdgionTls delete"
        );
    }

    fn update_status(&self, tls: &mut EdgionTls, ctx: &HandlerContext, validation_errors: &[String]) {
        let resource_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        if edgion_resources::resources::edgion_tls::validate_operator_attachment_dimensions(&tls.spec, resource_ns)
            .is_err()
        {
            // The raw parent vector is untrusted in FS/etcd and forged sync
            // input. Publish one fixed, bounded status shape without walking,
            // looking up, or cloning any parent.
            tls.status = Some(EdgionTlsStatus { parents: Vec::new() });
            return;
        }
        let generation = tls.metadata.generation;

        let validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);

        // parse() already resolved secret_ref -> spec.secret; reuse that result
        // instead of querying secret_store a second time.
        let mut resolved_refs_errors: Vec<ResolvedRefsError> = if !is_core_secret_ref(&tls.spec.secret_ref) {
            vec![ResolvedRefsError::InvalidKind {
                kind: format!(
                    "{}/{}",
                    tls.spec.secret_ref.group.as_deref().unwrap_or(""),
                    tls.spec.secret_ref.kind.as_deref().unwrap_or("Secret")
                ),
                name: tls.spec.secret_ref.name.clone(),
            }]
        } else if tls.spec.secret.is_some() {
            // Secret resolved: run the SAME validation the gateway runs and surface
            // cert-CONTENT failures on ResolvedRefs. Filter out the not-found variants
            // (secret/cert/key absent) — those are owned by the secret-absent branch
            // below; folding them here would double-report. status_message() is
            // deterministic (no wall-clock) so the message is byte-stable across
            // reconciles (avoids the status-write hot loop).
            use edgion_common::tls_validation::CertValidationError as CVE;
            edgion_common::tls_validation::validate_cert(tls)
                .errors
                .into_iter()
                .filter(|e| !matches!(e, CVE::SecretNotFound | CVE::CertNotFound | CVE::KeyNotFound))
                .map(|e| ResolvedRefsError::CertContentInvalid {
                    detail: e.status_message(),
                })
                .collect()
        } else {
            let secret_ns = tls
                .spec
                .secret_ref
                .namespace
                .as_ref()
                .or(tls.metadata.namespace.as_ref());
            let target_ns = secret_ns.map(|s| s.as_str()).unwrap_or("default").to_string();
            // Distinguish "denied by ReferenceGrant" from "Secret simply absent" so
            // the K8s ResolvedRefs condition reports RefNotPermitted instead of a
            // perpetual SecretNotFound wait when the cross-ns ref was rejected in parse().
            if cross_ns_secret_ref_denied(ctx, resource_ns, &tls.spec.secret_ref) {
                vec![ResolvedRefsError::RefNotPermitted {
                    from_namespace: resource_ns.to_string(),
                    from_kind: "EdgionTls",
                    target_namespace: target_ns,
                    target_name: tls.spec.secret_ref.name.clone(),
                }]
            } else {
                vec![ResolvedRefsError::SecretNotFound {
                    namespace: target_ns,
                    name: tls.spec.secret_ref.name.clone(),
                }]
            }
        };

        if let Some(ca_secret_ref) = tls
            .spec
            .client_auth
            .as_ref()
            .and_then(|auth| auth.ca_secret_ref.as_ref())
        {
            if !is_core_secret_ref(ca_secret_ref) {
                resolved_refs_errors.push(ResolvedRefsError::InvalidKind {
                    kind: format!(
                        "{}/{}",
                        ca_secret_ref.group.as_deref().unwrap_or(""),
                        ca_secret_ref.kind.as_deref().unwrap_or("Secret")
                    ),
                    name: ca_secret_ref.name.clone(),
                });
            }
        }

        let status = tls.status.get_or_insert_with(|| EdgionTlsStatus { parents: vec![] });

        if let Some(parent_refs) = &tls.spec.parent_refs {
            for parent_ref in parent_refs {
                let accepted_result = super::route_utils::validate_parent_ref_accepted(
                    resource_ns,
                    parent_ref,
                    None,
                    "EdgionTls",
                    ctx.runtime(),
                );
                let mut accepted_errors = accepted_result.errors;

                // EdgionTls only makes sense on HTTPS/TLS listeners — reject non-TLS ones.
                if accepted_errors.is_empty()
                    && !accepted_result.matched_listener_protocols.is_empty()
                    && !accepted_result
                        .matched_listener_protocols
                        .iter()
                        .any(|p| p == "HTTPS" || p == "TLS")
                {
                    accepted_errors.push(AcceptedError::Invalid {
                        message: "EdgionTls requires HTTPS or TLS listener".to_string(),
                    });
                }

                accepted_errors.extend(validation_accepted_errors.clone());

                let ps = super::route_utils::parent_status_for_update(
                    &mut status.parents,
                    parent_ref,
                    resource_ns,
                    &self.controller_name,
                );
                set_parent_conditions_full(&mut ps.conditions, &accepted_errors, &resolved_refs_errors, generation);
            }

            super::route_utils::retain_current_parent_statuses(
                &mut status.parents,
                parent_refs,
                resource_ns,
                &self.controller_name,
            );
        } else {
            super::route_utils::retain_current_parent_statuses(
                &mut status.parents,
                &[],
                resource_ns,
                &self.controller_name,
            );
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_tls_test.rs"]
mod tests;
