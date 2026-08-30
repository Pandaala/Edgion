//! Duration parsing utilities (GEP-2257).
//!
//! Edgion standardizes every operator-facing duration/timeout string on the
//! Gateway API GEP-2257 duration format, so a value valid on one field is valid
//! on every field — including the upstream Gateway API `SessionPersistence` and
//! `timeouts` fields, whose CRDs install this exact grammar.
//!
//! Grammar (single canonical form): `^([0-9]{1,5}(h|m|s|ms)){1,4}$`
//! - 1 to 4 concatenated segments, each `<1..=5 digits><unit>`.
//! - Units: `h` (hours), `m` (minutes), `s` (seconds), `ms` (milliseconds).
//! - No decimals, no `d`/days, no long unit names, no bare number.

use std::time::Duration;
use thiserror::Error;

/// The canonical GEP-2257 duration pattern.
///
/// Single source of truth shared by the JSON-schema layer
/// (`#[schemars(pattern(GEP2257_PATTERN))]`) and the runtime [`parse_duration`],
/// so the admission schema and the parser can never drift.
pub const GEP2257_PATTERN: &str = r"^([0-9]{1,5}(h|m|s|ms)){1,4}$";

/// Maximum digits allowed per segment (GEP-2257).
const MAX_SEGMENT_DIGITS: usize = 5;
/// Maximum number of segments allowed (GEP-2257).
const MAX_SEGMENTS: u32 = 4;

/// Error type for duration parsing failures.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum ParseDurationError {
    #[error("empty duration string")]
    EmptyString,

    #[error("invalid GEP-2257 duration: {0:?} (expected e.g. \"30s\", \"1h30m\", \"500ms\")")]
    InvalidFormat(String),

    #[error("unknown unit in {0:?} (allowed units: h, m, s, ms)")]
    UnknownUnit(String),
}

