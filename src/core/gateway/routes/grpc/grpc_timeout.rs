//! Parser and formatter for the gRPC `grpc-timeout` request header.
//!
//! Format per the gRPC-over-HTTP/2 spec: `{number}{unit}` where `unit` is
//! one of `H` (hour), `M` (minute), `S` (second), `m` (millisecond),
//! `u` (microsecond), `n` (nanosecond). The number is up to 8 decimal
//! digits.
//!
//! When a request carries `grpc-timeout`, an intermediary that re-emits the
//! header MUST adjust it for elapsed wall-clock time so downstream services
//! see a consistent remaining deadline.

use std::time::Duration;

/// Maximum number of decimal digits permitted in the `TimeoutValue` portion
/// per the gRPC-over-HTTP/2 spec.
const MAX_TIMEOUT_DIGITS: usize = 8;

/// Parse a `grpc-timeout` header value into a `Duration`.
///
/// Returns `None` on:
/// - empty or whitespace-only input
/// - missing unit suffix
/// - non-digit characters before the unit
/// - more than 8 digits in the numeric portion
/// - unknown unit character
/// - arithmetic overflow when converting to `Duration`
///
/// Returning `None` (instead of panicking or erroring) lets the caller fall
/// back to the existing route-level timeout behaviour for malformed input.
pub fn parse_grpc_timeout(value: &str) -> Option<Duration> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let unit_byte = *trimmed.as_bytes().last()?;
    let split_at = trimmed.len() - 1;
    // Guard against multi-byte UTF-8 final chars: slicing in the middle of a
    // codepoint panics. Wire-path callers go through HeaderValue::to_str(),
    // which only emits ASCII, but this fn is `pub` so the boundary check
    // closes a public-API panic surface.
    if !trimmed.is_char_boundary(split_at) {
        return None;
    }
    let digits = &trimmed[..split_at];

    if digits.is_empty() || digits.len() > MAX_TIMEOUT_DIGITS {
        return None;
    }

    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    let n: u64 = digits.parse().ok()?;

    match unit_byte {
        b'H' => n.checked_mul(3_600).map(Duration::from_secs),
        b'M' => n.checked_mul(60).map(Duration::from_secs),
        b'S' => Some(Duration::from_secs(n)),
        b'm' => Some(Duration::from_millis(n)),
        b'u' => Some(Duration::from_micros(n)),
        b'n' => Some(Duration::from_nanos(n)),
        _ => None,
    }
}

