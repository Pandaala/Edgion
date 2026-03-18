//! EdgionStreamPlugins Handler
//!
//! Handles EdgionStreamPlugins resources with Gateway API standard status management.

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    condition_types, HandlerContext, ProcessResult, ProcessorHandler,
};
use crate::types::prelude_resources::EdgionStreamPlugins;
use crate::types::resources::edgion_stream_plugins::EdgionStreamPluginsStatus;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use k8s_openapi::chrono::Utc;

/// EdgionStreamPlugins handler
pub struct EdgionStreamPluginsHandler;

impl EdgionStreamPluginsHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionStreamPluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<EdgionStreamPlugins> for EdgionStreamPluginsHandler {
    fn parse(&self, esp: EdgionStreamPlugins, _ctx: &HandlerContext) -> ProcessResult<EdgionStreamPlugins> {
        ProcessResult::Continue(esp)
    }

    fn update_status(&self, esp: &mut EdgionStreamPlugins, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = esp.metadata.generation;

        // Initialize status if not present
        let status = esp
            .status
            .get_or_insert_with(|| EdgionStreamPluginsStatus { conditions: vec![] });

        // Set Accepted condition
        let accepted = if validation_errors.is_empty() {
            k8s_condition_true(condition_types::ACCEPTED, "Accepted", "Resource accepted", generation)
        } else {
            k8s_condition_false(
                condition_types::ACCEPTED,
                "Invalid",
                &validation_errors.join("; "),
                generation,
            )
        };
        update_k8s_condition(&mut status.conditions, accepted);
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

/// Update or insert a k8s_openapi Condition
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
