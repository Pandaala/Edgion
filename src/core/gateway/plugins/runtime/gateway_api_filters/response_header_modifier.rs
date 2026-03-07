//! ResponseHeaderModifier filter implementation

use crate::core::gateway::plugins::runtime::log::PluginLog;
use crate::core::gateway::plugins::runtime::traits::{PluginSession, UpstreamResponseFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::{GRPCHeaderFilter, HTTPHeaderFilter};

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
        _log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.modify_headers(session)
    }
}

impl ResponseHeaderModifierFilter {
    fn modify_headers(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        // Set headers
        if let Some(headers) = &self.config.set {
            for h in headers {
                let _ = session.set_response_header(&h.name, &h.value);
            }
        }
        // Add headers
        if let Some(headers) = &self.config.add {
            for h in headers {
                let _ = session.append_response_header(&h.name, &h.value);
            }
        }
        // Remove headers
        if let Some(headers) = &self.config.remove {
            for h in headers {
                let _ = session.remove_response_header(h);
            }
        }
        PluginRunningResult::GoodNext
    }
}
