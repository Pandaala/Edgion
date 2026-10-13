//! EdgionGatewayConfig Handler
//!
//! Handles EdgionGatewayConfig resources with Gateway API standard status management.

use crate::conf_mgr::sync_runtime::resource_processor::credential::egc_tls;
use crate::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, update_condition, HandlerContext, ProcessResult,
    ProcessorHandler, ResourceRef,
};
use crate::config::is_reference_grant_validation_enabled;
use edgion_resources::prelude_resources::EdgionGatewayConfig;
use edgion_resources::refs::condition_fields::PreparedConditionReferences;
use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfigStatus;
use edgion_resources::resources::refs::egc_tls_fields::PreparedEgcTlsReferences;
use edgion_resources::resources::refs::plugin_attachment_fields::HttpPluginAttachments;
use edgion_resources::ResourceKind;

use super::san_guard;

/// Requeue every GatewayClass so its ResolvedRefs/Accepted conditions are recomputed after an
/// EdgionGatewayConfig change or delete. GatewayClass cardinality is tiny, so a brute-force
/// requeue-all is cheap and avoids a dedicated bidirectional ref-tracking subsystem. `ctx.requeue`
/// carries the trigger chain (cycle/depth safe); GatewayClass may then requeue its Gateways.
async fn requeue_all_gateway_classes(ctx: &HandlerContext) {
    let Some(gc_processor) = ctx.runtime().processor_registry.get("GatewayClass") else {
        return;
    };
    for key in gc_processor.list_keys() {
        ctx.requeue("GatewayClass", key).await;
    }
}

/// EdgionGatewayConfig handler
pub struct EdgionGatewayConfigHandler;

impl EdgionGatewayConfigHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionGatewayConfigHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionGatewayConfig> for EdgionGatewayConfigHandler {
    fn preparse(&self, egc: &mut EdgionGatewayConfig, _ctx: &HandlerContext) -> Vec<String> {
        // Phase 1 guard: reject outbound TLS subjectAltNames/hostname on
        // EdgionGatewayConfig where SAN pinning enforcement is not yet implemented.
        let mut errors = san_guard::check_egc_san(egc);
        // dnsResolver `linkSysRef` and `servers` are mutually exclusive (the
        // structural CRD cannot express `oneOf`, so it is enforced here).
        errors.extend(egc.spec.validate_dns_resolver());
        errors.extend(egc.spec.validate_link_sys());
        errors.extend(egc.spec.validate_request_body());
        errors.extend(egc.spec.validate_retry());
        errors.extend(egc.spec.validate_forwarded_headers());
        errors.extend(egc.spec.validate_real_ip());
        let trust = PreparedConditionReferences::from_gateway_config(egc);
        if let Err(error) = trust.visit_gateway_ip(egc, |leaf| match leaf.references() {
            Ok(tokens) => {
                for (index, token) in tokens.enumerate() {
                    if let Err(error) = token.validated() {
                        errors.push(format!("realIp.trustedIpRefs[{index}]: {error}"));
                    }
                }
            }
            Err(error) => errors.push(format!("realIp: {error}")),
        }) {
            errors.push(format!("realIp: {error}"));
        }
        errors.extend(egc.spec.validate_preflight_policy());
        errors.extend(egc.spec.validate_plugin_policy());
        errors.extend(egc_tls::identity_errors(egc));
        errors.extend(
            HttpPluginAttachments::EgcGlobals(egc)
                .entries()
                .into_iter()
                .enumerate()
                .filter_map(|(index, reference)| {
                    reference
                        .err()
                        .map(|error| format!("globalPluginsRef[{index}]: {error}"))
                }),
        );
        // Cluster-scoped status must not depend on a deployment's namespace
        // visibility. Validate referenced early plugins on the consuming class.
        errors
    }

