//! EdgionAcme Handler (Controller side)
//!
//! Handles EdgionAcme resources with:
//! - Validation (email, domains, challenge config, etc.)
//! - DNS credential Secret reference resolution
//! - ACME account Secret existence check + ResolvedRefs condition
//! - SecretRefManager registration for cascading updates
//! - Status management
//! - Notifying AcmeService for background certificate issuance/renewal
//!
//! When account credentials resolve successfully, the validated Secret is attached
//! to [`EdgionAcmeSpec::account_secret`] so `AcmeService` can restore the ACME
//! account without a redundant Kubernetes `GET`.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::credential::{
    resolve_secret, InvalidCredentialError, PendingReason, ResolveOutcome,
};

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_reasons, condition_types,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::core::controller::services::acme::notify_resource_changed;
use crate::types::credentials::resolvers::{AcmeAccountResolver, DnsCredentialResolver};
use crate::types::prelude_resources::EdgionAcme;
use crate::types::resources::common::SecretReference;
use crate::types::resources::edgion_acme::{AcmeChallengeType, EdgionAcmeStatus};
use crate::types::ResourceKind;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use k8s_openapi::chrono::Utc;

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
        if acme.has_wildcard_domains() && acme.spec.challenge.challenge_type != AcmeChallengeType::Dns01 {
            errors.push("EdgionAcme: wildcard domains (*.example.com) require dns-01 challenge type".to_string());
        }

        // Validate challenge config completeness
        match acme.spec.challenge.challenge_type {
            AcmeChallengeType::Http01 => {
                if acme.spec.challenge.http01.is_none() {
                    errors.push("EdgionAcme: http01 config is required when challenge type is http-01".to_string());
                }
            }
            AcmeChallengeType::Dns01 => {
                if acme.spec.challenge.dns01.is_none() {
                    errors.push("EdgionAcme: dns01 config is required when challenge type is dns-01".to_string());
                } else {
                    let dns01 = acme.spec.challenge.dns01.as_ref().unwrap();
                    // Validate provider name
                    match dns01.provider.as_str() {
                        "cloudflare" | "alidns" => {}
                        other => {
                            errors.push(format!(
                                "EdgionAcme: unsupported DNS provider '{}', supported: cloudflare, alidns",
                                other
                            ));
                        }
                    }
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
        acme.spec.account_secret = None;

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

        // `account_corrupted` means the Secret exists but carries unusable
        // `accountCredentials` (decode failure / unsupported Secret.type).
        // We still return Continue so `update_status` persists the condition,
        // but we must NOT notify AcmeService — it must not attempt registration
        // against a broken account.
        let account_corrupted = match outcome {
            ResolveOutcome::Ok(secret) => {
                acme.spec.account_secret = Some(secret);
                tracing::debug!(
                    edgion_acme = %resource_ref.key(),
                    "ACME account Secret validated"
                );
                let cond = k8s_condition_true(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::RESOLVED_REFS,
                    "ACME account credentials loaded",
                    generation,
                );
                update_k8s_condition(&mut status.conditions, cond);
                false
            }
            ResolveOutcome::Pending(PendingReason::SecretNotFound {
                ref namespace,
                ref name,
            }) => {
                tracing::info!(
                    edgion_acme = %resource_ref.key(),
                    %namespace,
                    name = %name,
                    "ACME account Secret absent, will be created on first registration"
                );
                let cond = k8s_condition_unknown(
                    condition_types::RESOLVED_REFS,
                    acme_reasons::NEEDS_REGISTRATION,
                    "Account Secret absent or missing accountCredentials key, will be created/patched on first ACME registration",
                    generation,
                );
                update_k8s_condition(&mut status.conditions, cond);
                false
            }
            ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns,
                to_ns,
                secret_name,
            }) => {
                tracing::warn!(
                    edgion_acme = %resource_ref.key(),
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
                let cond = k8s_condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::REF_NOT_PERMITTED,
                    format!(
                        "Reference from {}/EdgionAcme to Secret \"{}/{}\" is not permitted without a ReferenceGrant",
                        from_ns, to_ns, secret_name
                    ),
                    generation,
                );
                update_k8s_condition(&mut status.conditions, cond);
                false
            }
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
                // If M6/G ever unifies ACME onto the generic pipeline
                // (removing the auto-patch path), this arm should collapse
                // into the `DecodeError` / `InvalidSecretType` pattern.
                tracing::info!(
                    edgion_acme = %resource_ref.key(),
                    "ACME account Secret exists without accountCredentials key, will be patched on first registration"
                );
                let cond = k8s_condition_unknown(
                    condition_types::RESOLVED_REFS,
                    acme_reasons::NEEDS_REGISTRATION,
                    "Account Secret absent or missing accountCredentials key, will be created/patched on first ACME registration",
                    generation,
                );
                update_k8s_condition(&mut status.conditions, cond);
                false
            }
            ResolveOutcome::Invalid(InvalidCredentialError::DecodeError { key, detail }) => {
                tracing::error!(
                    edgion_acme = %resource_ref.key(),
                    key = %key,
                    detail = %detail,
                    "ACME account Secret data is corrupted (will not auto-repair)"
                );
                // M6/G: align ACME with the generic credential pipeline. This
                // was `AcmeAccountCorrupted` before, but that reason subsumed
                // both DecodeError and InvalidSecretType into one bucket,
                // diverging from how LinkSys / EdgionPlugins / EdgionTls
                // report the same pipeline variants.
                let cond = k8s_condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::DECODE_ERROR,
                    format!("credential decode failed for key '{}': {}", key, detail),
                    generation,
                );
                update_k8s_condition(&mut status.conditions, cond);
                true
            }
            ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, allowed }) => {
                tracing::error!(
                    edgion_acme = %resource_ref.key(),
                    got = %got,
                    ?allowed,
                    "ACME account Secret has unsupported type"
                );
                let cond = k8s_condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::INVALID_SECRET_TYPE,
                    format!(
                        "Secret.type {:?} is not accepted for this credential (allowed: {:?})",
                        got, allowed
                    ),
                    generation,
                );
                update_k8s_condition(&mut status.conditions, cond);
                true
            }
        };

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
        if let Some(ref dns01) = acme.spec.challenge.dns01 {
            let dns_secret_ref = SecretReference {
                name: dns01.credential_ref.name.clone(),
                namespace: dns01.credential_ref.namespace.clone(),
            };
            let secret_key = format_secret_key(
                dns_secret_ref.namespace.as_ref().or(acme.metadata.namespace.as_ref()),
                &dns_secret_ref.name,
            );

            // `validate()` already rejects unknown providers with an `Accepted=False`
            // reason, but we still reach `parse()` on the next reconcile.
            // Guard the resolver construction so unknown providers skip the
            // pipeline cleanly (no SecretRefManager pollution for an
            // unreachable resolve step).
            match DnsCredentialResolver::from_provider(&dns01.provider) {
                Some(resolver) => {
                    let outcome = resolve_secret(
                        ctx,
                        &resource_ref,
                        "edgion.io",
                        "EdgionAcme",
                        &dns_secret_ref,
                        &resolver,
                    )
                    .await;

                    match outcome {
                        ResolveOutcome::Ok(secret) => {
                            acme.spec.dns_credential_secret = Some(secret);
                            tracing::debug!(
                                edgion_acme = %resource_ref.key(),
                                secret_key = %secret_key,
                                "DNS credential Secret resolved"
                            );
                            // Do NOT write ResolvedRefs=True here — a True
                            // written by the account branch stays True, and
                            // an Unknown(NeedsRegistration) from the account
                            // branch intentionally survives DNS success.
                        }
                        ResolveOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
                            tracing::info!(
                                edgion_acme = %resource_ref.key(),
                                %namespace,
                                name = %name,
                                "DNS credential Secret not found yet, will be reprocessed when Secret arrives"
                            );
                            let cond = k8s_condition_false(
                                condition_types::RESOLVED_REFS,
                                condition_reasons::SECRET_NOT_FOUND,
                                format!("DNS credential Secret '{}/{}' not found", namespace, name),
                                generation,
                            );
                            update_k8s_condition(&mut status.conditions, cond);
                        }
                        ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                            from_ns,
                            to_ns,
                            secret_name,
                        }) => {
                            tracing::warn!(
                                edgion_acme = %resource_ref.key(),
                                from_ns = %from_ns,
                                to_ns = %to_ns,
                                secret_name = %secret_name,
                                "Cross-namespace reference to DNS credential Secret denied (ReferenceGrant missing)"
                            );
                            let cond = k8s_condition_false(
                                condition_types::RESOLVED_REFS,
                                condition_reasons::REF_NOT_PERMITTED,
                                format!(
                                    "Reference from {}/EdgionAcme to DNS credential Secret \"{}/{}\" is not permitted without a ReferenceGrant",
                                    from_ns, to_ns, secret_name
                                ),
                                generation,
                            );
                            update_k8s_condition(&mut status.conditions, cond);
                        }
                        ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(keys)) => {
                            tracing::warn!(
                                edgion_acme = %resource_ref.key(),
                                provider = %dns01.provider,
                                ?keys,
                                "DNS credential Secret missing required keys"
                            );
                            let cond = k8s_condition_false(
                                condition_types::RESOLVED_REFS,
                                condition_reasons::MISSING_KEYS,
                                format!(
                                    "DNS credential Secret for provider '{}' is missing required keys: {}",
                                    dns01.provider,
                                    keys.join(", ")
                                ),
                                generation,
                            );
                            update_k8s_condition(&mut status.conditions, cond);
                        }
                        ResolveOutcome::Invalid(InvalidCredentialError::DecodeError { key, detail }) => {
                            tracing::error!(
                                edgion_acme = %resource_ref.key(),
                                key = %key,
                                detail = %detail,
                                "DNS credential Secret value could not be decoded"
                            );
                            let cond = k8s_condition_false(
                                condition_types::RESOLVED_REFS,
                                condition_reasons::DECODE_ERROR,
                                format!("DNS credential decode failed for key '{}': {}", key, detail),
                                generation,
                            );
                            update_k8s_condition(&mut status.conditions, cond);
                        }
                        ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType { got, allowed }) => {
                            tracing::error!(
                                edgion_acme = %resource_ref.key(),
                                got = %got,
                                ?allowed,
                                "DNS credential Secret has unsupported type"
                            );
                            let cond = k8s_condition_false(
                                condition_types::RESOLVED_REFS,
                                condition_reasons::INVALID_SECRET_TYPE,
                                format!(
                                    "DNS credential Secret.type {:?} is not accepted (allowed: {:?})",
                                    got, allowed
                                ),
                                generation,
                            );
                            update_k8s_condition(&mut status.conditions, cond);
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
                        provider = %dns01.provider,
                        "Unsupported DNS provider — skipping DNS credential resolution (see Accepted condition for details)"
                    );
                }
            }
        }

        // Notify ACME service about the resource change (for certificate issuance/renewal)
        let ns = acme.metadata.namespace.as_deref().unwrap_or("default");
        let name = acme.metadata.name.as_deref().unwrap_or_default();
        notify_resource_changed(format!("{}/{}", ns, name));

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
            k8s_condition_true(
                condition_types::ACCEPTED,
                condition_reasons::ACCEPTED,
                "Resource accepted",
                generation,
            )
        } else {
            k8s_condition_false(
                condition_types::ACCEPTED,
                condition_reasons::INVALID,
                validation_errors.join("; "),
                generation,
            )
        };
        update_k8s_condition(&mut status.conditions, accepted);

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
// k8s_openapi::Condition helpers (EdgionAcmeStatus uses upstream Condition type)
// ============================================================================

