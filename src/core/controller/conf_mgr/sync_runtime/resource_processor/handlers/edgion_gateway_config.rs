//! EdgionGatewayConfig Handler
//!
//! Handles EdgionGatewayConfig resources with Gateway API standard status management.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, update_condition, HandlerContext, ProcessResult,
    ProcessorHandler,
};
use crate::types::prelude_resources::EdgionGatewayConfig;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfigStatus;

/// Requeue every GatewayClass so its ResolvedRefs/Accepted conditions are recomputed after an
/// EdgionGatewayConfig change or delete. GatewayClass cardinality is tiny, so a brute-force
/// requeue-all is cheap and avoids a dedicated bidirectional ref-tracking subsystem. `ctx.requeue`
/// carries the trigger chain (cycle/depth safe); GatewayClass requeues nothing, so the edge is
/// terminal.
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
    async fn parse(&self, egc: EdgionGatewayConfig, _ctx: &HandlerContext) -> ProcessResult<EdgionGatewayConfig> {
        egc.spec.check_deprecated_fields();
        ProcessResult::Continue(egc)
    }

    async fn on_change(&self, _egc: &EdgionGatewayConfig, ctx: &HandlerContext) {
        // A GatewayClass's ResolvedRefs/Accepted conditions depend on whether the
        // EdgionGatewayConfig it references exists, so recompute them on every EGC change.
        requeue_all_gateway_classes(ctx).await;
    }

    async fn on_delete(&self, _egc: &EdgionGatewayConfig, ctx: &HandlerContext) {
        requeue_all_gateway_classes(ctx).await;
    }

    fn update_status(&self, egc: &mut EdgionGatewayConfig, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = egc.metadata.generation;

        // Initialize status if not present
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
