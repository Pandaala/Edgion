//! RequestHeaderModifier filter implementation
//!
//! This filter modifies request headers based on HTTPHeaderFilter configuration.

use super::header_modifier::HeaderModifierPlan;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, RequestFilter};
use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::{GRPCHeaderFilter, HTTPHeaderFilter};

/// Filter that modifies request headers
pub struct RequestHeaderModifierFilter {
    plan: HeaderModifierPlan,
}

impl RequestHeaderModifierFilter {
    pub(crate) fn plan(&self) -> &HeaderModifierPlan {
        &self.plan
    }

    pub fn new(config: HTTPHeaderFilter) -> Result<Self, &'static str> {
        Ok(Self {
            plan: HeaderModifierPlan::from_http(config)?,
        })
    }

    /// Create from GRPCHeaderFilter (which has the same structure as HTTPHeaderFilter)
    pub fn new_from_grpc(config: GRPCHeaderFilter) -> Result<Self, &'static str> {
        Ok(Self {
            plan: HeaderModifierPlan::from_grpc(config)?,
        })
    }
}

#[async_trait]
impl RequestFilter for RequestHeaderModifierFilter {
    fn name(&self) -> &str {
        "RequestHeaderModifier"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        self.modify_headers(session, log)
    }
}

impl RequestHeaderModifierFilter {
    fn modify_headers(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Set headers - overwrite existing
        for header in self.plan.set() {
            if session
                .set_request_header_value(header.name(), header.value().clone())
                .is_err()
            {
                log.push_err("set req hdr fail");
            }
        }
        // Add headers - append to existing
        for header in self.plan.add() {
            if session
                .append_request_header_value(header.name(), header.value().clone())
                .is_err()
            {
                log.push_err("add req hdr fail");
            }
        }
        // Remove headers
        for name in self.plan.remove() {
            if session.remove_request_header(name).is_err() {
                log.push_err("remove req hdr fail");
            }
        }
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/gateway_api_filters/request_header_modifier_test.rs"]
mod tests;
