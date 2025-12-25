//! RequestHeaderModifier filter implementation
//! 
//! This filter modifies request headers based on HTTPHeaderFilter configuration.

use async_trait::async_trait;
use crate::types::resources::{HTTPHeaderFilter, GRPCHeaderFilter};
use crate::types::filters::{PluginConf, PluginRunningResult, PluginRunningStage};
use crate::core::plugins::plugin_runtime::traits::{Plugin, PluginSession};
use crate::core::plugins::plugin_runtime::log::PluginLog;

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
            }
        }
    }
}

#[async_trait]
impl Plugin for RequestHeaderModifierFilter {
    fn name(&self) -> &str {
        "RequestHeaderModifier"
    }

    fn run_sync(
        &self,
        _stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        _log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.modify_headers(session)
    }

    async fn run_async(
        &self,
        _stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        _log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.modify_headers(session)
    }

    fn supports_sync(&self) -> bool {
        true
    }

    fn get_stages(&self) -> Vec<PluginRunningStage> {
        vec![PluginRunningStage::Request]
    }

    fn check_schema(&self, _conf: &PluginConf) {}
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

