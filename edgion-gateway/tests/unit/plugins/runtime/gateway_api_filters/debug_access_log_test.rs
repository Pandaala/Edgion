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
        .withf(|name, value| name == "X-Debug-Access-Log" && serde_json::from_str::<serde_json::Value>(value).is_ok())
        .times(1)
        .returning(|_, _| Ok(()));
    let mut log = PluginLog::new("DebugAccessLogToHeader");

    let result = filter.add_debug_header(false, &mut session, &mut log);

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert_eq!(log.render_for_test(), "");
}
