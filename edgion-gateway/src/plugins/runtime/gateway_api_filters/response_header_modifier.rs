//! ResponseHeaderModifier filter implementation

use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::{GRPCHeaderFilter, HTTPHeaderFilter};

pub struct ResponseHeaderModifierFilter {
    config: HTTPHeaderFilter,
}

impl ResponseHeaderModifierFilter {
    pub fn new(config: HTTPHeaderFilter) -> Self {
        Self { config }
    }

    /// Create from GRPCHeaderFilter (which has the same structure as HTTPHeaderFilter)
    pub fn new_from_grpc(config: GRPCHeaderFilter) -> Self {
        // GRPCHeaderFilter and HTTPHeaderFilter have identical structure
        Self {
            config: HTTPHeaderFilter {
                set: config.set,
                add: config.add,
                remove: config.remove,
            },
        }
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
        if let Some(headers) = &self.config.set {
            for h in headers {
                if session.set_response_header(&h.name, &h.value).is_err() {
                    log.push_err("set resp hdr fail");
                }
            }
        }
        // Add headers
        if let Some(headers) = &self.config.add {
            for h in headers {
                if session.append_response_header(&h.name, &h.value).is_err() {
                    log.push_err("add resp hdr fail");
                }
            }
        }
        // Remove headers
        if let Some(headers) = &self.config.remove {
            for h in headers {
                if session.remove_response_header(h).is_err() {
                    log.push_err("remove resp hdr fail");
                }
            }
        }
        PluginRunningResult::GoodNext
    }
}
