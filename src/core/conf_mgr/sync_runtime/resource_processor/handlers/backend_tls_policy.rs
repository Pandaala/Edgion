//! BackendTLSPolicy Handler
//!
//! Handles BackendTLSPolicy resources with Gateway API standard status management.

use crate::core::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_reasons, condition_true, condition_types, format_secret_key, get_secret,
    update_condition, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::types::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use crate::types::prelude_resources::BackendTLSPolicy;
use crate::types::resources::backend_tls_policy::{
    BackendTLSPolicyCACertificateRef, BackendTLSPolicyStatus, PolicyAncestorStatus,
};
use crate::types::resources::common::{is_core_api_group, ParentReference};
use crate::types::ResourceKind;

/// BackendTLSPolicy handler
pub struct BackendTlsPolicyHandler {
    controller_name: String,
}

impl BackendTlsPolicyHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }
}

impl Default for BackendTlsPolicyHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

impl ProcessorHandler<BackendTLSPolicy> for BackendTlsPolicyHandler {
    fn parse(&self, mut btp: BackendTLSPolicy, ctx: &HandlerContext) -> ProcessResult<BackendTLSPolicy> {
        let resource_ref = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            btp.metadata.namespace.clone(),
            btp.metadata.name.clone().unwrap_or_default(),
        );
        // Clear old references first to handle updates safely.
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        btp.spec.resolved_ca_certificates = None;
        btp.spec.resolved_client_certificate = None;

        let policy_ns = btp.metadata.namespace.as_ref();
        let mut resolved_ca_secrets = Vec::new();

        if let Some(refs) = &btp.spec.validation.ca_certificate_refs {
            for ca_ref in refs {
                if !Self::is_secret_ref(ca_ref) {
                    continue;
                }

                let secret_key = format_secret_key(policy_ns, &ca_ref.name);
                ctx.secret_ref_manager()
                    .add_ref(secret_key.clone(), resource_ref.clone());

                if let Some(secret) = get_secret(policy_ns.map(|s| s.as_str()), &ca_ref.name) {
                    resolved_ca_secrets.push(secret);
                } else {
                    tracing::info!(
                        policy = %resource_ref.key(),
                        secret_key = %secret_key,
                        "CA Secret not found yet, BackendTLSPolicy will be reprocessed when Secret arrives"
                    );
                }
            }
        }

        if !resolved_ca_secrets.is_empty() {
            btp.spec.resolved_ca_certificates = Some(resolved_ca_secrets);
        }

        match btp.client_certificate_secret_ref() {
            Ok(Some(client_ref)) => {
                let client_ns = client_ref.namespace.as_ref().or(policy_ns);
                let secret_key = format_secret_key(client_ns, &client_ref.name);
                ctx.secret_ref_manager()
                    .add_ref(secret_key.clone(), resource_ref.clone());

                if let Some(secret) = get_secret(client_ns.map(|s| s.as_str()), &client_ref.name) {
                    btp.spec.resolved_client_certificate = Some(secret);
                } else {
                    tracing::info!(
                        policy = %resource_ref.key(),
                        secret_key = %secret_key,
                        "Client certificate Secret not found yet, BackendTLSPolicy will be reprocessed when Secret arrives"
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    policy = %resource_ref.key(),
                    err = %err,
                    "Invalid BackendTLSPolicy client certificate option"
                );
            }
        }

        ProcessResult::Continue(btp)
    }

    fn on_delete(&self, btp: &BackendTLSPolicy, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::BackendTLSPolicy,
            btp.metadata.namespace.clone(),
            btp.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        tracing::debug!(
            policy = %resource_ref.key(),
            "Cleared secret references on BackendTLSPolicy delete"
        );
    }

