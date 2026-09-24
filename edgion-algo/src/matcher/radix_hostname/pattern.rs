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
#[path = "../../../tests/unit/matcher/radix_hostname/pattern_test.rs"]
mod tests;
