//! Borrowed request-header accessors for [`PluginSession`].
//!
//! These live outside the `#[cfg_attr(test, mockall::automock)]` trait on
//! purpose. `mockall` 0.14 accepts only bare-reference (`&T`) or owned returns;
//! it rejects any return type nesting a non-`'static` lifetime (`Option<&str>`,
//! `Option<&[u8]>`, a borrowed view struct) and rejects `Fn`-object parameters.
//! By deriving everything here from the single bare-reference method
//! [`PluginSession::request_header_map`], this trait can use whatever shape is
//! most ergonomic while the generated mock only ever needs one expectation.
//!
//! The blanket impl covers `dyn PluginSession`, which is how plugins actually
//! receive the session.
//!
//! # Choosing between the borrowed and owned tiers
//!
//! A borrowed value keeps the session immutably borrowed for its whole
//! lifetime. Use [`PluginSession::header_value`] instead when the value must be
//! moved across an `.await`, handed to `spawn_blocking`, or held while the
//! session is used mutably - the borrow checker will reject the borrowed form
//! in exactly those cases.

use super::session::PluginSession;

pub trait PluginSessionHeaderExt {
    /// First value for `name`, dropping present-but-non-UTF-8 values.
    ///
    /// This is the allocation-free equivalent of
    /// [`PluginSession::header_value`] and reproduces its UTF-8 filtering
    /// byte-for-byte, so `header_str(n).is_some()` is a semantics-preserving
    /// replacement for `header_value(n).is_some()`.
    fn header_str(&self, name: &str) -> Option<&str>;

    /// Every value for `name`, as raw bytes, preserving duplicates and their
    /// insertion order for that requested name.
    fn header_all_bytes(&self, name: &str) -> impl Iterator<Item = &[u8]>;

    /// Every (name, raw value) pair, preserving duplicates and each name's
    /// value insertion order. Distinct names follow `HeaderMap`'s arbitrary
    /// iteration order; callers must not treat it as wire order.
    fn iter_headers(&self) -> impl Iterator<Item = (&str, &[u8])>;
}

impl<T: PluginSession + ?Sized> PluginSessionHeaderExt for T {
    #[inline]
    fn header_str(&self, name: &str) -> Option<&str> {
        self.request_header_map()
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    #[inline]
    fn header_all_bytes(&self, name: &str) -> impl Iterator<Item = &[u8]> {
        self.request_header_map()
            .get_all(name)
            .into_iter()
            .map(|value| value.as_bytes())
    }

    #[inline]
    fn iter_headers(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.request_header_map()
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
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
}
