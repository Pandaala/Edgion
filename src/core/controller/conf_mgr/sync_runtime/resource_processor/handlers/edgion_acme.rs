//! EdgionAcme Handler (Controller side)
//!
//! Handles EdgionAcme resources with:
//! - Validation (email, domains, challenge config, etc.)
//! - DNS credential Secret reference resolution → `ResolvedRefs` condition
//!   + SecretRefManager registration
//! - ACME account Secret existence check → `ResolvedRefs` condition
//!   + SecretRefManager registration
//! - Status management
//! - Notifying AcmeService for background certificate issuance/renewal
//!
//! Resolved Secret bytes are NOT attached to the in-memory `EdgionAcme` —
//! `AcmeService` re-fetches via the Kubernetes API. The handler's job here is
//! purely validation/observability + cache wiring.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_secret, InvalidCredentialError, PendingReason, ResolveOutcome,
};

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_false, condition_reasons, condition_true, condition_types, condition_unknown, update_condition,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::core::controller::services::acme::notify_resource_changed;
use crate::types::credentials::resolvers::{AcmeAccountResolver, DnsCredentialResolver};
use crate::types::prelude_resources::EdgionAcme;
use crate::types::resources::common::Condition;
use crate::types::resources::edgion_acme::{AcmeChallenge, EdgionAcmeStatus};
use crate::types::ResourceKind;

/// EdgionAcme handler
///
/// Features:
/// - validate: Check email, domains, challenge config consistency
/// - parse: Resolve DNS credential Secret reference + ACME account credential
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
/// strings for the same underlying errors (M6/G).
mod acme_reasons {
    /// ACME-specific · Secret absent or missing `accountCredentials` key —
    /// service will create/patch on first registration. Maps to
    /// `ResolvedRefs=Unknown`, not `False`, because this is the expected
    /// bootstrap state rather than a failure.
    pub const NEEDS_REGISTRATION: &str = "NeedsRegistration";
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionAcme> for EdgionAcmeHandler {
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

        // Validate wildcard domains require DNS-01
        if acme.has_wildcard_domains() && !acme.is_dns01() {
            errors.push("EdgionAcme: wildcard domains (*.example.com) require dns-01 challenge type".to_string());
        }

        // Validate provider name on DNS-01. The tagged enum already enforces
        // that http-01 and dns-01 cannot mix fields, so no "missing http01"
        // / "missing dns01" arms are needed.
        if let AcmeChallenge::Dns01 { provider, .. } = &acme.spec.challenge {
            match provider.as_str() {
                "cloudflare" | "alidns" => {}
                other => {
                    errors.push(format!(
                        "EdgionAcme: unsupported DNS provider '{}', supported: cloudflare, alidns",
                        other
                    ));
                }
            }
        }

        errors
    }

    async fn parse(&self, mut acme: EdgionAcme, ctx: &HandlerContext) -> ProcessResult<EdgionAcme> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionAcme,
            acme.metadata.namespace.clone(),
            acme.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        // Snapshot pre-parse state for change-gated notification (see notify guard below).
        // `prev_controller_gen` is the generation last written by `update_status`; it differs
        // from the current generation only when the spec changed. `prev_resolved_refs` captures
        // the credential state so a Secret add/remove/corrupt triggers a notify even when
        // generation is identical.
        let current_generation = acme.metadata.generation;
        let prev_controller_gen = acme
            .status
            .as_ref()
            .and_then(|s| s.conditions.iter().find(|c| c.type_ == condition_types::ACCEPTED))
            .and_then(|c| c.observed_generation);
        let prev_resolved_refs = acme
            .status
            .as_ref()
            .and_then(|s| s.conditions.iter().find(|c| c.type_ == condition_types::RESOLVED_REFS))
            .map(|c| (c.status.clone(), c.reason.clone()));

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

        // ── Map outcome → ResolvedRefs Action (P0-07 unified mapping) ─────
        let resource_key = resource_ref.key();
        let account_action =
            outcome_to_resolved_refs_action(&outcome, &resource_key, generation, &AcmeResolveCtx::Account);
        // `account_corrupted` means the Secret exists but carries unusable
        // `accountCredentials` (decode failure / unsupported Secret.type).
        // We still return Continue so `update_status` persists the condition,
        // but we must NOT notify AcmeService — it must not attempt registration
        // against a broken account.
        let account_corrupted = match account_action {
            AcmeResolveAction::NoAction => false,
            AcmeResolveAction::Continue(cond) => {
                update_condition(&mut status.conditions, cond);
                false
            }
            AcmeResolveAction::Abort(cond) => {
                update_condition(&mut status.conditions, cond);
                true
            }
        };

        // `outcome` was already mapped to a condition above; the resolved
        // Secret bytes are intentionally NOT attached to the in-memory
        // resource — AcmeService re-fetches via the Kubernetes API.

