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

use super::super::credential::pipeline::{authorize_bound_edgion_tls, fetch_bound_secret_readonly};
use super::super::credential::{fetch_bound_secret, BoundFetchOutcome};
use edgion_resources::resources::refs::edgion_tls_fields::{EdgionTlsField, EdgionTlsRefs};
use edgion_resources::resources::refs::ReferenceError;

fn field_error(field: &EdgionTlsField<'_>, wire: &SecretObjectReference) -> Option<ResolvedRefsError> {
    match field.token().validated() {
        Ok(_) => None,
        Err(ReferenceError::UnsupportedTarget) => Some(ResolvedRefsError::InvalidKind {
            kind: format!(
                "{}/{}",
                wire.group.as_deref().unwrap_or(""),
                wire.kind.as_deref().unwrap_or("Secret")
            ),
            name: wire.name.clone(),
        }),
        Err(error) => Some(ResolvedRefsError::InvalidReference { error }),
    }
}
fn field_status(
    ctx: &HandlerContext,
    owner: &ResourceRef,
    field: &EdgionTlsField<'_>,
    wire: &SecretObjectReference,
    present: bool,
) -> Option<ResolvedRefsError> {
    if let Some(error) = field_error(field, wire) {
        return Some(error);
    }
    match authorize_bound_edgion_tls(ctx, owner, field.token()) {
        BoundFetchOutcome::Invalid(error) => return Some(ResolvedRefsError::InvalidReference { error }),
        BoundFetchOutcome::Pending(super::super::credential::PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => {
            return Some(ResolvedRefsError::RefNotPermitted {
                from_namespace: from_ns,
                from_kind: "EdgionTls",
                target_namespace: to_ns,
                target_name: secret_name,
            })
        }
        BoundFetchOutcome::Pending(_) => {}
        BoundFetchOutcome::Ok(()) => {}
    }
    if !present {
        let token = field.token();
        let target = token.validated().ok()?.target();
        return Some(ResolvedRefsError::SecretNotFound {
            namespace: target.scope().namespace().unwrap_or("default").into(),
            name: target.name().into(),
        });
    }
    field.material().err().map(|_| ResolvedRefsError::CertContentInvalid {
        detail: "Secret material does not match its declared reference".into(),
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
        let owner = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );
        let refs = EdgionTlsRefs::from_tls(tls);
        let server = refs.server();
        if let Some(error) = field_error(&server, &tls.spec.secret_ref) {
            warnings.push(format!(
                "spec.secretRef must reference the core Secret kind with a valid source/name/namespace: {error:?}"
            ));
        } else if matches!(
            fetch_bound_secret_readonly(ctx, &owner, server.token()),
            BoundFetchOutcome::Pending(super::super::credential::PendingReason::SecretNotFound { .. })
        ) {
            let ns = tls
                .spec
                .secret_ref
                .namespace
                .as_ref()
                .or(tls.metadata.namespace.as_ref());
            warnings.push(format!(
                "Secret '{}' not found (may arrive later)",
                format_secret_key(ns, &tls.spec.secret_ref.name)
            ));
        }
        if let (Some(field), Some(wire)) = (
            refs.client_ca(),
            tls.spec.client_auth.as_ref().and_then(|a| a.ca_secret_ref.as_ref()),
        ) {
            if let Some(error) = field_error(&field, wire) {
                warnings.push(format!("spec.clientAuth.caSecretRef must reference the core Secret kind with a valid source/name/namespace: {error:?}"));
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

        // Actual field validation and recovery registration precede authorization and reads.
        let (server, ca) = {
            let refs = EdgionTlsRefs::from_tls(&tls);
            let server = match fetch_bound_secret(ctx, &resource_ref, refs.server().token()) {
                BoundFetchOutcome::Ok(value) => Some(super::super::credential::ca_refs::ordinary_secret_material(
                    value.secret,
                )),
                _ => None,
            };
            let ca = refs
                .client_ca()
                .and_then(|field| match fetch_bound_secret(ctx, &resource_ref, field.token()) {
                    BoundFetchOutcome::Ok(value) => Some(super::super::credential::ca_refs::ordinary_secret_material(
                        value.secret,
                    )),
                    _ => None,
                });
            (server, ca)
        };
        tls.spec.secret = server;
        if let Some(auth) = tls.spec.client_auth.as_mut() {
            auth.ca_secret = ca;
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

        let owner = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );
        let refs = EdgionTlsRefs::from_tls(tls);
        let mut resolved_refs_errors: Vec<ResolvedRefsError> = field_status(
            ctx,
            &owner,
            &refs.server(),
            &tls.spec.secret_ref,
            tls.spec.secret.is_some(),
        )
        .into_iter()
        .collect();
        if resolved_refs_errors.is_empty() && tls.spec.secret.is_some() {
            use edgion_common::tls_validation::CertValidationError as CVE;
            resolved_refs_errors.extend(
                edgion_common::tls_validation::validate_cert(tls)
                    .errors
                    .into_iter()
                    .filter(|e| !matches!(e, CVE::SecretNotFound | CVE::CertNotFound | CVE::KeyNotFound))
                    .map(|e| ResolvedRefsError::CertContentInvalid {
                        detail: e.status_message(),
                    }),
            );
        }
        if let (Some(field), Some(auth)) = (refs.client_ca(), tls.spec.client_auth.as_ref()) {
            if let Some(error) = field_status(
                ctx,
                &owner,
                &field,
                auth.ca_secret_ref.as_ref().unwrap(),
                auth.ca_secret.is_some(),
            ) {
                resolved_refs_errors.push(error);
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
