//! Tests for response builtins: status read/write, header read/write, upstream response filter phase.

use super::*;

#[test]
fn test_resp_status_read() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_status().returning(|| Some(404));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Status" && v == "not_found")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let s = resp.status\nif s != nil { if s == 404 { resp.set_header(\"X-Status\", \"not_found\") } }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_status_nil() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_status().returning(|| None);
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Status" && v == "no_status")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let s = resp.status\nif s == nil { resp.set_header(\"X-Status\", \"no_status\") }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_read() {
    let mut session = MockPluginSession::new();
    session
        .expect_get_response_header()
        .withf(|h| h == "X-B")
        .returning(|_| Some("hello".to_string()));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Copy" && v == "hello")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let v = resp.header(\"X-B\")\nif v != nil { resp.set_header(\"X-Copy\", v) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_nil() {
    let mut session = MockPluginSession::new();
    session
        .expect_get_response_header()
        .withf(|h| h == "X-B")
        .returning(|_| None);
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Copy" && v == "none")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let v = resp.header(\"X-B\")\nif v == nil { resp.set_header(\"X-Copy\", \"none\") }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_has_header_true() {
    let mut session = MockPluginSession::new();
    session
        .expect_get_response_header()
        .withf(|h| h == "X-B")
        .returning(|_| Some("val".to_string()));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-R" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "if resp.has_header(\"X-B\") { resp.set_header(\"X-R\", \"yes\") }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_has_header_false() {
    let mut session = MockPluginSession::new();
    session
        .expect_get_response_header()
        .withf(|h| h == "X-B")
        .returning(|_| None);
    let result = run(
        "if resp.has_header(\"X-B\") { resp.set_header(\"X-R\", \"yes\") }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_set_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-C" && v == "val")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"resp.set_header("X-C", "val")"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_append_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_append_response_header()
        .withf(|h, v| h == "Vary" && v == "Origin")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"resp.append_header("Vary", "Origin")"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_remove_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_remove_response_header()
        .withf(|h| h == "X-Remove")
        .times(1)
        .returning(|_| Ok(()));
    let result = run(r#"resp.remove_header("X-Remove")"#, session);
    assert_eq!(result, good_next());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_upstream_response_filter_phase() {
    use crate::core::gateway::plugins::http::dsl::config::DslConfig;
    use crate::core::gateway::plugins::http::dsl::plugin::DslPlugin;
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Phase" && v == "resp")
        .times(1)
        .returning(|_, _| Ok(()));

    let config = DslConfig {
        source: Some(r#"resp.set_header("X-Phase", "resp")"#.to_string()),
        name: "test-resp".into(),
        ..Default::default()
    };
    let plugin = DslPlugin::from_config_response(&config).expect("from_config_response failed");
    let mut log = PluginLog::new("test");
    let mut s = session;
    // run_upstream_response_filter uses block_in_place — requires multi-thread Tokio runtime
    let result = plugin.run_upstream_response_filter(&mut s as &mut dyn PluginSession, &mut log);
    assert_eq!(result, good_next());
}
