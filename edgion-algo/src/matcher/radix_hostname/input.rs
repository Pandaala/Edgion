//! Checked borrowed inputs for Route hostname lookup, not certificate selection.

use super::{RadixHostnamePatternError, RadixHostnamePatternRef};
use std::borrow::Cow;

#[derive(Clone, Copy)]
pub struct RadixHostnameQuery<'a>(&'a str);

impl<'a> RadixHostnameQuery<'a> {
    /// HTTP callers pass their already normalized authority hostname.
    pub fn new(hostname: &'a str) -> Result<Self, RadixHostnamePatternError> {
        let pattern = RadixHostnamePatternRef::parse(hostname)?;
        if pattern.is_wildcard() {
            return Err(RadixHostnamePatternError::InvalidPattern);
        }
        Ok(Self(hostname))
    }

    pub fn as_str(self) -> &'a str {
        self.0
    }
}

/// Validated Route SNI storage; borrowing a query never repeats validation.
pub struct NormalizedRouteSni<'a>(Cow<'a, str>);

impl NormalizedRouteSni<'_> {
    pub fn query(&self) -> RadixHostnameQuery<'_> {
        RadixHostnameQuery(&self.0)
    }

    pub fn into_owned(self) -> String {
        self.0.into_owned()
    }
}

/// Route-only SNI normalization. Missing SNI remains a transport policy.
pub fn normalize_route_sni(sni: &str) -> Result<NormalizedRouteSni<'_>, RadixHostnamePatternError> {
    let normalized = if sni.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(sni.to_ascii_lowercase())
    } else {
        Cow::Borrowed(sni)
    };
    RadixHostnameQuery::new(&normalized)?;
    Ok(NormalizedRouteSni(normalized))
}

#[cfg(test)]
#[path = "../../../tests/unit/matcher/radix_hostname/input_test.rs"]
mod tests;
