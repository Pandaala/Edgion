//! EdgionAcme Handler (Controller side)
//!
//! Handles EdgionAcme resources with:
//! - Validation (email, domains, challenge config, etc.)
//! - DNS credential Secret reference resolution
//! - SecretRefManager registration for cascading updates
//! - Status management
//! - Notifying AcmeService for background certificate issuance/renewal

use crate::core::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, get_secret, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::core::services::acme::notify_resource_changed;
use crate::types::prelude_resources::EdgionAcme;
use crate::types::resources::edgion_acme::{AcmeChallengeType, EdgionAcmeStatus};
use crate::types::ResourceKind;

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

impl ProcessorHandler<EdgionAcme> for EdgionAcmeHandler {
    fn validate(&self, acme: &EdgionAcme, _ctx: &HandlerContext) -> Vec<String> {
        let mut warnings = Vec::new();

        // Validate email
        if acme.spec.email.is_empty() {
            warnings.push("EdgionAcme: email is required".to_string());
        }

        // Validate domains
        if acme.spec.domains.is_empty() {
            warnings.push("EdgionAcme: at least one domain is required".to_string());
        }

        // Validate wildcard domains require DNS-01
        if acme.has_wildcard_domains() && acme.spec.challenge.challenge_type != AcmeChallengeType::Dns01 {
            warnings.push("EdgionAcme: wildcard domains (*.example.com) require dns-01 challenge type".to_string());
        }

        // Validate challenge config completeness
        match acme.spec.challenge.challenge_type {
            AcmeChallengeType::Http01 => {
                if acme.spec.challenge.http01.is_none() {
                    warnings.push("EdgionAcme: http01 config is required when challenge type is http-01".to_string());
                }
            }
            AcmeChallengeType::Dns01 => {
                if acme.spec.challenge.dns01.is_none() {
                    warnings.push("EdgionAcme: dns01 config is required when challenge type is dns-01".to_string());
                } else {
                    let dns01 = acme.spec.challenge.dns01.as_ref().unwrap();
                    // Validate provider name
                    match dns01.provider.as_str() {
                        "cloudflare" | "alidns" => {}
                        other => {
                            warnings.push(format!(
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

                    if get_secret(secret_ns.map(|s| s.as_str()), &dns01.credential_ref.name).is_none() {
                        let secret_key = format_secret_key(secret_ns, &dns01.credential_ref.name);
                        warnings.push(format!(
                            "EdgionAcme: DNS credential Secret '{}' not found (may arrive later)",
                            secret_key
                        ));
                    }
                }
            }
        }

        warnings
    }

    fn parse(&self, mut acme: EdgionAcme, ctx: &HandlerContext) -> ProcessResult<EdgionAcme> {
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
            if let Some(secret) = get_secret(secret_ns.map(|s| s.as_str()), &dns01.credential_ref.name) {
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

    fn on_delete(&self, acme: &EdgionAcme, ctx: &HandlerContext) {
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
        // Initialize status if not present
        let status = acme.status.get_or_insert_with(EdgionAcmeStatus::default);

        // If there are validation errors, set phase to Failed
        if !validation_errors.is_empty() {
            status.last_failure_reason = Some(validation_errors.join("; "));
        }
    }
}
