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
#[path = "../tests/unit/duration_test.rs"]
mod tests;
