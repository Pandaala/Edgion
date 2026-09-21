//! EdgionAcme Handler (Controller side)
//!
//! Handles EdgionAcme resources with:
//! - Validation (email, domains, challenge config, etc.)
//! - ACME account Secret existence check → `ResolvedRefs` condition
//!   + SecretRefManager registration
//! - Status management
//! - Notifying AcmeService for background certificate issuance/renewal
//!
//! Resolved Secret bytes are NOT attached to the in-memory `EdgionAcme` —
//! `AcmeService` re-fetches via the Kubernetes API. The handler's job here is
//! purely validation/observability + cache wiring.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_secret, InvalidCredentialError, PendingReason, ResolveOutcome,
};

use super::route_utils::{resolve_acme_listener_attachments, AcmeGatewayRefError};
use super::{remove_from_gateway_route_index, update_gateway_route_index};
use crate::conf_mgr::sync_runtime::resource_processor::status_ownership::acme_is_owned;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, condition_unknown, update_condition,
};
use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ProcessResult, ProcessorHandler, ResourceRef};
use crate::services::acme::notify_resource_changed;
use edgion_resources::credentials::resolvers::AcmeAccountResolver;
use edgion_resources::duration::parse_duration;
use edgion_resources::prelude_resources::EdgionAcme;
use edgion_resources::resources::common::Condition;
use edgion_resources::resources::edgion_acme::{AcmeChallenge, EdgionAcmeStatus};
use edgion_resources::ResourceKind;

/// EdgionAcme handler
///
/// Features:
/// - validate: Check email, domains, challenge config consistency
/// - parse: Resolve the ACME account credential Secret reference
/// - on_delete: Clear SecretRefManager references
/// - update_status: Update ACME lifecycle status
pub struct EdgionAcmeHandler;

