//! A string newtype whose own `Debug` always prints `[redacted]`.
//!
//! The alternative — a hand-written `Debug` impl on the owning config struct
//! that redacts one named field — has already bitten this codebase twice
//! (`GatewayConfig::admin_token`, then `GeoIpConfig::update_url`, the second
//! being the fix for a leak the first pattern let happen again): every future
//! field is silently *not* redacted unless the author remembers to extend the
//! hand-written impl, and a future credential field is printed in full by
//! default. Wrapping the sensitive field in `RedactedString` inverts that: a
//! plain `#[derive(Debug)]` on the owning struct redacts it by construction,
//! and adding a new non-sensitive field is safe without touching this type at
//! all.
//!
//! `Deref<Target = str>` keeps every existing `&str`-shaped call site working
//! (`.as_deref()` on `Option<RedactedString>`, deref coercion to `&str`
//! function parameters) without a wrapper-aware rewrite at the use site.
//! `Display` and `Serialize` are deliberately real (not redacted): this type
//! only protects `Debug`/diagnostic output, not the value itself — the field
//! is still sent over the wire / used for its real purpose (an HTTP request,
//! a bearer token comparison) exactly as a plain `String` would be.

use std::fmt;
use std::ops::Deref;

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct RedactedString(String);

impl RedactedString {
    pub fn into_inner(self) -> String {
        self.0
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for RedactedString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for RedactedString {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl Deref for RedactedString {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RedactedString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The whole point: never print the real value, regardless of what it is.
impl fmt::Debug for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
#[path = "../../tests/unit/util/redacted_test.rs"]
mod tests;
