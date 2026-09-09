//! Bounded inputs for the shared Redis rate-limit scripts.
//!
//! Currently adopted by DSL policies. HTTP/Stream adoption and the stronger
//! all-intermediates-exact contract remain in the numeric-domain task.
//! The sliding-window algorithm remains a floating-point approximation.

use std::time::Duration;

/// Largest integer exactly representable by Redis Lua's double precision.
pub const MAX_SAFE_LUA_INTEGER: u64 = (1_u64 << 53) - 1;
pub const MAX_RATE_LIMIT_UNITS: u32 = 1_000_000;
pub const MIN_RATE_LIMIT_WINDOW_MS: u64 = 100;
pub const MAX_RATE_LIMIT_WINDOW_MS: u64 = i32::MAX as u64;

/// Bound positive policy inputs and reject fractional milliseconds.
///
/// rate * window and cost * window stay below 2^51, double-window TTL below
/// 2^32, and rate + cost below 2^21. For a valid token bucket, elapsed < window
/// therefore has an exact integer product; elapsed >= window saturates the
/// bucket even when its finite product is rounded. This assumes Redis state
/// is written only by these scripts, with nonnegative timestamps/balances.
pub fn validate_rate_limit_policy(rate: u64, window: Duration) -> Result<i64, &'static str> {
    if rate == 0 || rate > u64::from(MAX_RATE_LIMIT_UNITS) {
        return Err("rate must be between 1 and 1000000");
    }
    let millis = u64::try_from(window.as_millis()).map_err(|_| "interval milliseconds overflow")?;
    if !(MIN_RATE_LIMIT_WINDOW_MS..=MAX_RATE_LIMIT_WINDOW_MS).contains(&millis) {
        return Err("interval must be between 100ms and 2147483647ms");
    }
    if window != Duration::from_millis(millis) {
        return Err("interval must contain whole milliseconds");
    }
    i64::try_from(millis).map_err(|_| "interval milliseconds overflow")
}

/// Validate request-dependent inputs before any Redis operation.
pub fn validate_rate_limit_call(cost: i64, now_ms: i64) -> Result<u32, &'static str> {
    let cost = u32::try_from(cost).map_err(|_| "invalid cost")?;
    if cost == 0 || cost > MAX_RATE_LIMIT_UNITS {
        return Err("invalid cost");
    }
    if now_ms < 0 || now_ms as u64 > MAX_SAFE_LUA_INTEGER {
        return Err("invalid timestamp");
    }
    Ok(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_and_call_boundaries() {
        let max_window = Duration::from_millis(MAX_RATE_LIMIT_WINDOW_MS);
        assert_eq!(
            validate_rate_limit_policy(1_000_000, max_window),
            Ok(i64::from(i32::MAX))
        );
        assert!(validate_rate_limit_policy(1_000_001, max_window).is_err());
        assert!(validate_rate_limit_policy(0, max_window).is_err());
        for window in [
            Duration::ZERO,
            Duration::from_millis(99),
            max_window + Duration::from_millis(1),
            Duration::MAX,
        ] {
            assert!(validate_rate_limit_policy(1, window).is_err());
        }
        assert!(validate_rate_limit_policy(1, Duration::from_micros(100_001)).is_err());
        assert_eq!(validate_rate_limit_policy(1, Duration::from_millis(100)), Ok(100));
        assert_eq!(
            validate_rate_limit_call(1_000_000, MAX_SAFE_LUA_INTEGER as i64),
            Ok(1_000_000)
        );
        for cost in [i64::MIN, -1, 0, 1_000_001, i64::MAX] {
            assert!(validate_rate_limit_call(cost, 0).is_err());
        }
        assert!(validate_rate_limit_call(1, -1).is_err());
        assert!(validate_rate_limit_call(1, MAX_SAFE_LUA_INTEGER as i64 + 1).is_err());
        assert!(u64::from(MAX_RATE_LIMIT_UNITS) * MAX_RATE_LIMIT_WINDOW_MS < (1 << 51));
    }
}