impl EdgionAcmeHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionAcmeHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// ACME-specific condition reasons.
///
/// Only `NeedsRegistration` is a genuine ACME extension — it models the
/// "Secret absent, will be created on first ACME registration" state which
/// has no equivalent in the generic credential pipeline. All other Secret
/// resolution failures route through `condition_reasons` so `EdgionAcme`,
/// `LinkSys`, `EdgionPlugins`, and `EdgionTls` produce the same reason
/// strings for the same underlying errors.
mod acme_reasons {
    /// ACME-specific · Secret absent or missing `accountCredentials` key —
    /// service will create/patch on first registration. Maps to
    /// `ResolvedRefs=Unknown`, not `False`, because this is the expected
    /// bootstrap state rather than a failure.
    pub const NEEDS_REGISTRATION: &str = "NeedsRegistration";
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionAcme> for EdgionAcmeHandler {
    fn filter(&self, acme: &EdgionAcme, ctx: &HandlerContext) -> bool {
        acme_is_owned(acme, ctx.runtime())
    }

    fn validate(&self, acme: &EdgionAcme, _ctx: &HandlerContext) -> Vec<String> {
        let mut errors = Vec::new();

        // Validate email
        if acme.spec.email.is_empty() {
            errors.push("EdgionAcme: email is required".to_string());
        }

        // Validate domains
        if acme.spec.domains.is_empty() {
            errors.push("EdgionAcme: at least one domain is required".to_string());
        }

        // Built-in HTTP-01 cannot issue wildcard certificates
        if acme.has_wildcard_domains() {
            errors.push(
                "EdgionAcme: built-in HTTP-01 does not support wildcard domains; use an externally issued TLS Secret"
                    .to_string(),
            );
        }

        // Validate GEP-2257 duration fields. The CRD pattern guards the K8s
        // path; this check covers FS mode and surfaces the error on the
        // Accepted condition instead of a silent runtime fallback.
        let renewal = &acme.spec.renewal;
        let duration_fields = [
            ("renewal.renewBefore", &renewal.renew_before),
            ("renewal.checkInterval", &renewal.check_interval),
            ("renewal.failBackoff", &renewal.fail_backoff),
        ];

        for (field, value) in duration_fields {
            if parse_duration(value).is_err() {
                errors.push(format!(
                    "EdgionAcme: {field} must be a valid GEP-2257 duration (e.g. \"30s\", \"24h\"), got '{value}'"
                ));
            }
        }

        errors
    }

    async fn parse(&self, mut acme: EdgionAcme, ctx: &HandlerContext) -> ProcessResult<EdgionAcme> {
        acme.spec.notify_after_publish = false;
        // Controller-derived serving authority is never trusted from input or
        // carried across reconciles.
        acme.spec.resolved_listener_attachments = None;
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionAcme,
            acme.metadata.namespace.clone(),
            acme.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);

        let resource_namespace = acme.metadata.namespace.as_deref().unwrap_or("default");
        let resource_name = acme.metadata.name.as_deref().unwrap_or_default();
        let gateway_ref = match &acme.spec.challenge {
            AcmeChallenge::Http01 { gateway_ref } => gateway_ref.clone(),
        };
        let parent_refs = vec![gateway_ref.clone()];
        let dropped_gateways = update_gateway_route_index(
            ResourceKind::EdgionAcme,
            resource_namespace,
            resource_name,
            Some(&parent_refs),
            ctx.runtime(),
        );
        for gateway_key in dropped_gateways {
            ctx.requeue(ResourceKind::Gateway.as_str(), gateway_key).await;
        }

        let gateway_group = gateway_ref.group.as_deref().unwrap_or("gateway.networking.k8s.io");
        let gateway_kind = gateway_ref.kind.as_deref().unwrap_or("Gateway");
        let gateway_namespace = gateway_ref.namespace.as_deref().unwrap_or(resource_namespace);
        if gateway_group == "gateway.networking.k8s.io"
            && gateway_kind == "Gateway"
            && gateway_namespace != resource_namespace
        {
            ctx.runtime()
                .cross_ns_ref_mgr
                .add_ref(gateway_namespace.to_string(), resource_ref.clone());
        }

        let gateway_outcome = resolve_acme_listener_attachments(&gateway_ref, resource_namespace, ctx.runtime());
        let gateway_resolved = gateway_outcome.is_ok();
        if let Ok(attachments) = &gateway_outcome {
            acme.spec.resolved_listener_attachments = Some(attachments.clone());
        }

        // Deliberately read the previous processed generation, not this input's
        // status: a concurrent service write may have rejected the previous
        // conditions PATCH. The serial worker publishes this cache before it
        // can process another status echo, even when status persistence fails.
        let previous = ctx
            .runtime()
            .processor_registry
            .get(ResourceKind::EdgionAcme.as_str())
            .and_then(|processor| processor.get_cached_any(&resource_ref.resource_key()))
            .and_then(|value| value.downcast::<EdgionAcme>().ok());

        // ── Resolve ACME account credential via pipeline ────────────────────
        let outcome = resolve_secret(
            ctx,
            &resource_ref,
            "edgion.io",
            "EdgionAcme",
            &acme.spec.private_key_secret_ref,
            &AcmeAccountResolver,
        )
        .await;

        let generation = acme.metadata.generation;
        let status = acme.status.get_or_insert_with(EdgionAcmeStatus::default);

        // Map outcome → ResolvedRefs Action
        let resource_key = resource_ref.key();
        let account_action = outcome_to_resolved_refs_action(&outcome, &resource_key, generation);
        // `account_corrupted` means the Secret exists but carries unusable
        // `accountCredentials` (decode failure / unsupported Secret.type).
        // We still return Continue so `update_status` persists the condition,
        // but we must NOT notify AcmeService — it must not attempt registration
        // against a broken account.
        let (account_condition, account_corrupted) = match account_action {
            AcmeResolveAction::Continue(cond) => (cond, false),
            AcmeResolveAction::Abort(cond) => (cond, true),
        };
        let resolved_refs_condition = match &gateway_outcome {
            Ok(_) => account_condition,
            Err(error) => gateway_ref_error_condition(error, generation),
        };
        update_condition(&mut status.conditions, resolved_refs_condition);

        // `outcome` was already mapped to a condition above; the resolved
        // Secret bytes are intentionally NOT attached to the in-memory
        // resource — AcmeService re-fetches via the Kubernetes API.

        if account_corrupted || !gateway_resolved {
            // Return Continue so `update_status` persists `ResolvedRefs=False`
            // and Gateway/operators can see the failure. We intentionally skip
            // `notify_resource_changed` — without
            // a usable account there is nothing productive for AcmeService to do.
            //
            // Consequence for background state: if a *prior* reconcile already
            // notified AcmeService and it cached an account / issuance state,
            // flipping to corrupted here does NOT tear that cache down. That is
            // deliberate — one bad Secret edit should not destroy a working
            // account. Operators must manually restart the service (or rotate
            // back to a valid Secret) to pick up repaired account data.
            return ProcessResult::Continue(acme);
        }

        // Notification must follow publication: the service's eligibility gate
        // must see the recovered/new conditions, not the previous cached denial.
        // Startup scanning and explicit Admin retry retain their own notifications.
        acme.spec.notify_after_publish = previous.as_ref().is_none_or(|previous| {
            previous.metadata.uid != acme.metadata.uid
                || previous.metadata.generation != acme.metadata.generation
                || resolved_refs_identity(previous.spec.current_status.as_ref())
                    != resolved_refs_identity(acme.status.as_ref())
        });

        ProcessResult::Continue(acme)
    }

