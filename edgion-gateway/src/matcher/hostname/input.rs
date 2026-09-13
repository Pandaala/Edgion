//! Checked borrowed inputs for Route hostname lookup, not certificate selection.

use edgion_resources::hostname::{HostnameError, HostnamePatternRef};
use std::borrow::Cow;

#[derive(Clone, Copy)]
pub struct HostnameQuery<'a>(&'a str);

impl<'a> HostnameQuery<'a> {
    /// HTTP callers pass their already normalized authority hostname.
    pub fn new(hostname: &'a str) -> Result<Self, HostnameError> {
        let pattern = HostnamePatternRef::parse(hostname)?;
        if pattern.is_wildcard() {
            return Err(HostnameError::InvalidPattern);
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
    pub fn query(&self) -> HostnameQuery<'_> {
        HostnameQuery(&self.0)
    }

    pub fn into_owned(self) -> String {
        self.0.into_owned()
    }
}

/// Route-only SNI normalization. Missing SNI remains a transport policy.
pub fn normalize_route_sni(sni: &str) -> Result<NormalizedRouteSni<'_>, HostnameError> {
    let normalized = if sni.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(sni.to_ascii_lowercase())
    } else {
        Cow::Borrowed(sni)
    };
    HostnameQuery::new(&normalized)?;
    Ok(NormalizedRouteSni(normalized))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_queries_borrow_and_sni_case_folding_is_separate() {
        assert!(matches!(
            normalize_route_sni("a.b.example.com").unwrap().0,
            Cow::Borrowed(_)
        ));
        assert_eq!(
            normalize_route_sni("A.B.Example.COM").unwrap().query().as_str(),
            "a.b.example.com"
        );
        assert!(HostnameQuery::new("UPPER.test").is_err());
        assert!(HostnameQuery::new("grpc").is_ok());
    }

    #[test]
    fn invalid_sni_is_not_repaired_or_treated_as_missing() {
        for value in [
            "",
            "*",
            "*.test",
            ".test",
            "a..test",
            "a.test.",
            "a.test:443",
            " a.test",
            "a.test ",
            "é.test",
            "127.0.0.1",
            "[::1]",
        ] {
            assert!(normalize_route_sni(value).is_err(), "{value}");
        }
    }
}
