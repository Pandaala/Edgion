//! DebugAccessLogToHeader filter implementation
//!
//! This filter adds the current access log as a JSON string to the response header
//! for debugging purposes.

use crate::core::observe::access_log::AccessLogEntry;
use crate::core::plugins::plugin_runtime::log::PluginLog;
use crate::core::plugins::plugin_runtime::traits::{PluginSession, UpstreamResponseFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::DebugAccessLogToHeaderConfig;

pub struct DebugAccessLogToHeaderFilter {}

impl DebugAccessLogToHeaderFilter {
    pub fn new(_config: &DebugAccessLogToHeaderConfig) -> Self {
        Self {}
    }

    fn add_debug_header(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        // Get EdgionHttpContext from session
        let ctx = session.ctx();

        // Create access log entry from context
        let entry = AccessLogEntry::from_context(ctx);

        // Convert to JSON
        let json = entry.to_json();

        // Set the debug header
        if let Err(e) = session.set_response_header("X-Debug-Access-Log", &json) {
            tracing::error!("Failed to set X-Debug-Access-Log header: {:?}", e);
        }

        PluginRunningResult::GoodNext
    }
}

impl UpstreamResponseFilter for DebugAccessLogToHeaderFilter {
    fn name(&self) -> &str {
        "DebugAccessLogToHeader"
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        _log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.add_debug_header(session)
    }
}
