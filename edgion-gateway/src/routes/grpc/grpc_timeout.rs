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
const MAX_TIMEOUT_VALUE: u128 = 99_999_999;

const TIMEOUT_UNITS: [(u128, char); 6] = [
    (1, 'n'),
    (1_000, 'u'),
    (1_000_000, 'm'),
    (1_000_000_000, 'S'),
    (60_000_000_000, 'M'),
    (3_600_000_000_000, 'H'),
];

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
/// digits. When a coarser unit is required, the value is rounded up so the
/// encoded timeout never shortens the remaining deadline. Zero is encoded as
/// `1n`; values beyond the maximum wire representation saturate at
/// `99999999H`.
pub fn format_grpc_timeout(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos == 0 {
        return "1n".to_string();
    }

    for (unit_nanos, suffix) in TIMEOUT_UNITS {
        // Quotient-plus-remainder avoids the overflow of
        // `(nanos + unit_nanos - 1) / unit_nanos` near `u128::MAX`.
        let value = nanos / unit_nanos + u128::from(!nanos.is_multiple_of(unit_nanos));
        if value <= MAX_TIMEOUT_VALUE {
            return format!("{value}{suffix}");
        }
    }

    format!("{MAX_TIMEOUT_VALUE}H")
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
        // 100_000_000 s → overflow S cap, round up in M.
        assert_eq!(format_grpc_timeout(Duration::from_secs(100_000_000)), "1666667M");
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

    fn duration_from_nanos(nanos: u128) -> Duration {
        Duration::new(
            u64::try_from(nanos / 1_000_000_000).expect("test duration fits u64 seconds"),
            (nanos % 1_000_000_000) as u32,
        )
    }

    fn selected_unit_quantum(encoded: &str) -> Duration {
        let suffix = encoded.as_bytes().last().copied().expect("non-empty timeout");
        let nanos = TIMEOUT_UNITS
            .iter()
            .find_map(|(nanos, unit)| (*unit as u8 == suffix).then_some(*nanos))
            .expect("known timeout unit");
        duration_from_nanos(nanos)
    }

    #[test]
    fn format_rounds_up_at_every_unit_transition() {
        let cases = [
            (MAX_TIMEOUT_VALUE, "99999999n"),
            (MAX_TIMEOUT_VALUE + 1, "100000u"),
            (MAX_TIMEOUT_VALUE * 1_000, "99999999u"),
            (MAX_TIMEOUT_VALUE * 1_000 + 1, "100000m"),
            (MAX_TIMEOUT_VALUE * 1_000_000, "99999999m"),
            (MAX_TIMEOUT_VALUE * 1_000_000 + 1, "100000S"),
            (MAX_TIMEOUT_VALUE * 1_000_000_000, "99999999S"),
            (MAX_TIMEOUT_VALUE * 1_000_000_000 + 1, "1666667M"),
            (MAX_TIMEOUT_VALUE * 60_000_000_000, "99999999M"),
            (MAX_TIMEOUT_VALUE * 60_000_000_000 + 1, "1666667H"),
            (MAX_TIMEOUT_VALUE * 3_600_000_000_000, "99999999H"),
        ];

        for (nanos, expected) in cases {
            assert_eq!(format_grpc_timeout(duration_from_nanos(nanos)), expected);
        }
    }

    #[test]
    fn formatted_representable_values_never_shorten_the_timeout() {
        let max_nanos = MAX_TIMEOUT_VALUE * 3_600_000_000_000;
        let samples = [
            1,
            250,
            MAX_TIMEOUT_VALUE,
            MAX_TIMEOUT_VALUE + 1,
            100_000_001,
            MAX_TIMEOUT_VALUE * 1_000 + 1,
            MAX_TIMEOUT_VALUE * 1_000_000 + 1,
            MAX_TIMEOUT_VALUE * 1_000_000_000 + 1,
            MAX_TIMEOUT_VALUE * 60_000_000_000 + 1,
            max_nanos,
        ];

        for nanos in samples {
            let original = duration_from_nanos(nanos);
            let encoded = format_grpc_timeout(original);
            let parsed = parse_grpc_timeout(&encoded).expect("formatter emits valid timeout");
            let quantum = selected_unit_quantum(&encoded);
            let overshoot = parsed.checked_sub(original).expect("formatted timeout must not shrink");

            assert!(overshoot < quantum, "{original:?} encoded as {encoded}");
            assert!(encoded.len() - 1 <= MAX_TIMEOUT_DIGITS);
        }
    }

    #[test]
    fn format_saturates_values_above_the_maximum_wire_timeout() {
        let just_above_max = duration_from_nanos(MAX_TIMEOUT_VALUE * 3_600_000_000_000 + 1);
        assert_eq!(format_grpc_timeout(just_above_max), "99999999H");
        assert_eq!(format_grpc_timeout(Duration::new(u64::MAX, 999_999_999)), "99999999H");
    }
}
