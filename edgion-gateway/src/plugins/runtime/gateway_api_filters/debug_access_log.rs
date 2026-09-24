//! DebugAccessLogToHeader filter implementation
//!
//! This filter adds the current access log as a JSON string to the response header
//! for debugging purposes.

use crate::observe::access_log::AccessLogEntry;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::DebugAccessLogToHeaderConfig;
use edgion_resources::sensitive;

pub struct DebugAccessLogToHeaderFilter {}

impl DebugAccessLogToHeaderFilter {
    pub fn new(_config: &DebugAccessLogToHeaderConfig) -> Self {
        Self {}
    }

    fn add_debug_header(
        &self,
        wrap_mode: bool,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if wrap_mode {
            let _ = log.push("wrap mode:header suppressed");
            return PluginRunningResult::GoodNext;
        }

        let ctx = session.ctx();
        let entry = AccessLogEntry::from_context(ctx);
        let json = entry.to_json();
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
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let wrap_mode = sensitive::is_wrap();
        self.add_debug_header(wrap_mode, session, log)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/gateway_api_filters/debug_access_log_test.rs"]
mod tests;
