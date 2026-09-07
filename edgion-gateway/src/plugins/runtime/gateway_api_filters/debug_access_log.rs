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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    #[test]
    fn wrap_mode_suppresses_header_before_reading_context() {
        let filter = DebugAccessLogToHeaderFilter::new(&DebugAccessLogToHeaderConfig::default());
        let mut session = MockPluginSession::new();
        session.expect_ctx().never();
        session.expect_set_response_header().never();
        let mut log = PluginLog::new("DebugAccessLogToHeader");

        let result = filter.add_debug_header(true, &mut session, &mut log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "wrap mode:header suppressed");
    }

    #[test]
    fn non_wrap_mode_sets_parseable_json_header() {
        let filter = DebugAccessLogToHeaderFilter::new(&DebugAccessLogToHeaderConfig::default());
        let mut session = MockPluginSession::new();
        session
            .expect_ctx()
            .times(1)
            .return_const(crate::EdgionHttpContext::new());
        session
            .expect_set_response_header()
            .withf(|name, value| {
                name == "X-Debug-Access-Log" && serde_json::from_str::<serde_json::Value>(value).is_ok()
            })
            .times(1)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new("DebugAccessLogToHeader");

        let result = filter.add_debug_header(false, &mut session, &mut log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "");
    }
}