        if account_corrupted {
            // Return Continue so `update_status` persists `ResolvedRefs=False`
            // and Gateway/operators can see the failure. We intentionally skip
            // DNS credential resolution and `notify_resource_changed` — without
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

        // ── Resolve DNS credential Secret (if DNS-01) ───────────────────────
        //
        // M8: run the same `resolve_secret` pipeline that LinkSys / EdgionTls /
        // EdgionPlugins use, so DNS credential resolution produces the same
        // four failure signals on `ResolvedRefs`:
        //   * `SecretNotFound`    — Secret object missing
        //   * `RefNotPermitted`   — cross-namespace `ReferenceGrant` absent
        //   * `MissingKeys`       — Secret lacks provider-specific keys
        //                           (`api-token` for Cloudflare,
        //                           `access-key-{id,secret}` for AliDNS)
        //   * `DecodeError`       — key value isn't valid UTF-8
        //
        // Condition aggregation with the account Secret branch above:
        // DNS only **overrides** `ResolvedRefs` with a harder-than-Unknown
        // failure. Concretely: if the account branch already wrote
        //   - `True`                 → DNS False overrides (exposes DNS error)
        //   - `Unknown=NeedsReg`     → DNS False overrides (DNS misconfig is
        //                              a harder failure than "will register
        //                              on first run"), DNS Ok is a no-op
        //                              (preserves the soft NeedsRegistration
        //                              signal)
        //   - `False=<account-err>`  → DNS False overrides with DNS message;
        //                              message-level info about the account
        //                              error is lost, but the condition stays
        //                              False and the operator is informed of
        //                              at least one actionable failure. This
        //                              is acceptable; aggregating both errors
        //                              into a single message is deferred to
        //                              a future refactor that collapses both
        //                              resolve steps through a shared
        //                              `ResolvedRefsError` collector.
        if let AcmeChallenge::Dns01 {
            provider,
            credential_ref,
            ..
        } = &acme.spec.challenge
        {
            // `validate()` already rejects unknown providers with an `Accepted=False`
            // reason, but we still reach `parse()` on the next reconcile.
            // Guard the resolver construction so unknown providers skip the
            // pipeline cleanly (no SecretRefManager pollution for an
            // unreachable resolve step).
            match DnsCredentialResolver::from_provider(provider) {
                Some(resolver) => {
                    let outcome =
                        resolve_secret(ctx, &resource_ref, "edgion.io", "EdgionAcme", credential_ref, &resolver).await;

                    // ── Map outcome → ResolvedRefs Action (P0-07 unified) ─
                    let dns_action = outcome_to_resolved_refs_action(
                        &outcome,
                        &resource_key,
                        generation,
                        &AcmeResolveCtx::Dns {
                            provider: provider.as_str(),
                        },
                    );
                    match dns_action {
                        AcmeResolveAction::NoAction => {}
                        AcmeResolveAction::Continue(cond) => {
                            update_condition(&mut status.conditions, cond);
                        }
                        AcmeResolveAction::Abort(cond) => {
                            // Today `role=Dns` never returns `Abort`, but if a
                            // future refactor introduces a Dns-corrupted path
                            // we must NOT silently drop the condition in
                            // release builds. Write it (graceful degrade) and
                            // raise a debug-only assertion so the unintentional
                            // contract change surfaces in dev/CI.
                            update_condition(&mut status.conditions, cond);
                            debug_assert!(false, "Dns role unexpectedly returned Abort");
                        }
                    }
                }
                None => {
                    // Unknown provider — `validate()` already surfaces this
                    // as Accepted=False/Invalid. Skip the DNS resolve step
                    // so we don't register a SecretRefManager entry for a
                    // resource that can't succeed anyway.
                    tracing::warn!(
                        edgion_acme = %resource_ref.key(),
                        provider = %provider,
                        "Unsupported DNS provider — skipping DNS credential resolution (see Accepted condition for details)"
                    );
                }
            }
        }

        // Notify AcmeService only when something meaningful changed:
        //   spec_changed  — generation advanced (user edited spec) or first create (no prev gen)
        //   creds_changed — ResolvedRefs status/reason flipped (Secret added/deleted/corrupted)
        //
        // AcmeService status writes (phase, certificate fields) trigger a reconcile with the same
        // generation and unchanged credentials. Without this guard those writes caused an infinite
        // notify→re-check→status-write→reconcile→notify loop. Restart safety is handled by
        // `scan_all_acme_resources`, which fires `notify_resource_changed` for every resource
        // ~10 s after `start_acme_service`.
        let new_resolved_refs = acme
            .status
            .as_ref()
            .and_then(|s| s.conditions.iter().find(|c| c.type_ == condition_types::RESOLVED_REFS))
            .map(|c| (c.status.clone(), c.reason.clone()));
        let spec_changed = current_generation != prev_controller_gen || prev_controller_gen.is_none();
        let creds_changed = prev_resolved_refs != new_resolved_refs;
        if spec_changed || creds_changed {
            let ns = acme.metadata.namespace.as_deref().unwrap_or("default");
            let name = acme.metadata.name.as_deref().unwrap_or_default();
            notify_resource_changed(format!("{}/{}", ns, name));
        }

        ProcessResult::Continue(acme)
    }

