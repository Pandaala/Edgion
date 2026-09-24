use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;

fn mock_with(headers: &[(&str, &[u8])]) -> MockPluginSession {
    let mut map = http::HeaderMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::from_bytes(name.as_bytes()).unwrap();
        map.append(name, http::HeaderValue::from_bytes(value).unwrap());
    }
    let mut mock = MockPluginSession::new();
    mock.expect_request_header_map().return_const(map);
    mock
}

fn response_mock_with(headers: &[(&str, &[u8])]) -> MockPluginSession {
    let mut map = http::HeaderMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::from_bytes(name.as_bytes()).unwrap();
        map.append(name, http::HeaderValue::from_bytes(value).unwrap());
    }
    let mut mock = MockPluginSession::new();
    mock.expect_response_header_map().return_const(map);
    mock
}

#[test]
fn borrowed_accessors_work_through_trait_object() {
    let mock = mock_with(&[("authorization", b"Bearer abc")]);
    let session: &dyn PluginSession = &mock;

    assert_eq!(session.header_str("authorization"), Some("Bearer abc"));
    // Header lookup is case-insensitive.
    assert_eq!(session.header_str("Authorization"), Some("Bearer abc"));
    assert_eq!(session.header_str("cookie"), None);
}

/// A present-but-non-UTF-8 header reads as **absent** through `header_str`,
/// exactly as it does through the owned `header_value`. Raw presence
/// (`HeaderMap::contains_key`) disagrees, which is why a migration must use
/// `header_str(..).is_some()` and never `contains_key` to preserve
/// behavior. Raw bytes stay reachable through `header_all_bytes`.
#[test]
fn non_utf8_value_reads_as_absent_through_header_str() {
    let session = mock_with(&[("x-raw", &[0xff, 0xfe][..])]);

    assert_eq!(session.header_str("x-raw"), None);
    assert!(session.request_header_map().contains_key("x-raw"));
    let raw: Vec<_> = session.header_all_bytes("x-raw").collect();
    assert_eq!(raw, vec![&[0xff, 0xfe][..]]);
}

#[test]
fn multi_value_header_preserves_per_name_order_and_duplicates() {
    let session = mock_with(&[("accept", b"a"), ("accept", b"b")]);

    // The single-value accessors expose only the first value, matching
    // `header_value`.
    assert_eq!(session.header_str("accept"), Some("a"));

    let all: Vec<_> = session.header_all_bytes("accept").collect();
    assert_eq!(all, vec![&b"a"[..], &b"b"[..]]);

    let pairs: Vec<_> = session.iter_headers().collect();
    assert_eq!(pairs, vec![("accept", &b"a"[..]), ("accept", &b"b"[..])]);
}

#[test]
fn response_headers_preserve_raw_values_and_duplicate_order() {
    let session = response_mock_with(&[("set-cookie", b"a=1"), ("set-cookie", &[0xff][..])]);

    let pairs: Vec<_> = session.iter_response_headers().collect();
    assert_eq!(pairs, vec![("set-cookie", &b"a=1"[..]), ("set-cookie", &[0xff][..])]);
}