    async fn after_publish(&self, acme: &EdgionAcme, _ctx: &HandlerContext) {
        if acme.spec.notify_after_publish {
            let ns = acme.metadata.namespace.as_deref().unwrap_or("default");
            let name = acme.metadata.name.as_deref().unwrap_or_default();
            notify_resource_changed(format!("{}/{}", ns, name)).await;
        }
    }

    async fn on_delete(&self, acme: &EdgionAcme, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionAcme,
            acme.metadata.namespace.clone(),
            acme.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);
        let resource_namespace = acme.metadata.namespace.as_deref().unwrap_or("default");
        let resource_name = acme.metadata.name.as_deref().unwrap_or_default();
        remove_from_gateway_route_index(
            ResourceKind::EdgionAcme,
            resource_namespace,
            resource_name,
            ctx.runtime(),
        );
        tracing::debug!(
            edgion_acme = %resource_ref.key(),
            "Cleared EdgionAcme dependency references on delete"
        );
    }

    fn update_status(&self, acme: &mut EdgionAcme, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = acme.metadata.generation;
        // `get_or_insert_with` pulls the current status (including `phase` / timestamps that
        // AcmeService owns via SSA) from the in-memory object, so the Controller's subsequent
        // Merge Patch preserves those fields. The two writers share the status field but only
        // intersect on conditions here — AcmeService's SSA patch omits `conditions` when empty
        // (`skip_serializing_if = "Vec::is_empty"`), so it never clears what we write.
        let status = acme.status.get_or_insert_with(EdgionAcmeStatus::default);

        // Accepted condition: reflects spec validation. phase / issuance status is
        // maintained separately by the background AcmeService (see parse/notify_resource_changed).
        let accepted = if validation_errors.is_empty() {
            condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Resource accepted",
                generation,
            )
        } else {
            condition_false(
                condition_types::ACCEPTED,
                condition_reasons::INVALID,
                validation_errors.join("; "),
                generation,
            )
        };
        update_condition(&mut status.conditions, accepted);

        // ResolvedRefs is set in parse(); update_status only manages Accepted.

        // Retain `last_failure_reason` text field for backward-compatibility with
        // existing tooling that reads it directly (e.g., dashboards). The canonical
        // status is now the Accepted condition above.
        if !validation_errors.is_empty() {
            status.last_failure_reason = Some(validation_errors.join("; "));
        }
    }
}

