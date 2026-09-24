use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::HTTPHeader;
use mockall::Sequence;
use std::sync::Arc;

use parking_lot::Mutex;

fn header(name: &str, value: &str) -> HTTPHeader {
    HTTPHeader {
        name: name.to_string(),
        value: value.to_string(),
    }
}

fn http_filter(
    set: Option<Vec<HTTPHeader>>,
    add: Option<Vec<HTTPHeader>>,
    remove: Option<Vec<String>>,
) -> HTTPHeaderFilter {
    HTTPHeaderFilter { set, add, remove }
}

fn grpc_filter(
    set: Option<Vec<HTTPHeader>>,
    add: Option<Vec<HTTPHeader>>,
    remove: Option<Vec<String>>,
) -> GRPCHeaderFilter {
    GRPCHeaderFilter { set, add, remove }
}

fn assert_add_appends_without_replacing(filter: RequestHeaderModifierFilter) {
    let values = Arc::new(Mutex::new(vec!["client-a".to_string(), "client-b".to_string()]));
    let appended = Arc::clone(&values);
    let mut session = MockPluginSession::new();
    session.expect_set_request_header_value().never();
    session.expect_remove_request_header().never();
    session
        .expect_append_request_header_value()
        .withf(|name, value| name == "X-Trace" && value == "edge")
        .times(1)
        .returning(move |_, value| {
            appended
                .lock()
                .push(value.to_str().expect("configured value remains text").to_string());
            Ok(())
        });
    let mut log = PluginLog::new("RequestHeaderModifier");

    assert_eq!(
        filter.modify_headers(&mut session, &mut log),
        PluginRunningResult::GoodNext
    );
    assert_eq!(&*values.lock(), &["client-a", "client-b", "edge"]);
    assert_eq!(log.render_for_test(), "");
}

#[test]
fn http_add_appends_to_preexisting_field_lines_and_ignores_equivalent_duplicate() {
    assert_add_appends_without_replacing(
        RequestHeaderModifierFilter::new(http_filter(
            None,
            Some(vec![header("X-Trace", "edge"), header("x-trace", "ignored")]),
            None,
        ))
        .expect("valid header modifier"),
    );
}

#[test]
fn grpc_add_appends_to_preexisting_field_lines_and_ignores_equivalent_duplicate() {
    assert_add_appends_without_replacing(
        RequestHeaderModifierFilter::new_from_grpc(grpc_filter(
            None,
            Some(vec![header("X-Trace", "edge"), header("x-trace", "ignored")]),
            None,
        ))
        .expect("valid header modifier"),
    );
}

#[test]
fn operations_run_in_set_add_remove_order_for_distinct_names() {
    let filter = RequestHeaderModifierFilter::new(http_filter(
        Some(vec![header("X-Set", "set")]),
        Some(vec![header("X-Add", "add")]),
        Some(vec!["X-Remove".to_string()]),
    ))
    .expect("valid header modifier");
    let mut session = MockPluginSession::new();
    let mut sequence = Sequence::new();
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Set" && value == "set")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_append_request_header_value()
        .withf(|name, value| name == "X-Add" && value == "add")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|name| name == "X-Remove")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_| Ok(()));
    let mut log = PluginLog::new("RequestHeaderModifier");

    assert_eq!(
        filter.modify_headers(&mut session, &mut log),
        PluginRunningResult::GoodNext
    );
    assert_eq!(log.render_for_test(), "");
}

#[test]
fn retained_entry_errors_keep_static_diagnostics() {
    let filter = RequestHeaderModifierFilter::new(http_filter(
        Some(vec![header("X-Set", "set")]),
        Some(vec![header("X-Add", "add")]),
        Some(vec!["X-Remove".to_string()]),
    ))
    .expect("valid header modifier");
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header_value()
        .returning(|_, _| Err(Box::new(std::io::Error::other("test failure"))));
    session
        .expect_append_request_header_value()
        .returning(|_, _| Err(Box::new(std::io::Error::other("test failure"))));
    session
        .expect_remove_request_header()
        .returning(|_| Err(Box::new(std::io::Error::other("test failure"))));
    let mut log = PluginLog::new("RequestHeaderModifier");

    assert_eq!(
        filter.modify_headers(&mut session, &mut log),
        PluginRunningResult::GoodNext
    );
    assert_eq!(
        log.render_for_test(),
        "err: set req hdr fail\nerr: add req hdr fail\nerr: remove req hdr fail"
    );
}

#[test]
fn invalid_equivalent_duplicates_do_not_mutate_or_log() {
    let filter = RequestHeaderModifierFilter::new(http_filter(
        Some(vec![header("X-Set", "first"), header("x-set", "ignored")]),
        Some(vec![header("X-Add", "first"), header("x-add", "ignored")]),
        Some(vec!["X-Remove".to_string(), "x-remove".to_string()]),
    ))
    .expect("valid header modifier");
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header_value()
        .withf(|name, value| name == "X-Set" && value == "first")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_append_request_header_value()
        .withf(|name, value| name == "X-Add" && value == "first")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_request_header()
        .withf(|name| name == "X-Remove")
        .times(1)
        .returning(|_| Ok(()));
    let mut log = PluginLog::new("RequestHeaderModifier");

    assert_eq!(
        filter.modify_headers(&mut session, &mut log),
        PluginRunningResult::GoodNext
    );
    assert_eq!(log.render_for_test(), "");
}