fn k8s_condition_true(type_: &str, reason: &str, message: &str, observed_generation: Option<i64>) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "True".to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

fn k8s_condition_false(
    type_: &str,
    reason: &str,
    message: impl Into<String>,
    observed_generation: Option<i64>,
) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "False".to_string(),
        reason: reason.to_string(),
        message: message.into(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

fn k8s_condition_unknown(
    type_: &str,
    reason: &str,
    message: impl Into<String>,
    observed_generation: Option<i64>,
) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "Unknown".to_string(),
        reason: reason.to_string(),
        message: message.into(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

fn update_k8s_condition(conditions: &mut Vec<Condition>, new_condition: Condition) {
    if let Some(existing) = conditions.iter_mut().find(|c| c.type_ == new_condition.type_) {
        let status_changed = existing.status != new_condition.status;
        existing.status = new_condition.status;
        existing.reason = new_condition.reason;
        existing.message = new_condition.message;
        existing.observed_generation = new_condition.observed_generation;
        if status_changed {
            existing.last_transition_time = new_condition.last_transition_time;
        }
    } else {
        conditions.push(new_condition);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::SecretRefManager;
    use crate::core::controller::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::core::controller::conf_mgr::ControllerRuntime;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::common::SecretReference;
    use crate::types::resources::edgion_acme::AcmeChallengeConfig;
    use crate::types::resources::edgion_acme::AcmeStorageConfig;
    use crate::types::resources::edgion_acme::Dns01ChallengeConfig;
    use crate::types::resources::edgion_acme::Http01ChallengeConfig;
    use crate::types::resources::gateway::SecretObjectReference;
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
        let secret_ref_manager = Arc::new(SecretRefManager::new());
        let ctx = HandlerContext::new(
            secret_ref_manager,
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
                private_key_secret_ref: SecretReference {
                    name: secret_name.to_string(),
                    namespace: None,
                },
                domains: vec!["example.com".to_string()],
                key_type: Default::default(),
                challenge: AcmeChallengeConfig {
                    challenge_type: AcmeChallengeType::Http01,
                    http01: Some(Http01ChallengeConfig {
                        gateway_ref: ParentReference {
                            group: None,
                            kind: None,
                            namespace: Some("default".to_string()),
                            name: "gw".to_string(),
                            section_name: None,
                            port: None,
                        },
                    }),
                    dns01: None,
                },
                renewal: Default::default(),
                external_account_binding: None,
                storage: AcmeStorageConfig {
                    secret_name: "cert".to_string(),
                    secret_namespace: None,
                },
                auto_edgion_tls: Default::default(),
                active_challenges: None,
                dns_credential_secret: None,
                account_secret: None,
            },
            status: None,
        }
    }

    // ========================================================================
    // Test: Happy path — Secret present with valid accountCredentials
    // ========================================================================

    #[tokio::test]
    async fn test_account_credential_happy_path() {
        let (key, secret) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme("default", "my-acme", "acme-account");

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status should be set");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs condition should exist");
                assert_eq!(resolved.status, "True");
                assert!(
                    acme.spec.account_secret.is_some(),
                    "validated account Secret should be embedded on spec"
                );
            }
            ProcessResult::Skip { reason } => {
                panic!("expected Continue, got Skip: {reason}")
            }
        }
    }

    // ========================================================================
    // Test: Secret not found → NeedsRegistration (Unknown)
    // ========================================================================

    #[tokio::test]
    async fn test_secret_not_found_needs_registration() {
        let (ctx, _runtime) = make_ctx(HashMap::new());
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme("default", "my-acme", "missing-secret");

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status should be set");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs condition should exist");
                assert_eq!(resolved.status, "Unknown");
                assert_eq!(resolved.reason, "NeedsRegistration");

                // Verify SecretRefManager registration
                let refs = ctx.secret_ref_manager().get_refs("default/missing-secret");
                let owner = ResourceRef::new(
                    ResourceKind::EdgionAcme,
                    Some("default".to_string()),
                    "my-acme".to_string(),
                );
                assert!(refs.contains(&owner), "SecretRefManager should contain owner");
            }
            ProcessResult::Skip { reason } => {
                panic!("expected Continue, got Skip: {reason}")
            }
        }
    }

    // ========================================================================
    // Test: Secret exists but missing accountCredentials key → NeedsRegistration
    // ========================================================================

    #[tokio::test]
    async fn test_secret_missing_key_needs_registration() {
        let (key, secret) = make_secret("default", "acme-account", &[("wrongKey", b"some-value")]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme("default", "my-acme", "acme-account");

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status should be set");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs condition should exist");
                assert_eq!(resolved.status, "Unknown");
                assert_eq!(resolved.reason, "NeedsRegistration");
            }
            ProcessResult::Skip { reason } => {
                panic!("expected Continue, got Skip: {reason}")
            }
        }
    }

    // ========================================================================
    // Test: DecodeError (corrupt data) → Continue + ResolvedRefs=False
    //       (no account_secret embedded, no notify to AcmeService)
    // ========================================================================

    #[tokio::test]
    async fn test_corrupt_account_data_returns_continue_with_resolved_refs_false() {
        let (key, secret) = make_secret("default", "acme-account", &[("accountCredentials", b"not-valid-json")]);
        let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme("default", "my-acme", "acme-account");

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                assert!(
                    acme.spec.account_secret.is_none(),
                    "corrupted account must not be embedded on spec"
                );
                let status = acme.status.expect("status should be set");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs condition should exist");
                assert_eq!(resolved.status, "False");
                // M6/G: ACME now uses the generic `DecodeError` reason shared
                // with LinkSys / EdgionPlugins / EdgionTls instead of a
                // private `AcmeAccountCorrupted` bucket.
                assert_eq!(resolved.reason, "DecodeError");
                assert!(
                    resolved.message.contains("decode failed"),
                    "message should mention decode failure, got: {}",
                    resolved.message
                );
            }
            ProcessResult::Skip { reason } => {
                panic!("expected Continue so status persists, got Skip: {reason}")
            }
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

    // ========================================================================
    // DNS-01 credential pipeline tests (M8)
    // ========================================================================
    //
    // These cover the four `ResolveOutcome` paths the DNS resolve branch can
    // take — `SecretNotFound`, `ReferenceGrantMissing`, `MissingKeys`, and
    // the happy path. The account Secret is always present and valid so the
    // tests isolate the DNS branch's contribution to `ResolvedRefs`.

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
                private_key_secret_ref: SecretReference {
                    name: account_secret.to_string(),
                    namespace: None,
                },
                domains: vec!["*.example.com".to_string()],
                key_type: Default::default(),
                challenge: AcmeChallengeConfig {
                    challenge_type: AcmeChallengeType::Dns01,
                    http01: None,
                    dns01: Some(Dns01ChallengeConfig {
                        provider: dns_provider.to_string(),
                        credential_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: dns_secret_name.to_string(),
                            namespace: dns_secret_ns.map(|s| s.to_string()),
                        },
                        propagation_timeout: 120,
                        propagation_check_interval: 5,
                    }),
                },
                renewal: Default::default(),
                external_account_binding: None,
                storage: AcmeStorageConfig {
                    secret_name: "cert".to_string(),
                    secret_namespace: None,
                },
                auto_edgion_tls: Default::default(),
                active_challenges: None,
                dns_credential_secret: None,
                account_secret: None,
            },
            status: None,
        }
    }

    /// DNS Secret missing → `ResolvedRefs=False/SecretNotFound`.
    #[tokio::test]
    async fn test_dns_credential_secret_not_found() {
        // Account Secret present so the account branch writes `True`; DNS
        // branch must override with `False/SecretNotFound`.
        let (k, s) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
        let (ctx, _rt) = make_ctx(HashMap::from([(k, s)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "missing-dns", None);

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs");
                assert_eq!(resolved.status, "False");
                assert_eq!(resolved.reason, "SecretNotFound");
                assert!(
                    resolved.message.contains("DNS credential"),
                    "message should mention DNS credential, got: {}",
                    resolved.message
                );
                assert!(
                    acme.spec.dns_credential_secret.is_none(),
                    "spec.dns_credential_secret must be None when Secret missing"
                );
                // SecretRefManager must track the missing DNS Secret so the
                // resource re-reconciles when it arrives.
                let refs = ctx.secret_ref_manager().get_refs("default/missing-dns");
                let owner = ResourceRef::new(
                    ResourceKind::EdgionAcme,
                    Some("default".to_string()),
                    "my-acme".to_string(),
                );
                assert!(
                    refs.contains(&owner),
                    "SecretRefManager must track the missing DNS Secret"
                );
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    /// Cloudflare DNS Secret missing `api-token` → `ResolvedRefs=False/MissingKeys`.
    #[tokio::test]
    async fn test_dns_credential_missing_api_token() {
        let (k1, s1) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
        let (k2, s2) = make_secret("default", "cf-secret", &[("wrong-key", b"value")]);
        let (ctx, _rt) = make_ctx(HashMap::from([(k1, s1), (k2, s2)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "cf-secret", None);

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs");
                assert_eq!(resolved.status, "False");
                assert_eq!(resolved.reason, "MissingKeys");
                assert!(
                    resolved.message.contains("api-token"),
                    "message should name the missing key, got: {}",
                    resolved.message
                );
                assert!(
                    acme.spec.dns_credential_secret.is_none(),
                    "spec.dns_credential_secret must be None when Secret is invalid"
                );
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    /// AliDNS DNS Secret missing both keys → message names both.
    #[tokio::test]
    async fn test_dns_credential_alidns_missing_both_keys() {
        let (k1, s1) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
        let (k2, s2) = make_secret("default", "alidns-secret", &[("unrelated", b"v")]);
        let (ctx, _rt) = make_ctx(HashMap::from([(k1, s1), (k2, s2)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme_dns01("default", "my-acme", "acme-account", "alidns", "alidns-secret", None);

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs");
                assert_eq!(resolved.status, "False");
                assert_eq!(resolved.reason, "MissingKeys");
                assert!(resolved.message.contains("access-key-id"), "got {}", resolved.message);
                assert!(
                    resolved.message.contains("access-key-secret"),
                    "got {}",
                    resolved.message
                );
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    /// DNS Secret happy path + account happy path → account's `True` survives
    /// (DNS Ok is a no-op). `spec.dns_credential_secret` gets populated.
    #[tokio::test]
    async fn test_dns_credential_happy_path_preserves_true() {
        let (k1, s1) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
        let (k2, s2) = make_secret("default", "cf-secret", &[("api-token", b"cf-token-value")]);
        let (ctx, _rt) = make_ctx(HashMap::from([(k1, s1), (k2, s2)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "cf-secret", None);

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                assert!(
                    acme.spec.dns_credential_secret.is_some(),
                    "validated DNS credential Secret must be embedded on spec"
                );
                let status = acme.status.expect("status");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs");
                assert_eq!(resolved.status, "True");
                assert_eq!(resolved.reason, "ResolvedRefs");
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    /// DNS Ok but account pending (NeedsRegistration) → soft Unknown state
    /// survives (DNS Ok is a no-op, doesn't overwrite account's `Unknown`).
    #[tokio::test]
    async fn test_dns_credential_ok_preserves_account_needs_registration() {
        let (k2, s2) = make_secret("default", "cf-secret", &[("api-token", b"cf-token")]);
        // account Secret absent → Unknown(NeedsRegistration) from account branch.
        let (ctx, _rt) = make_ctx(HashMap::from([(k2, s2)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme_dns01("default", "my-acme", "missing-account", "cloudflare", "cf-secret", None);

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs");
                assert_eq!(
                    resolved.status, "Unknown",
                    "DNS success must not override account NeedsRegistration"
                );
                assert_eq!(resolved.reason, "NeedsRegistration");
                assert!(
                    acme.spec.dns_credential_secret.is_some(),
                    "DNS secret still populated on spec"
                );
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    /// Account pending + DNS missing → DNS `False` overrides account's
    /// soft `Unknown` (hard failure beats soft).
    #[tokio::test]
    async fn test_dns_credential_false_overrides_account_needs_registration() {
        // No account Secret, no DNS Secret.
        let (ctx, _rt) = make_ctx(HashMap::new());
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme_dns01(
            "default",
            "my-acme",
            "missing-account",
            "cloudflare",
            "missing-dns",
            None,
        );

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs");
                assert_eq!(resolved.status, "False");
                assert_eq!(resolved.reason, "SecretNotFound");
                assert!(
                    resolved.message.contains("DNS credential"),
                    "DNS error must be surfaced, got: {}",
                    resolved.message
                );
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    /// Cross-namespace DNS credential without a ReferenceGrant →
    /// `ResolvedRefs=False/RefNotPermitted`.
    #[tokio::test]
    async fn test_dns_credential_cross_namespace_denied() {
        let (k1, s1) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
        let (k2, s2) = make_secret("credentials", "cf-secret", &[("api-token", b"cf-token")]);
        let (ctx, _rt) = make_ctx(HashMap::from([(k1, s1), (k2, s2)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme_dns01(
            "default",
            "my-acme",
            "acme-account",
            "cloudflare",
            "cf-secret",
            Some("credentials"),
        );

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs");
                assert_eq!(resolved.status, "False");
                assert_eq!(resolved.reason, "RefNotPermitted");
                assert!(
                    resolved.message.contains("credentials"),
                    "cross-ns message should mention target ns, got: {}",
                    resolved.message
                );
                assert!(
                    acme.spec.dns_credential_secret.is_none(),
                    "spec.dns_credential_secret must stay None when grant missing"
                );
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }

    /// Unknown DNS provider → DNS resolve branch is a no-op (validate() is
    /// the canonical error path). Account's `True` must survive.
    #[tokio::test]
    async fn test_dns_credential_unknown_provider_skips_dns_resolve() {
        let (k, s) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
        let (ctx, _rt) = make_ctx(HashMap::from([(k, s)]));
        let handler = EdgionAcmeHandler::new();
        let acme = make_acme_dns01(
            "default",
            "my-acme",
            "acme-account",
            "route53-not-supported",
            "any-secret",
            None,
        );

        let result = handler.parse(acme, &ctx).await;

        match result {
            ProcessResult::Continue(acme) => {
                let status = acme.status.expect("status");
                let resolved = status
                    .conditions
                    .iter()
                    .find(|c| c.type_ == "ResolvedRefs")
                    .expect("ResolvedRefs");
                // account = True, DNS resolve skipped → condition unchanged.
                assert_eq!(resolved.status, "True");
                assert!(
                    acme.spec.dns_credential_secret.is_none(),
                    "no Secret attached for unknown provider"
                );
            }
            ProcessResult::Skip { reason } => panic!("expected Continue, got Skip: {reason}"),
        }
    }
}