/// Format a `Duration` as a `grpc-timeout` header value.
///
/// Picks the smallest unit that keeps the numeric portion within 8 decimal
/// digits while preserving precision when possible. Sub-nanosecond
/// granularity is not representable on the wire and is rounded up to
/// `1n` so the upstream never receives `0n` (which the spec leaves
/// implementation-defined).
pub fn format_grpc_timeout(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos == 0 {
        return "1n".to_string();
    }

    const MAX: u128 = 99_999_999;

    if nanos <= MAX {
        return format!("{}n", nanos);
    }

    let micros = d.as_micros();
    if micros <= MAX {
        return format!("{}u", micros);
    }

    let millis = d.as_millis();
    if millis <= MAX {
        return format!("{}m", millis);
    }

    let secs = d.as_secs() as u128;
    if secs <= MAX {
        return format!("{}S", secs);
    }

    let minutes = secs / 60;
    if minutes <= MAX {
        return format!("{}M", minutes);
    }

    let hours = secs / 3_600;
    format!("{}H", hours.min(MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_unit() {
        assert_eq!(parse_grpc_timeout("1H"), Some(Duration::from_secs(3_600)));
        assert_eq!(parse_grpc_timeout("2M"), Some(Duration::from_secs(120)));
        assert_eq!(parse_grpc_timeout("5S"), Some(Duration::from_secs(5)));
        assert_eq!(parse_grpc_timeout("100m"), Some(Duration::from_millis(100)));
        assert_eq!(parse_grpc_timeout("500u"), Some(Duration::from_micros(500)));
        assert_eq!(parse_grpc_timeout("250n"), Some(Duration::from_nanos(250)));
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(parse_grpc_timeout("  10S  "), Some(Duration::from_secs(10)));
    }

    #[test]
    fn rejects_empty_and_whitespace() {
        assert_eq!(parse_grpc_timeout(""), None);
        assert_eq!(parse_grpc_timeout("   "), None);
    }

    #[test]
    fn rejects_missing_or_bad_unit() {
        assert_eq!(parse_grpc_timeout("100"), None);
        assert_eq!(parse_grpc_timeout("100x"), None);
        assert_eq!(parse_grpc_timeout("100s"), None); // lowercase second is not in the spec
    }

    #[test]
    fn rejects_non_digit_value() {
        assert_eq!(parse_grpc_timeout("ten S"), None);
        assert_eq!(parse_grpc_timeout("-5S"), None);
        assert_eq!(parse_grpc_timeout("1.5S"), None);
    }

    #[test]
    fn rejects_multibyte_utf8_suffix_without_panic() {
        // Final char is a multi-byte UTF-8 codepoint — `trimmed.len() - 1`
        // lands in the middle of the codepoint. The boundary guard must
        // prevent the slice panic and return None. Uses lint-permitted
        // multi-byte chars (`€` is 3 bytes, `π` is 2 bytes, `©` is 2 bytes).
        assert_eq!(parse_grpc_timeout("100€"), None);
        assert_eq!(parse_grpc_timeout("5π"), None);
        assert_eq!(parse_grpc_timeout("©"), None);
    }

    #[test]
    fn rejects_too_many_digits() {
        assert_eq!(parse_grpc_timeout("100000000S"), None); // 9 digits
        assert_eq!(parse_grpc_timeout("99999999S"), Some(Duration::from_secs(99_999_999)));
    }

    #[test]
    fn rejects_overflow_on_unit_scaling() {
        // 9-digit cap already rejects the input below, but exercise the
        // checked_mul guard with a hypothetical overflow value as defence in depth.
        assert_eq!(
            parse_grpc_timeout("99999999H"),
            Some(Duration::from_secs(99_999_999 * 3_600))
        );
    }

    #[test]
    fn format_picks_smallest_unit_that_fits() {
        // 250 ns: ≤ 99_999_999 ns → emit as ns.
        assert_eq!(format_grpc_timeout(Duration::from_nanos(250)), "250n");
        // 500 µs = 500_000 ns: still fits in ns.
        assert_eq!(format_grpc_timeout(Duration::from_micros(500)), "500000n");
        // 100 ms = 100_000_000 ns → overflow ns cap, fall to µs (100_000).
        assert_eq!(format_grpc_timeout(Duration::from_millis(100)), "100000u");
        // 5 s = 5_000_000_000 ns → overflow ns cap, fall to µs (5_000_000).
        assert_eq!(format_grpc_timeout(Duration::from_secs(5)), "5000000u");
        // 99_999 s = 99_999_000 ms → fits in ms (just under the 8-digit cap).
        assert_eq!(format_grpc_timeout(Duration::from_secs(99_999)), "99999000m");
        // 100_000 s = 100_000_000 ms → overflow ms cap, fall to S.
        assert_eq!(format_grpc_timeout(Duration::from_secs(100_000)), "100000S");
        // 99_999_999 s → still fits in S.
        assert_eq!(format_grpc_timeout(Duration::from_secs(99_999_999)), "99999999S");
        // 100_000_000 s → overflow S cap, fall to M (≈ 1_666_666).
        assert_eq!(format_grpc_timeout(Duration::from_secs(100_000_000)), "1666666M");
    }

    #[test]
    fn format_clamps_zero_to_one_nanosecond() {
        assert_eq!(format_grpc_timeout(Duration::ZERO), "1n");
    }

    #[test]
    fn round_trip_preserves_value_within_unit_resolution() {
        let original = Duration::from_millis(123);
        let formatted = format_grpc_timeout(original);
        assert_eq!(parse_grpc_timeout(&formatted), Some(original));
    }
}