fn resolved_refs_identity(status: Option<&EdgionAcmeStatus>) -> Option<(&str, &str)> {
    status
        .and_then(|status| {
            status
                .conditions
                .iter()
                .find(|condition| condition.type_ == condition_types::RESOLVED_REFS)
        })
        .map(|condition| (condition.status.as_str(), condition.reason.as_str()))
}

fn gateway_ref_error_condition(error: &AcmeGatewayRefError, generation: Option<i64>) -> Condition {
    let (reason, message) = match error {
        AcmeGatewayRefError::InvalidKind { group, kind } => (
            condition_reasons::INVALID_KIND,
            format!("challenge.gatewayRef must reference gateway.networking.k8s.io/Gateway, got {group}/{kind}"),
        ),
        AcmeGatewayRefError::RefNotPermitted { namespace, name } => (
            condition_reasons::REF_NOT_PERMITTED,
            format!("Reference to Gateway \"{namespace}/{name}\" is not permitted without a ReferenceGrant"),
        ),
        AcmeGatewayRefError::ParentNotFound { namespace, name } => (
            condition_reasons::NO_MATCHING_PARENT,
            format!("Referenced Gateway \"{namespace}/{name}\" was not found or is not controlled by Edgion"),
        ),
        AcmeGatewayRefError::NoMatchingListener => (
            condition_reasons::NO_MATCHING_PARENT,
            "challenge.gatewayRef sectionName and port do not select a listener".to_string(),
        ),
        AcmeGatewayRefError::NoHttpListener => (
            condition_reasons::NO_MATCHING_PARENT,
            "challenge.gatewayRef does not select an HTTP listener".to_string(),
        ),
        AcmeGatewayRefError::TooManyListeners => (
            condition_reasons::INVALID,
            "Referenced Gateway exceeds the HTTP-01 listener attachment limit".to_string(),
        ),
        AcmeGatewayRefError::UnresolvedGateway => (
            condition_reasons::NO_MATCHING_PARENT,
            "Referenced Gateway does not have a current Controller attachment proof".to_string(),
        ),
    };
    condition_false(condition_types::RESOLVED_REFS, reason, message, generation)
}

// ============================================================================
// Resolve-outcome → ResolvedRefs Action mapping
//
// Pure mapping from a credential `ResolveOutcome` to the next action `parse`
// should take on the account `ResolvedRefs` condition.
// ============================================================================

#[derive(Debug)]
enum AcmeResolveAction {
    /// Write the condition, continue.
    Continue(Condition),
    /// Write the condition, then short-circuit `parse`.
    /// Only `Account + DecodeError/InvalidSecretType`.
    Abort(Condition),
}

