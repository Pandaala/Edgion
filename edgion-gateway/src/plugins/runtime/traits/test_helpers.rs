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
