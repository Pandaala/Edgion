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
                            // we must NOT silently drop the condition. Write
                            // it (graceful degrade) and emit a structured
                            // warn so the unintentional contract change is
                            // visible to operators in both debug and release.
                            update_condition(&mut status.conditions, cond);
                            tracing::warn!(
                                edgion_acme = %resource_ref.key(),
                                provider = %provider,
                                "Dns role unexpectedly returned Abort — condition written, continuing"
                            );
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

// Unit tests for this module live in `tests/edgion_acme_test.rs`.
#[cfg(test)]
#[path = "tests/edgion_acme_test.rs"]
mod tests;
