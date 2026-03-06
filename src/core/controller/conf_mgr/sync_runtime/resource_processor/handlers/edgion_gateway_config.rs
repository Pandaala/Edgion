//! EdgionGatewayConfig Handler
//!
//! Handles EdgionGatewayConfig resources with Gateway API standard status management.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, ready_condition, update_condition, HandlerContext,
    ProcessResult, ProcessorHandler,
};
use crate::types::prelude_resources::EdgionGatewayConfig;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfigStatus;

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

impl ProcessorHandler<EdgionGatewayConfig> for EdgionGatewayConfigHandler {
    fn parse(&self, egc: EdgionGatewayConfig, _ctx: &HandlerContext) -> ProcessResult<EdgionGatewayConfig> {
        ProcessResult::Continue(egc)
    }

    fn update_status(&self, egc: &mut EdgionGatewayConfig, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = egc.metadata.generation;

        // Initialize status if not present
        let status = egc
            .status
            .get_or_insert_with(|| EdgionGatewayConfigStatus { conditions: vec![] });

        // Set Accepted condition
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

        // Set Ready condition (always ready after parsing)
        update_condition(&mut status.conditions, ready_condition(generation));
    }
}
