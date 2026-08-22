//! Canonical exact HTTP(S) Origin evidence shared by protocol frontends.

use std::collections::BTreeSet;

use url::{Origin, Url};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExactCorsOriginSet {
    origins: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExactOriginError {
    Invalid,
    NonCanonical,
}

impl ExactCorsOriginSet {
    pub fn insert_configured_literal(&mut self, value: &str) -> bool {
        let Ok(canonical) = canonical_http_origin(value) else {
            return false;
        };
        if canonical != value {
            return false;
        }
        self.origins.insert(canonical)
    }

    pub fn extend_configured_literals<'a>(&mut self, values: impl IntoIterator<Item = &'a str>) {
        for value in values {
            self.insert_configured_literal(value);
        }
    }

    pub fn contains_request_origin(&self, value: &str) -> Result<bool, ExactOriginError> {
        let canonical = canonical_http_origin(value)?;
        if canonical != value {
            return Err(ExactOriginError::NonCanonical);
        }
        Ok(self.origins.contains(&canonical))
    }

    pub fn len(&self) -> usize {
        self.origins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.origins.iter().map(String::as_str)
    }
}

fn canonical_http_origin(value: &str) -> Result<String, ExactOriginError> {
    if !value.is_ascii() {
        return Err(ExactOriginError::NonCanonical);
    }
    let parsed = Url::parse(value).map_err(|_| ExactOriginError::Invalid)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ExactOriginError::Invalid);
    }
    match parsed.origin() {
        Origin::Tuple(..) => Ok(parsed.origin().ascii_serialization()),
        Origin::Opaque(_) => Err(ExactOriginError::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_literals_must_already_be_canonical_ascii_origins() {
        let mut set = ExactCorsOriginSet::default();
        assert!(set.insert_configured_literal("https://example.com"));
        assert!(set.insert_configured_literal("http://192.0.2.1:8080"));
        assert!(set.insert_configured_literal("http://[::1]:8080"));
        assert!(set.insert_configured_literal("https://xn--bcher-kva.example"));
        assert!(!set.insert_configured_literal("HTTPS://EXAMPLE.COM"));
        assert!(!set.insert_configured_literal("https://example.com:443"));
        assert!(!set.insert_configured_literal("https://example.com/"));
        assert!(!set.insert_configured_literal("https://user@example.com"));
        assert!(!set.insert_configured_literal("https://example.com/path"));
        assert!(!set.insert_configured_literal("https://example.com?q=1"));
        assert!(!set.insert_configured_literal("https://example.com#fragment"));
        assert!(!set.insert_configured_literal("null"));
        assert!(!set.insert_configured_literal("https://éxample.com"));
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            vec![
                "http://192.0.2.1:8080",
                "http://[::1]:8080",
                "https://example.com",
                "https://xn--bcher-kva.example"
            ]
        );
    }

    #[test]
    fn request_origin_is_not_silently_normalized() {
        let mut set = ExactCorsOriginSet::default();
        set.insert_configured_literal("https://example.com");
        assert_eq!(set.contains_request_origin("https://example.com"), Ok(true));
        assert_eq!(
            set.contains_request_origin("HTTPS://EXAMPLE.COM"),
            Err(ExactOriginError::NonCanonical)
        );
        assert_eq!(
            set.contains_request_origin("https://example.com/"),
            Err(ExactOriginError::NonCanonical)
        );
        assert_eq!(set.contains_request_origin("http://example.com"), Ok(false));
        assert_eq!(set.contains_request_origin("null"), Err(ExactOriginError::Invalid));
    }
}