/// Map a `ResolveOutcome` to the next action for the `ResolvedRefs` condition.
///
/// Constructs the appropriate `Condition` (status + reason + message) and emits
/// a `tracing` event at the right level. Does not touch `HandlerContext`,
/// status, or the resource — pure with respect to caller state, so callers can
/// drive it from unit tests.
fn outcome_to_resolved_refs_action<T>(
    outcome: &ResolveOutcome<T>,
    resource_key: &str,
    generation: Option<i64>,
) -> AcmeResolveAction {
    use AcmeResolveAction::*;

    match outcome {
        // ── Ok ────────────────────────────────────────────────────────────
        ResolveOutcome::Ok(_) => {
            tracing::debug!(edgion_acme = %resource_key, "ACME account Secret validated");
            Continue(condition_true(
                condition_types::RESOLVED_REFS,
                condition_reasons::RESOLVED_REFS,
                "ACME account credentials loaded",
                generation,
            ))
        }

        // ── Pending::SecretNotFound ───────────────────────────────────────
        ResolveOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
            tracing::info!(
                edgion_acme = %resource_key,
                %namespace,
                name = %name,
                "ACME account Secret absent, will be created on first registration"
            );
            Continue(condition_unknown(
                condition_types::RESOLVED_REFS,
                acme_reasons::NEEDS_REGISTRATION,
                "Account Secret absent or missing accountCredentials key, will be created/patched on first ACME registration",
                generation,
            ))
        }

        // ── Pending::ReferenceGrantMissing ────────────────────────────────
        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => {
            tracing::warn!(
                edgion_acme = %resource_key,
                from_ns = %from_ns,
                to_ns = %to_ns,
                secret_name = %secret_name,
                "Cross-namespace reference to account Secret denied (ReferenceGrant missing)"
            );
            // Cross-ns denial uses the Gateway-API standard `RefNotPermitted`
            // reason. The pipeline's `check_reference_allowed` returns a bare
            // bool so "no grant yet" and "grant denies" are indistinguishable.
            Continue(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::REF_NOT_PERMITTED,
                format!(
                    "Reference from {}/EdgionAcme to Secret \"{}/{}\" is not permitted without a ReferenceGrant",
                    from_ns, to_ns, secret_name
                ),
                generation,
            ))
        }

        // ── Invalid::MissingKeys ──────────────────────────────────────────
        ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(_)) => {
            // Deliberate divergence from LinkSys / EdgionPlugins / EdgionTls:
            // those handlers report `MissingKeys` as `ResolvedRefs=False`
            // (`reason=MissingKeys`, an Edgion extension) because a Secret
            // with missing credential keys is a hard operator error for
            // generic credential flows.
            //
            // ACME is different — `AcmeService` will *auto-patch* the
            // `accountCredentials` key into the Secret on first
            // registration. So a Secret that exists but lacks
            // `accountCredentials` is an *expected bootstrap state*, not
            // a failure; we surface it as `ResolvedRefs=Unknown` /
            // `NeedsRegistration` to match the "absent Secret" case above
            // and avoid spurious alerts.
            //
            // If the auto-patch path is ever removed, this arm should
            // collapse into the `DecodeError` / `InvalidSecretType` pattern.
            tracing::info!(
                edgion_acme = %resource_key,
                "ACME account Secret exists without accountCredentials key, will be patched on first registration"
            );
            Continue(condition_unknown(
                condition_types::RESOLVED_REFS,
                acme_reasons::NEEDS_REGISTRATION,
                "Account Secret absent or missing accountCredentials key, will be created/patched on first ACME registration",
                generation,
            ))
        }

        // ── Invalid::DecodeError ──────────────────────────────────────────
        ResolveOutcome::Invalid(InvalidCredentialError::DecodeError { key, detail }) => {
            tracing::error!(
                edgion_acme = %resource_key,
                key = %key,
                detail = %detail,
                "ACME account Secret data is corrupted (will not auto-repair)"
            );
            Abort(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::DECODE_ERROR,
                format!("credential decode failed for key '{}': {}", key, detail),
                generation,
            ))
        }

        ResolveOutcome::Invalid(InvalidCredentialError::InvalidValue { key, class }) => {
            tracing::error!(
                edgion_acme = %resource_key,
                key = %key,
                class = %class,
                "ACME account Secret contains an invalid credential value"
            );
            Abort(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::INVALID,
                format!("credential value is invalid for key '{}': {}", key, class),
                generation,
            ))
        }

        // ── Invalid::InvalidSecretType ────────────────────────────────────
        ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, allowed }) => {
            tracing::error!(
                edgion_acme = %resource_key,
                got = %got,
                ?allowed,
                "ACME account Secret has unsupported type"
            );
            Abort(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::INVALID_SECRET_TYPE,
                format!(
                    "Secret.type {:?} is not accepted for this credential (allowed: {:?})",
                    got, allowed
                ),
                generation,
            ))
        }
    }
}

// Unit tests for this module live in `tests/edgion_acme_test.rs`.
#[cfg(test)]
#[path = "tests/edgion_acme_test.rs"]
mod tests;
