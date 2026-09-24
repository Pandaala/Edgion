//! Borrowed header accessors for [`PluginSession`].
//!
//! These live outside the `#[cfg_attr(test, mockall::automock)]` trait on
//! purpose. `mockall` 0.14 accepts only bare-reference (`&T`) or owned returns;
//! it rejects any return type nesting a non-`'static` lifetime (`Option<&str>`,
//! `Option<&[u8]>`, a borrowed view struct) and rejects `Fn`-object parameters.
//! Request and response accessors derive from the corresponding bare-reference
//! [`PluginSession::request_header_map`] and [`PluginSession::response_header_map`]
//! methods. This trait can use ergonomic borrowed shapes while each side adds only
//! one expectation to the generated mock.
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

    /// Every live response (name, raw value) pair, preserving duplicates and
    /// each name's value insertion order. Returns an empty iterator before a
    /// response is bound.
    fn iter_response_headers(&self) -> impl Iterator<Item = (&str, &[u8])>;
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

    #[inline]
    fn iter_response_headers(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.response_header_map()
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_bytes()))
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/traits/header_ext_test.rs"]
mod tests;
