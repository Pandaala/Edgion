use super::*;
use crate::observe::logs::buffer::{LogBuffer, ULogBuffer};

#[test]
fn render_for_test_empty_buffers_returns_empty_string() {
    let log = PluginLog::new("test");
    assert_eq!(log.render_for_test(), "");
}

#[test]
fn render_for_test_log_only_joins_entries_with_newline() {
    let mut log = PluginLog::new("test");
    log.push("first");
    log.push("second");
    assert_eq!(log.render_for_test(), "first\nsecond");
}

#[test]
fn render_for_test_ulog_only_when_log_is_none() {
    // Force the ulog-only branch by direct field assignment.
    let mut log = PluginLog::new("test");
    let mut u = ULogBuffer::new();
    u.push("only-ulog");
    log.ulog = Some(u);
    assert_eq!(log.render_for_test(), "only-ulog");
}

#[test]
fn render_for_test_both_buffers_separated_by_marker() {
    // Construct both buffers directly to make the separator behavior
    // testable independent of the tier-1 overflow threshold.
    let mut main = LogBuffer::new();
    main.push("main-a");
    main.push("main-b");
    let mut user = ULogBuffer::new();
    user.push("user-x");
    let log = PluginLog {
        log: Some(main),
        ulog: Some(user),
        ..PluginLog::new("test")
    };
    assert_eq!(log.render_for_test(), "main-a\nmain-b\n--- \nuser-x");
}