    async fn on_delete(&self, acme: &EdgionAcme, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionAcme,
            acme.metadata.namespace.clone(),
            acme.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        tracing::debug!(
            edgion_acme = %resource_ref.key(),
            "Cleared secret references on EdgionAcme delete"
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

// ============================================================================
// Resolve-outcome → ResolvedRefs Action mapping (P0-07)
//
// Pure mapping from a credential `ResolveOutcome` to the next action `parse`
// should take on the `ResolvedRefs` condition. Centralizes the 12 (account+DNS)
// × (6-variant) condition construction sites into a single match keyed on
// `(outcome, role)`, so future changes touch one location instead of twelve.
// ============================================================================

/// Credential context: drives ACME-bootstrap semantics and the Ok-arm behavior.
///
/// Per-variant behavior:
/// - `Account`
///   - `Ok`                    → `True / RESOLVED_REFS` (write condition)
///   - `SecretNotFound`        → `Unknown / NEEDS_REGISTRATION` (auto-bootstrap)
///   - `MissingKeys`           → `Unknown / NEEDS_REGISTRATION` (auto-bootstrap)
///   - `ReferenceGrantMissing` → `False / REF_NOT_PERMITTED`
///   - `DecodeError`           → `False / DECODE_ERROR` + `Abort` (corrupt account)
///   - `InvalidSecretType`     → `False / INVALID_SECRET_TYPE` + `Abort`
/// - `Dns { provider }`
///   - `Ok`                    → `NoAction` (don't override account branch's True/Unknown)
///   - all failures            → `False / <reason>`; never returns `Abort`
///   - `provider` is interpolated into the `MissingKeys` message
///
/// Encoded as an enum so `Account` cannot carry a provider and `Dns` cannot
/// omit one — the contract is enforced statically.
#[derive(Debug)]
enum AcmeResolveCtx<'a> {
    Account,
    Dns { provider: &'a str },
}

#[derive(Debug)]
enum AcmeResolveAction {
    /// Don't write a condition, don't abort. Only `Dns + Ok`.
    NoAction,
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
    ctx: &AcmeResolveCtx<'_>,
) -> AcmeResolveAction {
    use AcmeResolveAction::*;
    use AcmeResolveCtx::*;

    match (outcome, ctx) {
        // ── Ok ────────────────────────────────────────────────────────────
        (ResolveOutcome::Ok(_), Account) => {
            tracing::debug!(edgion_acme = %resource_key, "ACME account Secret validated");
            Continue(condition_true(
                condition_types::RESOLVED_REFS,
                condition_reasons::RESOLVED_REFS,
                "ACME account credentials loaded",
                generation,
            ))
        }

        // ── Pending::SecretNotFound ───────────────────────────────────────
        (ResolveOutcome::Pending(PendingReason::SecretNotFound { namespace, name }), Account) => {
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
        (
            ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns,
                to_ns,
                secret_name,
            }),
            Account,
        ) => {
            tracing::warn!(
                edgion_acme = %resource_key,
                from_ns = %from_ns,
                to_ns = %to_ns,
                secret_name = %secret_name,
                "Cross-namespace reference to account Secret denied (ReferenceGrant missing)"
            );
            // M6/F: cross-ns denial collapses onto Gateway-API standard
            // `RefNotPermitted`. The pipeline's `check_reference_allowed`
            // returns a bare bool, so distinguishing "no grant yet" from
            // "grant denies" is impossible — we used to advertise such a
            // distinction via `CrossNsReferenceNotPermitted`, but it was
            // always misleading.
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
        (ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(_)), Account) => {
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
            // If M6/G ever unifies ACME onto the generic pipeline
            // (removing the auto-patch path), this arm should collapse
            // into the `DecodeError` / `InvalidSecretType` pattern.
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
        (ResolveOutcome::Invalid(InvalidCredentialError::DecodeError { key, detail }), Account) => {
            tracing::error!(
                edgion_acme = %resource_key,
                key = %key,
                detail = %detail,
                "ACME account Secret data is corrupted (will not auto-repair)"
            );
            // M6/G: align ACME with the generic credential pipeline. This
            // was `AcmeAccountCorrupted` before, but that reason subsumed
            // both DecodeError and InvalidSecretType into one bucket,
            // diverging from how LinkSys / EdgionPlugins / EdgionTls
            // report the same pipeline variants.
            Abort(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::DECODE_ERROR,
                format!("credential decode failed for key '{}': {}", key, detail),
                generation,
            ))
        }

        // ── Invalid::InvalidSecretType ────────────────────────────────────
        (ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, allowed }), Account) => {
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

        // ── Ok (Dns) ──────────────────────────────────────────────────────
        (ResolveOutcome::Ok(_), Dns { .. }) => {
            // Do NOT write a condition: an account-branch True / Unknown
            // (NeedsRegistration) must survive a successful DNS resolve. See
            // the DNS-only-overrides aggregation rules in `parse` for details.
            tracing::debug!(edgion_acme = %resource_key, "DNS credential Secret resolved");
            NoAction
        }

        // ── Pending::SecretNotFound (Dns) ─────────────────────────────────
        (ResolveOutcome::Pending(PendingReason::SecretNotFound { namespace, name }), Dns { .. }) => {
            tracing::info!(
                edgion_acme = %resource_key,
                %namespace,
                name = %name,
                "DNS credential Secret not found yet, will be reprocessed when Secret arrives"
            );
            Continue(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::SECRET_NOT_FOUND,
                format!("DNS credential Secret '{}/{}' not found", namespace, name),
                generation,
            ))
        }

        // ── Pending::ReferenceGrantMissing (Dns) ──────────────────────────
        (
            ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns,
                to_ns,
                secret_name,
            }),
            Dns { .. },
        ) => {
            tracing::warn!(
                edgion_acme = %resource_key,
                from_ns = %from_ns,
                to_ns = %to_ns,
                secret_name = %secret_name,
                "Cross-namespace reference to DNS credential Secret denied (ReferenceGrant missing)"
            );
            Continue(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::REF_NOT_PERMITTED,
                format!(
                    "Reference from {}/EdgionAcme to DNS credential Secret \"{}/{}\" is not permitted without a ReferenceGrant",
                    from_ns, to_ns, secret_name
                ),
                generation,
            ))
        }

        // ── Invalid::MissingKeys (Dns) ────────────────────────────────────
        (ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(keys)), Dns { provider }) => {
            tracing::warn!(
                edgion_acme = %resource_key,
                provider = %provider,
                ?keys,
                "DNS credential Secret missing required keys"
            );
            Continue(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::MISSING_KEYS,
                format!(
                    "DNS credential Secret for provider '{}' is missing required keys: {}",
                    provider,
                    keys.join(", ")
                ),
                generation,
            ))
        }

        // ── Invalid::DecodeError (Dns) ────────────────────────────────────
        (ResolveOutcome::Invalid(InvalidCredentialError::DecodeError { key, detail }), Dns { .. }) => {
            tracing::error!(
                edgion_acme = %resource_key,
                key = %key,
                detail = %detail,
                "DNS credential Secret value could not be decoded"
            );
            Continue(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::DECODE_ERROR,
                format!("DNS credential decode failed for key '{}': {}", key, detail),
                generation,
            ))
        }

        // ── Invalid::InvalidSecretType (Dns) ──────────────────────────────
        (ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, allowed }), Dns { .. }) => {
            tracing::error!(
                edgion_acme = %resource_key,
                got = %got,
                ?allowed,
                "DNS credential Secret has unsupported type"
            );
            Continue(condition_false(
                condition_types::RESOLVED_REFS,
                condition_reasons::INVALID_SECRET_TYPE,
                format!(
                    "DNS credential Secret.type {:?} is not accepted (allowed: {:?})",
                    got, allowed
                ),
                generation,
            ))
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::core::controller::conf_mgr::ControllerRuntime;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::common::SecretObjectReference;
    use crate::types::resources::edgion_acme::AcmeStorageConfig;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::api::ObjectMeta;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    // ========================================================================
    // Test helpers
    // ========================================================================

    fn make_ctx(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(secrets);
        let ctx = HandlerContext::new(
            None,
            None,
            TriggerChain::default(),
            5,
            20,
            runtime.clone(),
        );
        (ctx, runtime)
    }

    fn make_secret(namespace: &str, name: &str, data: &[(&str, &[u8])]) -> (String, Secret) {
        let mut bmap = BTreeMap::new();
        for (k, v) in data {
            bmap.insert(k.to_string(), ByteString(v.to_vec()));
        }
        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            data: Some(bmap),
            ..Default::default()
        };
        let key = format!("{}/{}", namespace, name);
        (key, secret)
    }

    fn make_acme(ns: &str, name: &str, secret_name: &str) -> EdgionAcme {
        EdgionAcme {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: crate::types::resources::edgion_acme::EdgionAcmeSpec {
                server: "https://acme-v02.api.letsencrypt.org/directory".to_string(),
                email: "admin@example.com".to_string(),
                private_key_secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: secret_name.to_string(),
                    namespace: None,
                },
                domains: vec!["example.com".to_string()],
                key_type: Default::default(),
                challenge: AcmeChallenge::Http01 {
                    gateway_ref: ParentReference {
                        group: None,
                        kind: None,
                        namespace: Some("default".to_string()),
                        name: "gw".to_string(),
                        section_name: None,
                        port: None,
                    },
                },
                renewal: Default::default(),
                external_account_binding: None,
                storage: AcmeStorageConfig {
                    secret_name: "cert".to_string(),
                    secret_namespace: None,
                },
                auto_edgion_tls: Default::default(),
            },
            status: None,
        }
    }

    // ========================================================================
    // Shared scenario type and driver for parse() table-driven tests
    // ========================================================================

    struct AcmeParseScenario {
        name: &'static str,
        secrets: Vec<(String, Secret)>,
        acme: EdgionAcme,
        expected_status: &'static str,
        expected_reason: Option<&'static str>,
        expected_message_substrings: &'static [&'static str],
        /// Each entry is a secret store key (e.g. "default/missing-secret").
        /// The driver asserts that the EdgionAcme owner is registered for that key.
        expected_refs_owner_for_keys: &'static [&'static str],
    }

    async fn run_acme_parse_scenario(scenario: AcmeParseScenario) {
        let secrets: HashMap<String, Secret> = scenario.secrets.into_iter().collect();
        let (ctx, _runtime) = make_ctx(secrets);
        let handler = EdgionAcmeHandler::new();

        let acme_ns = scenario.acme.metadata.namespace.clone().unwrap_or_default();
        let acme_name = scenario.acme.metadata.name.clone().unwrap_or_default();

        let result = handler.parse(scenario.acme, &ctx).await;

        let resolved = match result {
            ProcessResult::Continue(acme) => {
                let status = acme
                    .status
                    .unwrap_or_else(|| panic!("[{}] status should be set", scenario.name));
                status
                    .conditions
                    .into_iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .unwrap_or_else(|| panic!("[{}] ResolvedRefs condition should exist", scenario.name))
            }
            ProcessResult::Skip { reason } => {
                panic!("[{}] expected Continue, got Skip: {reason}", scenario.name)
            }
        };

        assert_eq!(
            resolved.status, scenario.expected_status,
            "[{}] status mismatch",
            scenario.name
        );

        if let Some(expected_reason) = scenario.expected_reason {
            assert_eq!(resolved.reason, expected_reason, "[{}] reason mismatch", scenario.name);
        }

        for &substr in scenario.expected_message_substrings {
            assert!(
                resolved.message.contains(substr),
                "[{}] message missing {:?}, got: {}",
                scenario.name,
                substr,
                resolved.message
            );
        }

        let owner = ResourceRef::new(ResourceKind::EdgionAcme, Some(acme_ns), acme_name);
        for &secret_key in scenario.expected_refs_owner_for_keys {
            let refs = ctx.secret_ref_manager().get_refs(secret_key);
            assert!(
                refs.contains(&owner),
                "[{}] SecretRefManager should contain owner for key {secret_key}",
                scenario.name
            );
        }
    }

    // ========================================================================
    // Test: account-branch parse scenarios (Http01 challenge)
    // ========================================================================

    #[tokio::test]
    async fn test_acme_account_resolve_scenarios() {
        let scenarios: Vec<AcmeParseScenario> = vec![
            // 1. Happy path — Secret present with valid accountCredentials.
            AcmeParseScenario {
                name: "account_credential_happy_path",
                secrets: vec![make_secret(
                    "default",
                    "acme-account",
                    &[("accountCredentials", br#"{"ok":true}"#)],
                )],
                acme: make_acme("default", "my-acme", "acme-account"),
                expected_status: "True",
                expected_reason: None,
                expected_message_substrings: &[],
                expected_refs_owner_for_keys: &[],
            },
            // 2. Secret not found → NeedsRegistration (Unknown).
            //    SecretRefManager must register the missing secret so the
            //    resource re-reconciles when it arrives.
            AcmeParseScenario {
                name: "secret_not_found_needs_registration",
                secrets: vec![],
                acme: make_acme("default", "my-acme", "missing-secret"),
                expected_status: "Unknown",
                expected_reason: Some("NeedsRegistration"),
                expected_message_substrings: &[],
                expected_refs_owner_for_keys: &["default/missing-secret"],
            },
            // 3. Secret exists but missing accountCredentials key → NeedsRegistration.
            AcmeParseScenario {
                name: "secret_missing_key_needs_registration",
                secrets: vec![make_secret("default", "acme-account", &[("wrongKey", b"some-value")])],
                acme: make_acme("default", "my-acme", "acme-account"),
                expected_status: "Unknown",
                expected_reason: Some("NeedsRegistration"),
                expected_message_substrings: &[],
                expected_refs_owner_for_keys: &[],
            },
            // 4. Corrupt account data → Continue + ResolvedRefs=False/DecodeError.
            //    ACME uses the generic DecodeError reason (M6/G) shared with
            //    LinkSys / EdgionPlugins / EdgionTls instead of a private bucket.
            AcmeParseScenario {
                name: "corrupt_account_data_returns_continue_with_resolved_refs_false",
                secrets: vec![make_secret(
                    "default",
                    "acme-account",
                    &[("accountCredentials", b"not-valid-json")],
                )],
                acme: make_acme("default", "my-acme", "acme-account"),
                expected_status: "False",
                expected_reason: Some("DecodeError"),
                expected_message_substrings: &["decode failed"],
                expected_refs_owner_for_keys: &[],
            },
        ];

        for scenario in scenarios {
            run_acme_parse_scenario(scenario).await;
        }
    }

    // ========================================================================
    // Test: DNS-branch parse scenarios (Dns01 challenge)
    // ========================================================================
    //
    // These cover the four ResolveOutcome paths the DNS resolve branch can
    // take — SecretNotFound, ReferenceGrantMissing, MissingKeys, and the
    // happy path. The account Secret is present and valid where noted so
    // the tests isolate the DNS branch's contribution to ResolvedRefs.

    #[tokio::test]
    async fn test_acme_dns_resolve_scenarios() {
        let (valid_account_key, valid_account_secret) =
            make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);

        let scenarios: Vec<AcmeParseScenario> = vec![
            // 6. DNS Secret missing → ResolvedRefs=False/SecretNotFound.
            //    Account Secret present so account branch writes True first;
            //    DNS branch must override with False/SecretNotFound.
            AcmeParseScenario {
                name: "dns_credential_secret_not_found",
                secrets: vec![(valid_account_key.clone(), valid_account_secret.clone())],
                acme: make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "missing-dns", None),
                expected_status: "False",
                expected_reason: Some("SecretNotFound"),
                expected_message_substrings: &["DNS credential"],
                expected_refs_owner_for_keys: &["default/missing-dns"],
            },
            // 7. Cloudflare DNS Secret missing api-token → ResolvedRefs=False/MissingKeys.
            AcmeParseScenario {
                name: "dns_credential_missing_api_token",
                secrets: vec![
                    (valid_account_key.clone(), valid_account_secret.clone()),
                    make_secret("default", "cf-secret", &[("wrong-key", b"value")]),
                ],
                acme: make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "cf-secret", None),
                expected_status: "False",
                expected_reason: Some("MissingKeys"),
                expected_message_substrings: &["api-token"],
                expected_refs_owner_for_keys: &[],
            },
            // 8. AliDNS Secret missing both keys → message names both.
            AcmeParseScenario {
                name: "dns_credential_alidns_missing_both_keys",
                secrets: vec![
                    (valid_account_key.clone(), valid_account_secret.clone()),
                    make_secret("default", "alidns-secret", &[("unrelated", b"v")]),
                ],
                acme: make_acme_dns01("default", "my-acme", "acme-account", "alidns", "alidns-secret", None),
                expected_status: "False",
                expected_reason: Some("MissingKeys"),
                expected_message_substrings: &["access-key-id", "access-key-secret"],
                expected_refs_owner_for_keys: &[],
            },
            // 9. DNS Secret happy path + account happy path → True/ResolvedRefs.
            //    Reason must equal the literal "ResolvedRefs" constant (not just status==True).
            AcmeParseScenario {
                name: "dns_credential_happy_path_preserves_true",
                secrets: vec![
                    (valid_account_key.clone(), valid_account_secret.clone()),
                    make_secret("default", "cf-secret", &[("api-token", b"cf-token-value")]),
                ],
                acme: make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "cf-secret", None),
                expected_status: "True",
                expected_reason: Some("ResolvedRefs"),
                expected_message_substrings: &[],
                expected_refs_owner_for_keys: &[],
            },
            // 10. DNS Ok but account pending (NeedsRegistration) → Unknown survives.
            //     DNS success must NOT override account's soft Unknown state.
            AcmeParseScenario {
                name: "dns_credential_ok_preserves_account_needs_registration",
                secrets: vec![make_secret("default", "cf-secret", &[("api-token", b"cf-token")])],
                acme: make_acme_dns01("default", "my-acme", "missing-account", "cloudflare", "cf-secret", None),
                expected_status: "Unknown",
                expected_reason: Some("NeedsRegistration"),
                expected_message_substrings: &[],
                expected_refs_owner_for_keys: &[],
            },
            // 11. Account pending + DNS missing → DNS False overrides account's soft Unknown.
            //     Hard failure (False) beats soft pending (Unknown).
            AcmeParseScenario {
                name: "dns_credential_false_overrides_account_needs_registration",
                secrets: vec![],
                acme: make_acme_dns01(
                    "default",
                    "my-acme",
                    "missing-account",
                    "cloudflare",
                    "missing-dns",
                    None,
                ),
                expected_status: "False",
                expected_reason: Some("SecretNotFound"),
                expected_message_substrings: &["DNS credential"],
                expected_refs_owner_for_keys: &[],
            },
            // 12. Cross-namespace DNS credential without ReferenceGrant →
            //     ResolvedRefs=False/RefNotPermitted; message must name the target ns.
            AcmeParseScenario {
                name: "dns_credential_cross_namespace_denied",
                secrets: vec![
                    (valid_account_key.clone(), valid_account_secret.clone()),
                    make_secret("credentials", "cf-secret", &[("api-token", b"cf-token")]),
                ],
                acme: make_acme_dns01(
                    "default",
                    "my-acme",
                    "acme-account",
                    "cloudflare",
                    "cf-secret",
                    Some("credentials"),
                ),
                expected_status: "False",
                expected_reason: Some("RefNotPermitted"),
                expected_message_substrings: &["credentials"],
                expected_refs_owner_for_keys: &[],
            },
            // 13. Unknown DNS provider → DNS resolve branch is a no-op
            //     (validate() is the canonical error path).
            //     Account's True must survive. Reason is not checked (None).
            AcmeParseScenario {
                name: "dns_credential_unknown_provider_skips_dns_resolve",
                secrets: vec![(valid_account_key.clone(), valid_account_secret.clone())],
                acme: make_acme_dns01(
                    "default",
                    "my-acme",
                    "acme-account",
                    "route53-not-supported",
                    "any-secret",
                    None,
                ),
                expected_status: "True",
                expected_reason: None,
                expected_message_substrings: &[],
                expected_refs_owner_for_keys: &[],
            },
        ];

        for scenario in scenarios {
            run_acme_parse_scenario(scenario).await;
        }
    }

    // ========================================================================
    // Test: on_delete clears SecretRefManager
    // ========================================================================

    #[tokio::test]
    async fn test_on_delete_clears_refs() {
        let (key, secret) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme("default", "my-acme", "acme-account");

        // Parse to register reference
        let result = handler.parse(acme.clone(), &ctx).await;
        assert!(matches!(result, ProcessResult::Continue(_)));

        let owner = ResourceRef::new(
            ResourceKind::EdgionAcme,
            Some("default".to_string()),
            "my-acme".to_string(),
        );
        let refs = ctx.secret_ref_manager().get_refs("default/acme-account");
        assert!(refs.contains(&owner), "reference should be registered after parse");

        // Delete
        handler.on_delete(&acme, &ctx).await;

        let refs = ctx.secret_ref_manager().get_refs("default/acme-account");
        assert!(!refs.contains(&owner), "reference should be cleared after on_delete");
    }

    fn make_acme_dns01(
        ns: &str,
        name: &str,
        account_secret: &str,
        dns_provider: &str,
        dns_secret_name: &str,
        dns_secret_ns: Option<&str>,
    ) -> EdgionAcme {
        EdgionAcme {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(ns.to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: crate::types::resources::edgion_acme::EdgionAcmeSpec {
                server: "https://acme-v02.api.letsencrypt.org/directory".to_string(),
                email: "admin@example.com".to_string(),
                private_key_secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: account_secret.to_string(),
                    namespace: None,
                },
                domains: vec!["*.example.com".to_string()],
                key_type: Default::default(),
                challenge: AcmeChallenge::Dns01 {
                    provider: dns_provider.to_string(),
                    credential_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: dns_secret_name.to_string(),
                        namespace: dns_secret_ns.map(|s| s.to_string()),
                    },
                    propagation_timeout: 120,
                    propagation_check_interval: 5,
                },
                renewal: Default::default(),
                external_account_binding: None,
                storage: AcmeStorageConfig {
                    secret_name: "cert".to_string(),
                    secret_namespace: None,
                },
                auto_edgion_tls: Default::default(),
            },
            status: None,
        }
    }

    // ========================================================================
    // outcome_to_resolved_refs_action: table-driven tests
    // ========================================================================

    /// Expected outcome shape for a single scenario row.
    enum ExpectedAction {
        /// AcmeResolveAction::NoAction
        NoAction,
        /// AcmeResolveAction::Continue with the given field expectations
        Continue {
            status: &'static str,
            reason: &'static str,
            message_substrings: &'static [&'static str],
        },
        /// AcmeResolveAction::Abort with the given field expectations
        Abort {
            status: &'static str,
            reason: &'static str,
            message_substrings: &'static [&'static str],
        },
    }

    struct OutcomeScenario {
        name: &'static str,
        outcome: ResolveOutcome<Secret>,
        ctx: AcmeResolveCtx<'static>,
        generation: Option<i64>,
        expected: ExpectedAction,
    }

    fn run_outcome_scenario(scenario: OutcomeScenario) {
        let action =
            outcome_to_resolved_refs_action(&scenario.outcome, "default/acme", scenario.generation, &scenario.ctx);
        match (&scenario.expected, action) {
            (ExpectedAction::NoAction, AcmeResolveAction::NoAction) => {}
            (
                ExpectedAction::Continue {
                    status,
                    reason,
                    message_substrings,
                },
                AcmeResolveAction::Continue(cond),
            ) => {
                assert_eq!(cond.status, *status, "[{}] status mismatch", scenario.name);
                assert_eq!(cond.reason, *reason, "[{}] reason mismatch", scenario.name);
                for &substr in *message_substrings {
                    assert!(
                        cond.message.contains(substr),
                        "[{}] message missing {:?}, got: {}",
                        scenario.name,
                        substr,
                        cond.message
                    );
                }
                assert_eq!(
                    cond.observed_generation, scenario.generation,
                    "[{}] generation mismatch",
                    scenario.name
                );
            }
            (
                ExpectedAction::Abort {
                    status,
                    reason,
                    message_substrings,
                },
                AcmeResolveAction::Abort(cond),
            ) => {
                assert_eq!(cond.status, *status, "[{}] status mismatch", scenario.name);
                assert_eq!(cond.reason, *reason, "[{}] reason mismatch", scenario.name);
                for &substr in *message_substrings {
                    assert!(
                        cond.message.contains(substr),
                        "[{}] message missing {:?}, got: {}",
                        scenario.name,
                        substr,
                        cond.message
                    );
                }
                assert_eq!(
                    cond.observed_generation, scenario.generation,
                    "[{}] generation mismatch",
                    scenario.name
                );
            }
            (expected, got) => {
                let expected_label = match expected {
                    ExpectedAction::NoAction => "NoAction",
                    ExpectedAction::Continue { .. } => "Continue",
                    ExpectedAction::Abort { .. } => "Abort",
                };
                panic!("[{}] expected {}, got {:?}", scenario.name, expected_label, got);
            }
        }
    }

    #[test]
    fn test_account_outcome_scenarios() {
        let scenarios: Vec<OutcomeScenario> = vec![
            OutcomeScenario {
                name: "account_ok_returns_continue_with_true_resolved_refs",
                outcome: ResolveOutcome::Ok(Secret::default()),
                ctx: AcmeResolveCtx::Account,
                generation: Some(1),
                expected: ExpectedAction::Continue {
                    status: "True",
                    reason: condition_reasons::RESOLVED_REFS,
                    message_substrings: &["ACME account credentials loaded"],
                },
            },
            OutcomeScenario {
                name: "account_secret_not_found_returns_continue_unknown_needs_registration",
                outcome: ResolveOutcome::Pending(PendingReason::SecretNotFound {
                    namespace: "default".to_string(),
                    name: "acme-account".to_string(),
                }),
                ctx: AcmeResolveCtx::Account,
                generation: Some(2),
                expected: ExpectedAction::Continue {
                    status: "Unknown",
                    reason: acme_reasons::NEEDS_REGISTRATION,
                    message_substrings: &["Account Secret absent"],
                },
            },
            OutcomeScenario {
                name: "account_ref_grant_missing_returns_continue_false_ref_not_permitted",
                outcome: ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                    from_ns: "default".to_string(),
                    to_ns: "secrets".to_string(),
                    secret_name: "acme-key".to_string(),
                }),
                ctx: AcmeResolveCtx::Account,
                generation: Some(3),
                expected: ExpectedAction::Continue {
                    status: "False",
                    reason: condition_reasons::REF_NOT_PERMITTED,
                    message_substrings: &["Reference from default/EdgionAcme to Secret \"secrets/acme-key\""],
                },
            },
            OutcomeScenario {
                name: "account_missing_keys_returns_continue_unknown_needs_registration",
                // The Account MissingKeys arm uses a fixed bootstrap-state message
                // (NeedsRegistration) rather than interpolating the `keys` vec.
                outcome: ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(vec!["accountCredentials"])),
                ctx: AcmeResolveCtx::Account,
                generation: Some(4),
                expected: ExpectedAction::Continue {
                    status: "Unknown",
                    reason: acme_reasons::NEEDS_REGISTRATION,
                    message_substrings: &["Account Secret absent"],
                },
            },
            OutcomeScenario {
                name: "account_decode_error_returns_abort_false_decode_error",
                outcome: ResolveOutcome::Invalid(InvalidCredentialError::DecodeError {
                    key: "accountCredentials",
                    detail: "invalid utf-8 at byte 3".to_string(),
                }),
                ctx: AcmeResolveCtx::Account,
                generation: Some(5),
                expected: ExpectedAction::Abort {
                    status: "False",
                    reason: condition_reasons::DECODE_ERROR,
                    message_substrings: &["credential decode failed for key 'accountCredentials'", "invalid utf-8"],
                },
            },
            OutcomeScenario {
                name: "account_invalid_secret_type_returns_abort_false_invalid_type",
                outcome: ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType {
                    got: "Opaque".to_string(),
                    allowed: &["edgion.io/acme-account"],
                }),
                ctx: AcmeResolveCtx::Account,
                generation: Some(6),
                expected: ExpectedAction::Abort {
                    status: "False",
                    reason: condition_reasons::INVALID_SECRET_TYPE,
                    message_substrings: &["Secret.type", "Opaque", "not accepted for this credential"],
                },
            },
        ];

        for scenario in scenarios {
            run_outcome_scenario(scenario);
        }
    }

    #[test]
    fn test_dns_outcome_scenarios() {
        let scenarios: Vec<OutcomeScenario> = vec![
            OutcomeScenario {
                name: "dns_ok_returns_no_action",
                outcome: ResolveOutcome::Ok(Secret::default()),
                ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
                generation: Some(1),
                expected: ExpectedAction::NoAction,
            },
            OutcomeScenario {
                name: "dns_secret_not_found_returns_continue_false_secret_not_found",
                outcome: ResolveOutcome::Pending(PendingReason::SecretNotFound {
                    namespace: "default".to_string(),
                    name: "cf-token".to_string(),
                }),
                ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
                generation: Some(2),
                expected: ExpectedAction::Continue {
                    status: "False",
                    reason: condition_reasons::SECRET_NOT_FOUND,
                    message_substrings: &["DNS credential Secret 'default/cf-token' not found"],
                },
            },
            OutcomeScenario {
                name: "dns_ref_grant_missing_returns_continue_false_ref_not_permitted_with_dns_label",
                outcome: ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                    from_ns: "default".to_string(),
                    to_ns: "secrets".to_string(),
                    secret_name: "cf-token".to_string(),
                }),
                ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
                generation: Some(3),
                expected: ExpectedAction::Continue {
                    status: "False",
                    reason: condition_reasons::REF_NOT_PERMITTED,
                    message_substrings: &[
                        "Reference from default/EdgionAcme to DNS credential Secret \"secrets/cf-token\"",
                    ],
                },
            },
            OutcomeScenario {
                name: "dns_missing_keys_returns_continue_false_missing_keys_with_provider",
                outcome: ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(vec!["api-token"])),
                ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
                generation: Some(4),
                expected: ExpectedAction::Continue {
                    status: "False",
                    reason: condition_reasons::MISSING_KEYS,
                    message_substrings: &["DNS credential Secret for provider 'cloudflare'", "api-token"],
                },
            },
            OutcomeScenario {
                name: "dns_decode_error_returns_continue_false_decode_error_with_dns_label",
                outcome: ResolveOutcome::Invalid(InvalidCredentialError::DecodeError {
                    key: "api-token",
                    detail: "invalid utf-8 at byte 1".to_string(),
                }),
                ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
                generation: Some(5),
                expected: ExpectedAction::Continue {
                    status: "False",
                    reason: condition_reasons::DECODE_ERROR,
                    message_substrings: &["DNS credential decode failed for key 'api-token'", "invalid utf-8"],
                },
            },
            OutcomeScenario {
                name: "dns_invalid_secret_type_returns_continue_false_invalid_type_with_dns_label",
                outcome: ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType {
                    got: "kubernetes.io/tls".to_string(),
                    allowed: &["Opaque"],
                }),
                ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
                generation: Some(6),
                expected: ExpectedAction::Continue {
                    status: "False",
                    reason: condition_reasons::INVALID_SECRET_TYPE,
                    message_substrings: &["DNS credential Secret.type", "kubernetes.io/tls", "not accepted"],
                },
            },
        ];

        for scenario in scenarios {
            run_outcome_scenario(scenario);
        }
    }
}
