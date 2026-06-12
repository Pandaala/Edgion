//! DebugAccessLogToHeader filter implementation
//!
//! This filter adds the current access log as a JSON string to the response header
//! for debugging purposes.

use crate::observe::access_log::AccessLogEntry;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::DebugAccessLogToHeaderConfig;

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
        let _ = session.set_response_header("X-Debug-Access-Log", &json);

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
