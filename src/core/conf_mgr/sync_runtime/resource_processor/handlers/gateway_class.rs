//! GatewayClass Handler
//!
//! Handles GatewayClass resources.

use crate::core::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_true, condition_types, update_condition, HandlerContext,
    ProcessResult, ProcessorHandler,
};
use crate::types::prelude_resources::GatewayClass;
use crate::types::resources::gateway_class::GatewayClassStatus;

/// GatewayClass handler
pub struct GatewayClassHandler;

impl GatewayClassHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GatewayClassHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<GatewayClass> for GatewayClassHandler {
    fn parse(&self, gc: GatewayClass, _ctx: &HandlerContext) -> ProcessResult<GatewayClass> {
        ProcessResult::Continue(gc)
    }

    fn update_status(&self, gc: &mut GatewayClass, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = gc.metadata.generation;
        let status = gc.status.get_or_insert_with(GatewayClassStatus::default);

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

        update_condition(
            &mut status.conditions,
            condition_true(
                "SupportedVersion",
                "SupportedVersion",
                "Gateway API version is supported",
                generation,
            ),
        );
    }
}