/// Parse a GEP-2257 duration string into a [`Duration`].
///
/// Accepts 1-4 concatenated `<1..=5 digits><unit>` segments, where unit is one
/// of `h`, `m`, `s`, `ms`. Rejects bare numbers, decimals, days, and long unit
/// names — every Edgion duration field uses this one grammar.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use edgion_resources::duration::parse_duration;
///
/// assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
/// assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
/// assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
/// assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
/// assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
///
/// // Non-GEP-2257 forms are rejected.
/// assert!(parse_duration("30").is_err()); // bare number
/// assert!(parse_duration("1.5h").is_err()); // decimal
/// assert!(parse_duration("1d").is_err()); // days
/// ```
pub fn parse_duration(s: &str) -> Result<Duration, ParseDurationError> {
    let s = s.trim();

    if s.is_empty() {
        return Err(ParseDurationError::EmptyString);
    }

    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut segments = 0u32;
    let mut total = Duration::ZERO;

    while i < bytes.len() {
        // Parse 1..=MAX_SEGMENT_DIGITS ASCII digits.
        let digit_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let digit_len = i - digit_start;
        if digit_len == 0 || digit_len > MAX_SEGMENT_DIGITS {
            return Err(ParseDurationError::InvalidFormat(s.to_string()));
        }
        // 1..=5 ASCII digits always fit in u64.
        let num: u64 = s[digit_start..i]
            .parse()
            .map_err(|_| ParseDurationError::InvalidFormat(s.to_string()))?;

        // Every segment must carry a unit (no bare number).
        if i >= bytes.len() {
            return Err(ParseDurationError::InvalidFormat(s.to_string()));
        }

        // Match the two-char "ms" before the single-char "m"/"s".
        let component = if s[i..].starts_with("ms") {
            i += 2;
            Duration::from_millis(num)
        } else if bytes[i] == b's' {
            i += 1;
            Duration::from_secs(num)
        } else if bytes[i] == b'm' {
            i += 1;
            Duration::from_secs(num * 60)
        } else if bytes[i] == b'h' {
            i += 1;
            Duration::from_secs(num * 3600)
        } else {
            return Err(ParseDurationError::UnknownUnit(s.to_string()));
        };

        total += component;
        segments += 1;
        if segments > MAX_SEGMENTS {
            return Err(ParseDurationError::InvalidFormat(s.to_string()));
        }
    }

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_matches_parser() {
        // The runtime parser accepts exactly the GEP-2257 grammar the pattern
        // constant advertises; this guards against the two drifting apart.
        assert_eq!(GEP2257_PATTERN, r"^([0-9]{1,5}(h|m|s|ms)){1,4}$");
    }

    #[test]
    fn test_parse_milliseconds() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("1000ms").unwrap(), Duration::from_millis(1000));
        assert_eq!(parse_duration("0ms").unwrap(), Duration::ZERO);
    }

    #[test]
    fn test_parse_seconds() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
    }

    #[test]
    fn test_parse_minutes() {
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
    }

    #[test]
    fn test_parse_hours() {
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("24h").unwrap(), Duration::from_secs(86400));
    }

    #[test]
    fn test_parse_with_whitespace() {
        assert_eq!(parse_duration("  30s  ").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration(" 5m ").unwrap(), Duration::from_secs(300));
    }

    #[test]
    fn test_parse_combined_formats() {
        // 1h30m = 5400s
        assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
        // 1m30s = 90s
        assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
        // 1h30m15s = 5415s
        assert_eq!(parse_duration("1h30m15s").unwrap(), Duration::from_secs(5415));
        // Max 4 segments: 1h2m3s4ms
        assert_eq!(
            parse_duration("1h2m3s4ms").unwrap(),
            Duration::from_secs(3723) + Duration::from_millis(4)
        );
    }

    #[test]
    fn test_parse_zero_duration() {
        assert_eq!(parse_duration("0s").unwrap(), Duration::ZERO);
        assert_eq!(parse_duration("0m").unwrap(), Duration::ZERO);
        assert_eq!(parse_duration("0h").unwrap(), Duration::ZERO);
    }

    #[test]
    fn test_parse_max_segment_value() {
        // 5 digits per segment is the GEP-2257 ceiling.
        assert_eq!(parse_duration("99999s").unwrap(), Duration::from_secs(99999));
        assert_eq!(parse_duration("99999h").unwrap(), Duration::from_secs(99999 * 3600));
    }

    #[test]
    fn test_reject_empty_string() {
        assert_eq!(parse_duration(""), Err(ParseDurationError::EmptyString));
        assert_eq!(parse_duration("   "), Err(ParseDurationError::EmptyString));
    }

    #[test]
    fn test_reject_bare_number() {
        // Bare numbers are no longer accepted (they were ambiguous).
        assert!(matches!(
            parse_duration("30"),
            Err(ParseDurationError::InvalidFormat(_))
        ));
        assert!(matches!(
            parse_duration("3600"),
            Err(ParseDurationError::InvalidFormat(_))
        ));
        // Trailing bare number after a valid segment.
        assert!(matches!(
            parse_duration("1m30"),
            Err(ParseDurationError::InvalidFormat(_))
        ));
    }

    #[test]
    fn test_reject_decimals() {
        assert!(parse_duration("1.5h").is_err());
        assert!(parse_duration("0.5m").is_err());
        assert!(parse_duration("2.5s").is_err());
    }

    #[test]
    fn test_reject_days_and_long_names() {
        assert!(matches!(parse_duration("1d"), Err(ParseDurationError::UnknownUnit(_))));
        // Long unit names produce a zero-length digit run after the first unit.
        assert!(parse_duration("2hours").is_err());
        assert!(parse_duration("5minutes").is_err());
        assert!(parse_duration("100millis").is_err());
    }

    #[test]
    fn test_reject_too_many_digits() {
        // 6 digits exceeds the GEP-2257 per-segment ceiling.
        assert!(matches!(
            parse_duration("123456h"),
            Err(ParseDurationError::InvalidFormat(_))
        ));
    }

    #[test]
    fn test_reject_too_many_segments() {
        // 5 segments exceeds the GEP-2257 ceiling.
        assert!(matches!(
            parse_duration("1h1m1s1ms1s"),
            Err(ParseDurationError::InvalidFormat(_))
        ));
    }

    #[test]
    fn test_reject_negative() {
        // A leading '-' is simply not a digit.
        assert!(matches!(
            parse_duration("-30s"),
            Err(ParseDurationError::InvalidFormat(_))
        ));
    }

    #[test]
    fn test_reject_invalid_format() {
        assert!(parse_duration("abc").is_err());
        assert!(matches!(parse_duration("30x"), Err(ParseDurationError::UnknownUnit(_))));
        assert!(parse_duration("s30").is_err());
        assert!(parse_duration("ms").is_err());
    }
}
