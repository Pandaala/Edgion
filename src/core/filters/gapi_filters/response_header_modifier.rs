//! ResponseHeaderModifier filter implementation

use async_trait::async_trait;
use crate::types::resources::HTTPHeaderFilter;
use crate::types::filters::{PluginConf, PluginRunningResult, PluginRunningStage};
use crate::core::filters::plugin_runtime::traits::{Plugin, PluginSession};
use crate::core::filters::plugin_runtime::log::PluginLog;

pub struct ResponseHeaderModifierFilter {
    config: HTTPHeaderFilter,
}

impl ResponseHeaderModifierFilter {
    pub fn new(config: HTTPHeaderFilter) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Plugin for ResponseHeaderModifierFilter {
    fn name(&self) -> &str {
        "ResponseHeaderModifier"
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
        vec![PluginRunningStage::UpstreamResponseFilter]
    }

    fn check_schema(&self, _conf: &PluginConf) {}
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
        // Remove headers - TODO: need remove_response_header in FilterSession
        PluginRunningResult::GoodNext
    }
}

