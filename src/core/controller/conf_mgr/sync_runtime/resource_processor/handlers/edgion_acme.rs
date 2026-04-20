//! EdgionAcme Handler (Controller side)
//!
//! Handles EdgionAcme resources with:
//! - Validation (email, domains, challenge config, etc.)
//! - DNS credential Secret reference resolution
//! - SecretRefManager registration for cascading updates
//! - Status management
//! - Notifying AcmeService for background certificate issuance/renewal

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_reasons, condition_types,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::core::controller::services::acme::notify_resource_changed;
use crate::types::prelude_resources::EdgionAcme;
use crate::types::resources::edgion_acme::{AcmeChallengeType, EdgionAcmeStatus};
use crate::types::ResourceKind;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use k8s_openapi::chrono::Utc;

/// EdgionAcme handler
///
/// Features:
/// - validate: Check email, domains, challenge config consistency
/// - parse: Resolve DNS credential Secret reference
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

#[async_trait::async_trait]
impl ProcessorHandler<EdgionAcme> for EdgionAcmeHandler {
    fn validate(&self, acme: &EdgionAcme, ctx: &HandlerContext) -> Vec<String> {
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

                    // Check DNS credential Secret existence
                    let secret_ns = dns01
                        .credential_ref
                        .namespace
                        .as_ref()
                        .or(acme.metadata.namespace.as_ref());

                    if ctx
                        .runtime()
                        .secret_store
                        .get(secret_ns.map(|s| s.as_str()), &dns01.credential_ref.name)
                        .is_none()
                    {
                        let secret_key = format_secret_key(secret_ns, &dns01.credential_ref.name);
                        errors.push(format!(
                            "EdgionAcme: DNS credential Secret '{}' not found (may arrive later)",
                            secret_key
                        ));
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

        // Resolve DNS credential Secret (if DNS-01)
        if let Some(ref dns01) = acme.spec.challenge.dns01 {
            let secret_ns = dns01
                .credential_ref
                .namespace
                .as_ref()
                .or(acme.metadata.namespace.as_ref());
            let secret_key = format_secret_key(secret_ns, &dns01.credential_ref.name);

            // Register reference for cascading updates
            ctx.secret_ref_manager()
                .add_ref(secret_key.clone(), resource_ref.clone());

            // Try to resolve Secret
            if let Some(secret) = ctx
                .runtime()
                .secret_store
                .get(secret_ns.map(|s| s.as_str()), &dns01.credential_ref.name)
            {
                acme.spec.dns_credential_secret = Some(secret);
                tracing::debug!(
                    edgion_acme = %resource_ref.key(),
                    secret_key = %secret_key,
                    "DNS credential Secret resolved"
                );
            } else {
                tracing::info!(
                    edgion_acme = %resource_ref.key(),
                    secret_key = %secret_key,
                    "DNS credential Secret not found yet, will be reprocessed when Secret arrives"
                );
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
                &validation_errors.join("; "),
                generation,
            )
        };
        update_k8s_condition(&mut status.conditions, accepted);

        // Retain `last_failure_reason` text field for backward-compatibility with
        // existing tooling that reads it directly (e.g., dashboards). The canonical
        // status is now the Accepted condition above.
        if !validation_errors.is_empty() {
            status.last_failure_reason = Some(validation_errors.join("; "));
        }
    }
}

/// Create a k8s_openapi Condition with True status
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

/// Create a k8s_openapi Condition with False status
fn k8s_condition_false(type_: &str, reason: &str, message: &str, observed_generation: Option<i64>) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "False".to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

/// Update or insert a k8s_openapi Condition (only bump `last_transition_time` on status flip).
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
