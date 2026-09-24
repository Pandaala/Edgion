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
#[path = "../../../tests/unit/routes/grpc/grpc_timeout_test.rs"]
mod tests;