    fn update_status(&self, btp: &mut BackendTLSPolicy, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = btp.metadata.generation;

        let mut resolved_ref_errors = Vec::new();
        resolved_ref_errors.extend_from_slice(validation_errors);

        if let Some(refs) = &btp.spec.validation.ca_certificate_refs {
            for ca_ref in refs {
                if !Self::is_secret_ref(ca_ref) {
                    resolved_ref_errors.push(format!(
                        "Unsupported caCertificateRef kind/group: {}/{}",
                        ca_ref.group, ca_ref.kind
                    ));
                    continue;
                }
                if !Self::is_ca_secret_resolved(btp, &ca_ref.name) {
                    resolved_ref_errors.push(format!("CA Secret '{}' not found", ca_ref.name));
                }
            }
        }

        match btp.client_certificate_secret_ref() {
            Ok(Some(client_ref)) => {
                if btp.spec.resolved_client_certificate.is_none() {
                    resolved_ref_errors.push(format!("Client certificate Secret '{}' not found", client_ref.name));
                } else if let Some(secret) = &btp.spec.resolved_client_certificate {
                    if !Self::secret_has_client_cert_pair(secret) {
                        resolved_ref_errors.push(format!(
                            "Client certificate Secret '{}' must contain '{}' and '{}'",
                            client_ref.name, CERT, KEY
                        ));
                    }
                }
            }
            Ok(None) => {}
            Err(err) => resolved_ref_errors.push(err),
        }

        // Initialize status if not present
        let status = btp
            .status
            .get_or_insert_with(|| BackendTLSPolicyStatus { ancestors: vec![] });

        // BackendTLSPolicy targets Services (via target_refs), but ancestors refer to Gateways.
        // For now, we create a synthetic ancestor representing this policy.
        let synthetic_ancestor = ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("BackendTLSPolicy".to_string()),
            namespace: btp.metadata.namespace.clone(),
            name: btp.metadata.name.clone().unwrap_or_default(),
            section_name: None,
            port: None,
        };

        // Find or create ancestor status
        let ancestor_status = status.ancestors.iter_mut().find(|as_| {
            as_.ancestor_ref.name == synthetic_ancestor.name
                && as_.ancestor_ref.namespace == synthetic_ancestor.namespace
        });

        let conditions = if let Some(as_) = ancestor_status {
            &mut as_.conditions
        } else {
            status.ancestors.push(PolicyAncestorStatus {
                ancestor_ref: synthetic_ancestor,
                controller_name: self.controller_name.clone(),
                conditions: Vec::new(),
            });
            &mut status.ancestors.last_mut().expect("just inserted").conditions
        };

        if validation_errors.is_empty() {
            update_condition(
                conditions,
                condition_true(
                    condition_types::ACCEPTED,
                    condition_reasons::ACCEPTED,
                    "Policy accepted",
                    generation,
                ),
            );
        } else {
            update_condition(
                conditions,
                condition_false(
                    condition_types::ACCEPTED,
                    condition_reasons::INVALID_KIND,
                    validation_errors.join("; "),
                    generation,
                ),
            );
        }

        if resolved_ref_errors.is_empty() {
            update_condition(
                conditions,
                condition_true(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::RESOLVED_REFS,
                    "All references resolved",
                    generation,
                ),
            );
        } else {
            update_condition(
                conditions,
                condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::REF_NOT_PERMITTED,
                    resolved_ref_errors.join("; "),
                    generation,
                ),
            );
        }

        update_condition(
            conditions,
            condition_true(
                condition_types::PROGRAMMED,
                condition_reasons::PROGRAMMED,
                "Configuration programmed",
                generation,
            ),
        );
        update_condition(
            conditions,
            condition_true(
                condition_types::READY,
                condition_reasons::READY,
                "Policy is ready",
                generation,
            ),
        );
    }
}

impl BackendTlsPolicyHandler {
    #[inline]
    fn is_secret_ref(ca_ref: &BackendTLSPolicyCACertificateRef) -> bool {
        is_core_api_group(&ca_ref.group) && ca_ref.kind == "Secret"
    }

    #[inline]
    fn is_ca_secret_resolved(btp: &BackendTLSPolicy, name: &str) -> bool {
        btp.spec
            .resolved_ca_certificates
            .as_ref()
            .is_some_and(|secrets| secrets.iter().any(|s| s.metadata.name.as_deref() == Some(name)))
    }

    #[inline]
    fn secret_has_client_cert_pair(secret: &k8s_openapi::api::core::v1::Secret) -> bool {
        secret
            .data
            .as_ref()
            .is_some_and(|d| d.contains_key(CERT) && d.contains_key(KEY))
    }

    #[allow(dead_code)]
    #[inline]
    fn secret_has_ca_cert(secret: &k8s_openapi::api::core::v1::Secret) -> bool {
        secret.data.as_ref().is_some_and(|d| d.contains_key(CA_CERT))
    }
}
