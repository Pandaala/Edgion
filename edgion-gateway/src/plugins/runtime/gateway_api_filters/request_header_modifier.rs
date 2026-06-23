//! RequestHeaderModifier filter implementation
//!
//! This filter modifies request headers based on HTTPHeaderFilter configuration.

use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, RequestFilter};
use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::{GRPCHeaderFilter, HTTPHeaderFilter};

/// Filter that modifies request headers
pub struct RequestHeaderModifierFilter {
    config: HTTPHeaderFilter,
}

impl RequestHeaderModifierFilter {
    pub fn new(config: HTTPHeaderFilter) -> Self {
        Self { config }
    }

    /// Create from GRPCHeaderFilter (which has the same structure as HTTPHeaderFilter)
    pub fn new_from_grpc(config: GRPCHeaderFilter) -> Self {
        // GRPCHeaderFilter and HTTPHeaderFilter have identical structure
        // Both use HTTPHeader for set/add and Vec<String> for remove
        Self {
            config: HTTPHeaderFilter {
                set: config.set,
                add: config.add,
                remove: config.remove,
            },
        }
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
        if let Some(headers) = &self.config.set {
            for h in headers {
                if session.set_request_header(&h.name, &h.value).is_err() {
                    log.push_err("set req hdr fail");
                }
            }
        }
        // Add headers - append to existing
        if let Some(headers) = &self.config.add {
            for h in headers {
                if session.append_request_header(&h.name, &h.value).is_err() {
                    log.push_err("add req hdr fail");
                }
            }
        }
        // Remove headers
        if let Some(names) = &self.config.remove {
            for name in names {
                if session.remove_request_header(name).is_err() {
                    log.push_err("remove req hdr fail");
                }
            }
        }
        PluginRunningResult::GoodNext
    }
}
