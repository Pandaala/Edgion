//! Test helpers for `MockPluginSession` setup.
//!
//! Import with `use crate::plugins::runtime::traits::test_helpers::*;`
//! (available only in `#[cfg(test)]` contexts).

#[cfg(test)]
use crate::plugins::runtime::traits::session::MockPluginSession;

/// Set up default expectations for the five path/URI accessors on a `MockPluginSession`.
///
/// Use this in tests that need a session mock but don't care about exact path
/// semantics.  All five accessors return values derived from `path`:
///
/// | method              | return value         |
/// |---------------------|----------------------|
/// | `get_path()`        | `path`               |
/// | `get_request_path()`| `path` (entry-time)  |
/// | `get_uri()`         | `path` (no query)    |
/// | `get_request_uri()` | `path` (no query)    |
/// | `get_query()`       | `None`               |
///
/// For tests that require distinct per-accessor values (e.g. testing rewrite
/// behaviour where current path differs from entry path), set up the individual
/// `expect_get_*` expectations directly instead.
///
/// # Example
///
/// ```rust,ignore
/// let mut session = MockPluginSession::new();
/// setup_default_path_mocks(&mut session, "/api/v1/orders");
/// // ... add other expectations ...
/// ```
#[cfg(test)]
pub fn setup_default_path_mocks(mock: &mut MockPluginSession, path: &str) {
    let path_owned = path.to_string();
    mock.expect_get_path().return_const(path_owned.clone());
    mock.expect_get_request_path().return_const(path_owned.clone());
    mock.expect_get_uri().return_const(path_owned.clone());
    mock.expect_get_request_uri().return_const(path_owned);
    mock.expect_get_query().return_const(Option::<String>::None);
}

/// Wire every request-header tier on a `MockPluginSession` from one source of truth.
///
/// `PluginSession` exposes request headers four ways - the borrowed
/// `request_header_map()` (which every `PluginSessionHeaderExt` accessor is
/// derived from), the owned `header_value()`, and the bulk `request_headers()` /
/// `request_headers_bytes()`. A test that stubs only some of them panics as soon
/// as the code under test reads another, and a test that stubs them by hand can
/// let them disagree. This builds a single `http::HeaderMap` and serves all of
/// them from it, so they cannot drift.
///
/// Because this stubs `request_headers()` too, a test using it must NOT also set
/// `expect_request_headers()` - pass the full header list here instead.
///
/// Header names must be valid; values are taken as-is. Duplicate names are
/// appended, so multi-value headers can be expressed by repeating a name.
///
/// # Example
///
/// ```rust,ignore
/// let mut session = MockPluginSession::new();
/// setup_header_mocks(&mut session, &[("origin", "https://example.com")]);
/// ```
#[cfg(test)]
pub fn setup_header_mocks(mock: &mut MockPluginSession, headers: &[(&str, &str)]) {
    use std::str::FromStr;

    let mut map = http::HeaderMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::from_str(name).expect("test header name must be valid");
        let value = http::HeaderValue::from_str(value).expect("test header value must be valid");
        map.append(name, value);
    }

    // Owned tier mirrors `PingoraSessionAdapter::header_value`: first value
    // only, and a present-but-non-UTF-8 value reads as absent.
    let owned = map.clone();
    mock.expect_header_value()
        .returning(move |name| owned.get(name).and_then(|v| v.to_str().ok()).map(str::to_string));

    // Owned bulk tier mirrors `PingoraSessionAdapter::request_headers`: every
    // field line in order, dropping any whose value is not valid UTF-8.
    let all = map.clone();
    mock.expect_request_headers().returning(move || {
        all.iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str().to_string(), v.to_string())))
            .collect()
    });

    // Raw bulk tier mirrors `PingoraSessionAdapter::request_headers_bytes`: no
    // UTF-8 requirement, so non-UTF-8 values stay visible.
    let raw = map.clone();
    mock.expect_request_headers_bytes().returning(move || {
        raw.iter()
            .map(|(name, value)| (name.as_str().as_bytes().to_vec(), value.as_bytes().to_vec()))
            .collect()
    });

    mock.expect_request_header_map().return_const(map);
}

/// Wire the borrowed accessors that condition evaluation reads directly.
///
/// Conditions read `Method`, `Path`, `RequestPath`, `Header`, `Cookie`,
/// `RemoteIp`, and `DirectPeerIp` through borrowed session accessors instead of
/// the owned `key_get()`. This serves those accessors from one key/value list:
/// headers and cookies go through [`setup_header_mocks`], `Method`, `Path`, and
/// `RequestPath` default to empty, and the IP accessors are stubbed only when
/// listed so a test may set them separately. Other key sources still need an
/// explicit `expect_key_get()`.
#[cfg(test)]
pub fn setup_condition_input_mocks(mock: &mut MockPluginSession, inputs: &[(edgion_resources::common::KeyGet, &str)]) {
    use edgion_resources::common::KeyGet;

    let value_of = |target: &KeyGet| {
        inputs
            .iter()
            .find(|(key, _)| key == target)
            .map(|(_, value)| value.to_string())
    };
    let mut headers: Vec<(&str, &str)> = inputs
        .iter()
        .filter_map(|(key, value)| match key {
            KeyGet::Header { name } => Some((name.as_str(), *value)),
            _ => None,
        })
        .collect();
    let cookie = inputs
        .iter()
        .filter_map(|(key, value)| match key {
            KeyGet::Cookie { name } => Some(format!("{name}={value}")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("; ");
    if !cookie.is_empty() {
        headers.push(("cookie", cookie.as_str()));
    }
    setup_header_mocks(mock, &headers);
    mock.expect_get_method()
        .return_const(value_of(&KeyGet::Method).unwrap_or_default());
    mock.expect_get_path()
        .return_const(value_of(&KeyGet::Path).unwrap_or_default());
    mock.expect_get_request_path()
        .return_const(value_of(&KeyGet::RequestPath).unwrap_or_default());
    if let Some(ip) = value_of(&KeyGet::RemoteIp) {
        mock.expect_remote_ip().return_const(ip);
    }
    if let Some(ip) = value_of(&KeyGet::DirectPeerIp) {
        mock.expect_direct_peer_ip().return_const(ip);
    }
}
