//! Gateway API hostname syntax and intersection, independent of routing payloads.

use std::{error::Error, fmt, net::IpAddr};

pub const MAX_HOSTNAME_LEN: u32 = 253;

/// A rejected Gateway API hostname pattern.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadixHostnamePatternError {
    InvalidPattern,
    InvalidLength,
    IpLiteral,
}

impl fmt::Display for RadixHostnamePatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPattern => "hostnames must use lowercase DNS labels with only an optional leading '*.'",
            Self::InvalidLength => "hostnames must contain between 1 and 253 bytes",
            Self::IpLiteral => "hostnames must not be IP address literals",
        })
    }
}

impl Error for RadixHostnamePatternError {}

/// A borrowed, validated Gateway API v1.5 hostname pattern.
///
/// This is a configuration value, not an HTTP authority or certificate SAN.
/// The Gateway API grammar permits single-label names and does not impose a
/// separate 63-byte label limit. A bare `*` is not a hostname pattern.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadixHostnamePatternRef<'a> {
    pattern: &'a str,
    wildcard: bool,
}

impl<'a> RadixHostnamePatternRef<'a> {
    pub fn parse(pattern: &'a str) -> Result<Self, RadixHostnamePatternError> {
        if pattern.is_empty() || pattern.len() > MAX_HOSTNAME_LEN as usize {
            return Err(RadixHostnamePatternError::InvalidLength);
        }
        // Check the complete literal, not the suffix of a wildcard pattern:
        // '*.127.0.0.1' is a valid DNS pattern, not an IP address literal.
        if pattern.parse::<IpAddr>().is_ok() {
            return Err(RadixHostnamePatternError::IpLiteral);
        }
        let wildcard = pattern.starts_with("*.");
        let name = if wildcard { &pattern[2..] } else { pattern };
        let alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
        if !name.split('.').all(|label| {
            let bytes = label.as_bytes();
            bytes.first().is_some_and(|byte| alphanumeric(*byte))
                && bytes.last().is_some_and(|byte| alphanumeric(*byte))
                && bytes.iter().all(|byte| alphanumeric(*byte) || *byte == b'-')
        }) {
            return Err(RadixHostnamePatternError::InvalidPattern);
        }
        Ok(Self { pattern, wildcard })
    }

    pub fn as_str(self) -> &'a str {
        self.pattern
    }

    pub fn is_wildcard(self) -> bool {
        self.wildcard
    }

    /// The wildcard suffix, including its leading dot (for example `.example.com`).
    pub fn suffix(self) -> Option<&'a str> {
        self.wildcard.then(|| &self.pattern[1..])
    }

    /// Match an already validated, normalized hostname query without parsing it.
    ///
    /// Callers validate the query once before visiting candidate patterns. This
    /// method does not accept authority ports or perform case normalization.
    pub fn matches(self, hostname: &str) -> bool {
        radix_hostname_matches(hostname, self.pattern)
    }

    /// Return the narrower input pattern when their hostname sets intersect.
    pub fn intersection(self, other: Self) -> Option<Self> {
        match (self.suffix(), other.suffix()) {
            (None, _) => other.matches(self.pattern).then_some(self),
            (Some(_), None) => self.matches(other.pattern).then_some(other),
            (Some(left), Some(right)) => {
                if left.ends_with(right) {
                    Some(self)
                } else if right.ends_with(left) {
                    Some(other)
                } else {
                    None
                }
            }
        }
    }
}

/// Pure suffix predicate for validated configuration and normalized queries.
/// Runtime owners validate once before iteration; this performs no parsing or
/// allocation. Exact comparison preserves the existing ASCII-insensitive API.
pub fn radix_hostname_matches(hostname: &str, pattern: &str) -> bool {
    match pattern.strip_prefix('*') {
        Some(suffix) if suffix.starts_with('.') => hostname.len() > suffix.len() && hostname.ends_with(suffix),
        Some(_) => false,
        None => hostname.eq_ignore_ascii_case(pattern),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_api_hostname_grammar_and_lengths() {
        for value in ["a", "a-b", "example.com", "*.a", "*.example.com", "*.127.0.0.1"] {
            assert!(RadixHostnamePatternRef::parse(value).is_ok(), "{value}");
        }
        for value in [
            "",
            "*",
            "*.",
            "foo.*.com",
            "example.com.",
            ".example.com",
            "a..b",
            "-a",
            "a-",
            "A.example.com",
            "a_b",
            "a:80",
            "127.0.0.1",
            "::1",
            "[::1]",
            "é.example",
            " a",
        ] {
            assert!(RadixHostnamePatternRef::parse(value).is_err(), "{value}");
        }
        assert_eq!(
            RadixHostnamePatternRef::parse("127.0.0.1"),
            Err(RadixHostnamePatternError::IpLiteral)
        );
        assert!(RadixHostnamePatternRef::parse(&"a".repeat(253)).is_ok());
        assert!(RadixHostnamePatternRef::parse(&"a".repeat(254)).is_err());
        assert!(RadixHostnamePatternRef::parse(&format!("*.{}", "a".repeat(251))).is_ok());
        assert!(RadixHostnamePatternRef::parse(&format!("*.{}", "a".repeat(252))).is_err());
    }

    #[test]
    fn wildcard_matches_preserve_label_boundaries_and_require_a_prefix() {
        let pattern = RadixHostnamePatternRef::parse("*.example.com").unwrap();
        assert!(pattern.is_wildcard());
        assert_eq!(pattern.suffix(), Some(".example.com"));
        for name in ["a.example.com", "a.b.example.com"] {
            assert!(pattern.matches(name));
        }
        for name in ["example.com", "badexample.com", ".example.com", "example.org"] {
            assert!(!pattern.matches(name));
        }
    }

    #[test]
    fn intersections_are_commutative_and_return_the_narrower_input() {
        for (left, right, expected) in [
            ("a", "a", Some("a")),
            ("a", "b", None),
            ("*.example.com", "a.example.com", Some("a.example.com")),
            ("*.example.com", "a.b.example.com", Some("a.b.example.com")),
            ("*.example.com", "example.com", None),
            ("*.example.com", "*.b.example.com", Some("*.b.example.com")),
            ("*.example.com", "*.badexample.com", None),
            ("*.a", "a.a", Some("a.a")),
        ] {
            let left = RadixHostnamePatternRef::parse(left).unwrap();
            let right = RadixHostnamePatternRef::parse(right).unwrap();
            assert_eq!(left.intersection(right).map(RadixHostnamePatternRef::as_str), expected);
            assert_eq!(right.intersection(left).map(RadixHostnamePatternRef::as_str), expected);
            assert_eq!(left.intersection(left), Some(left));
        }
    }
}
