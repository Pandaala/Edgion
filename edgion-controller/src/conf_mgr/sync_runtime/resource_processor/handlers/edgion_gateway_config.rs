//! EdgionGatewayConfig Handler
//!
//! Handles EdgionGatewayConfig resources with Gateway API standard status management.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{
    fetch_secret, resolve_ca_certificate_refs, CaFetchStrategy, FetchOutcome,
};
use crate::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, update_condition, HandlerContext, ProcessResult,
    ProcessorHandler, ResourceRef,
};
use edgion_resources::prelude_resources::EdgionGatewayConfig;
use edgion_resources::resources::common::is_core_api_group;
use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfigStatus;
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
        errors.extend(egc.spec.validate_preflight_policy());
        errors.extend(egc.spec.validate_plugin_policy());
        // Cluster-scoped status must not depend on a deployment's namespace
        // visibility. Validate referenced early plugins on the consuming class.
        errors
    }

    async fn parse(&self, mut egc: EdgionGatewayConfig, ctx: &HandlerContext) -> ProcessResult<EdgionGatewayConfig> {
        // Resolve the cluster-wide outbound CA Secret (validation.caCertificateRefs) and
        // register the reference so a Secret change requeues this EGC. EGC is
        // cluster-scoped, so `metadata.namespace` is None; the referenced Secret
        // SHOULD therefore carry an explicit `namespace` in `validation.caCertificateRefs[]`.
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionGatewayConfig,
            egc.metadata.namespace.clone(),
            egc.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_value_refs(&resource_ref);
        for reference in egc.spec.global_plugins_ref.as_deref().unwrap_or_default() {
            let namespace = reference.namespace.as_deref().unwrap_or("default");
            ctx.runtime()
                .plugin_ref_mgr
                .add_ref(format!("{namespace}/{}", reference.name), resource_ref.clone());
        }

        if let Some(ref mut ob) = egc.spec.outbound_tls {
            // Replace both runtime fields on every generation, including missing or denied refs.
            ob.resolved_ca_certificates = None;
            ob.resolved_client_certificate = None;

            // Like global CA refs, a cluster-scoped identity needs an explicit namespace.
            // fetch_secret registers dependencies before checking grants or material presence.
            if let Some(reference) = ob.client_certificate_ref.as_ref().filter(|reference| {
                reference
                    .namespace
                    .as_deref()
                    .is_some_and(|namespace| !namespace.is_empty())
                    && reference.kind.as_deref().unwrap_or("Secret") == "Secret"
                    && is_core_api_group(reference.group.as_deref().unwrap_or(""))
            }) {
                if let FetchOutcome::Ok(secret) =
                    fetch_secret(ctx, &resource_ref, "edgion.io", "EdgionGatewayConfig", reference)
                {
                    ob.resolved_client_certificate = Some(secret);
                }
            }

            // Resolve cluster-wide outbound CA certificate refs.
            // EGC is cluster-scoped (owner_ns = None); each ref must carry an explicit
            // namespace or it is skipped as unresolvable by the helper.
            let refs = ob
                .validation
                .as_ref()
                .and_then(|v| v.ca_certificate_refs.as_deref())
                .unwrap_or_default();
            if !refs.is_empty() {
                let resolved = resolve_ca_certificate_refs(
                    ctx,
                    &resource_ref,
                    None, // cluster-scoped: no owner namespace
                    refs,
                    CaFetchStrategy::GrantGated {
                        group: "edgion.io",
                        kind: "EdgionGatewayConfig",
                    },
                );
                ob.resolved_ca_certificates = (!resolved.is_empty()).then_some(resolved);
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
            egc.metadata.namespace.clone(),
            egc.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_value_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_value_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_value_refs(&resource_ref);
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
