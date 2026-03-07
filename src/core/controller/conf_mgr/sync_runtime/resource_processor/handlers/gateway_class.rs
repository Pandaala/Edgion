//! GatewayClass Handler
//!
//! Handles GatewayClass resources.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_false, condition_true, condition_types, update_condition, HandlerContext, ProcessResult, ProcessorHandler,
};
use crate::types::prelude_resources::GatewayClass;
use crate::types::resources::gateway_class::GatewayClassStatus;

/// GatewayClass handler
pub struct GatewayClassHandler {
    controller_name: String,
}

impl GatewayClassHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }
}

impl Default for GatewayClassHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

impl ProcessorHandler<GatewayClass> for GatewayClassHandler {
    fn filter(&self, gc: &GatewayClass) -> bool {
        gc.spec.controller_name == self.controller_name
    }

    fn parse(&self, gc: GatewayClass, _ctx: &HandlerContext) -> ProcessResult<GatewayClass> {
        ProcessResult::Continue(gc)
    }

    fn update_status(&self, gc: &mut GatewayClass, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = gc.metadata.generation;
        let status = gc.status.get_or_insert_with(GatewayClassStatus::default);

        if validation_errors.is_empty() {
            update_condition(
                &mut status.conditions,
                condition_true("Accepted", "Accepted", "GatewayClass accepted", generation),
            );
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
