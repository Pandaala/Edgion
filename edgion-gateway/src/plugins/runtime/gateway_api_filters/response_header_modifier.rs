//! ResponseHeaderModifier filter implementation

use super::header_modifier::HeaderModifierPlan;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::{GRPCHeaderFilter, HTTPHeaderFilter};

pub struct ResponseHeaderModifierFilter {
    plan: HeaderModifierPlan,
}

impl ResponseHeaderModifierFilter {
    pub fn new(config: HTTPHeaderFilter) -> Result<Self, &'static str> {
        config.validate_response()?;
        Ok(Self {
            plan: HeaderModifierPlan::from_http(config)?,
        })
    }

    /// Create from GRPCHeaderFilter (which has the same structure as HTTPHeaderFilter)
    pub fn new_from_grpc(config: GRPCHeaderFilter) -> Result<Self, &'static str> {
        config.validate_response()?;
        Ok(Self {
            plan: HeaderModifierPlan::from_grpc(config)?,
        })
    }
}

impl UpstreamResponseFilter for ResponseHeaderModifierFilter {
    fn name(&self) -> &str {
        "ResponseHeaderModifier"
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.modify_headers(session, log)
    }
}

impl ResponseHeaderModifierFilter {
    fn modify_headers(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Set headers
        for header in self.plan.set() {
            if session
                .set_response_header_value(header.name(), header.value().clone())
                .is_err()
            {
                log.push_err("set resp hdr fail");
            }
        }
        // Add headers
        for header in self.plan.add() {
            if session
                .append_response_header_value(header.name(), header.value().clone())
                .is_err()
            {
                log.push_err("add resp hdr fail");
            }
        }
        // Remove headers
        for name in self.plan.remove() {
            if session.remove_response_header(name).is_err() {
                log.push_err("remove resp hdr fail");
            }
        }
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/gateway_api_filters/response_header_modifier_test.rs"]
mod tests;
