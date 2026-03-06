//! RequestHeaderModifier filter implementation
//!
//! This filter modifies request headers based on HTTPHeaderFilter configuration.

use crate::core::gateway::plugins::runtime::log::PluginLog;
use crate::core::gateway::plugins::runtime::traits::{PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::{GRPCHeaderFilter, HTTPHeaderFilter};
use async_trait::async_trait;

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

    async fn run_request(&self, session: &mut dyn PluginSession, _log: &mut PluginLog) -> PluginRunningResult {
        self.modify_headers(session)
    }
}

impl RequestHeaderModifierFilter {
    fn modify_headers(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        // Set headers - overwrite existing
        if let Some(headers) = &self.config.set {
            for h in headers {
                let _ = session.set_request_header(&h.name, &h.value);
            }
        }
        // Add headers - append to existing
        if let Some(headers) = &self.config.add {
            for h in headers {
                let _ = session.append_request_header(&h.name, &h.value);
            }
        }
        // Remove headers
        if let Some(names) = &self.config.remove {
            for name in names {
                let _ = session.remove_request_header(name);
            }
        }
        PluginRunningResult::GoodNext
    }
}
