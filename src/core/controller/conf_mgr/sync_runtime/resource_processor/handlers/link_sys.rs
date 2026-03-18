//! LinkSys Handler
//!
//! Handles LinkSys resources with Gateway API standard status management.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, update_condition, HandlerContext, ProcessResult,
    ProcessorHandler,
};
use crate::types::prelude_resources::LinkSys;
use crate::types::resources::link_sys::LinkSysStatus;

/// LinkSys handler
pub struct LinkSysHandler;

impl LinkSysHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinkSysHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<LinkSys> for LinkSysHandler {
    async fn parse(&self, ls: LinkSys, _ctx: &HandlerContext) -> ProcessResult<LinkSys> {
        ProcessResult::Continue(ls)
    }

    fn update_status(&self, ls: &mut LinkSys, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ls.metadata.generation;

        // Initialize status if not present
        let status = ls.status.get_or_insert_with(|| LinkSysStatus { conditions: vec![] });

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