    async fn parse(&self, mut egc: EdgionGatewayConfig, ctx: &HandlerContext) -> ProcessResult<EdgionGatewayConfig> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionGatewayConfig,
            None,
            egc.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_value_refs(&resource_ref);
        let owner_key = format!("{}:{}", resource_ref.kind_str(), resource_ref.resource_key());
        ctx.runtime().edgion_config_data_refs.clear_refs_from(&owner_key);
        let trust = PreparedConditionReferences::from_gateway_config(&egc);
        let _ = trust.clear_gateway_ip_scopes(&mut egc);
        let _ = trust.visit_gateway_ip_mut(&mut egc, |mut leaf| {
            let mut scopes = Vec::new();
            let markers = match leaf.references() {
                Ok(tokens) => tokens
                    .map(|token| {
                        if let Ok(bound) = super::ip_list_refs::validated_ip_reference(&token) {
                            ctx.runtime().edgion_config_data_refs.add_ref(
                                format!(
                                    "{}/{}",
                                    bound.target().scope().namespace().unwrap_or_default(),
                                    bound.target().name()
                                ),
                                owner_key.clone(),
                            );
                        }
                        let outcome = super::ip_list_refs::resolve_ip_reference(
                            token,
                            is_reference_grant_validation_enabled(),
                            |namespace, name| ctx.runtime().edgion_config_data_store.get(Some(namespace), name),
                            |_| unreachable!("cluster RealIp cannot query a namespaced ReferenceGrant"),
                        );
                        scopes.push(outcome.read_scope);
                        outcome.denied
                    })
                    .collect::<Vec<_>>(),
                Err(_) => return,
            };
            for (reference, marker) in leaf.config_mut().ip_refs.iter_mut().zip(markers) {
                reference.ref_denied = marker;
            }
            let _ = leaf.set_read_scopes(&scopes);
        });
        if let Some(field) = egc.spec.outbound_tls.as_mut() {
            field.resolved_ca_certificates = None;
            field.resolved_client_certificate = None;
        }
        let prepared = PreparedEgcTlsReferences::from_config(&egc);
        if prepared.bind(&egc).is_err() {
            return ProcessResult::Continue(egc);
        }
        for reference in HttpPluginAttachments::EgcGlobals(&egc).entries().into_iter().flatten() {
            ctx.runtime()
                .plugin_ref_mgr
                .add_ref(reference.into_key(), resource_ref.clone());
        }

        if let Ok(mut binding) = prepared.bind_mut(&mut egc) {
            let material = binding
                .references()
                .map(|references| egc_tls::resolve(ctx, &resource_ref, references));
            if let Ok(material) = material {
                if let Some(field) = binding.outbound_tls_mut().as_mut() {
                    field.resolved_ca_certificates = material.ca_certificates;
                    field.resolved_client_certificate = material.client_certificate;
                }
            }
        }

        ProcessResult::Continue(egc)
    }

    async fn on_change(&self, _egc: &EdgionGatewayConfig, ctx: &HandlerContext) {
        // A GatewayClass's ResolvedRefs/Accepted conditions depend on whether the
        // EdgionGatewayConfig it references exists, so recompute them on every EGC change.
        requeue_all_gateway_classes(ctx).await;
    }

    async fn on_delete(&self, egc: &EdgionGatewayConfig, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionGatewayConfig,
            None,
            egc.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().edgion_config_data_refs.clear_refs_from(&format!(
            "{}:{}",
            resource_ref.kind_str(),
            resource_ref.resource_key()
        ));
        // A GatewayClass's ResolvedRefs/Accepted conditions depend on whether the
        // EdgionGatewayConfig it references exists, so recompute them on every EGC delete.
        requeue_all_gateway_classes(ctx).await;
    }

    fn update_status(&self, egc: &mut EdgionGatewayConfig, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = egc.metadata.generation;

        let status = egc
            .status
            .get_or_insert_with(|| EdgionGatewayConfigStatus { conditions: vec![] });

        if validation_errors.is_empty() {
            update_condition(&mut status.conditions, accepted_condition(generation));
        } else {
            update_condition(
                &mut status.conditions,
                condition_false(
                    condition_types::ACCEPTED,
                    "Invalid",
                    validation_errors.join("; "),
                    generation,
                ),
            );
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config_test.rs"]
mod tests;
