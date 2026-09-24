//! Bounded inputs for the shared Redis rate-limit scripts.
//!
//! The bounds keep every integer intermediate used by the HTTP, stream and DSL
//! rate limiters exact in Redis Lua and representable by Redis/Rust integer
//! APIs. The sliding-window estimate itself remains a floating-point
//! approximation by design.

use std::time::Duration;

/// Largest integer exactly representable by Redis Lua's double precision.
pub const MAX_SAFE_LUA_INTEGER: u64 = (1_u64 << 53) - 1;
pub const MAX_RATE_LIMIT_UNITS: u32 = 1_000_000;
pub const MIN_RATE_LIMIT_WINDOW_MS: u64 = 100;
pub const MAX_RATE_LIMIT_WINDOW_MS: u64 = i32::MAX as u64;

/// Bound a rate or request cost before it enters rate-limit arithmetic.
pub fn validate_rate_limit_units(value: u64) -> Result<(), &'static str> {
    if value == 0 || value > u64::from(MAX_RATE_LIMIT_UNITS) {
        return Err("must be between 1 and 1000000");
    }
    Ok(())
}

/// Convert a rate-limit window to exact, bounded integer milliseconds.
pub fn validate_rate_limit_window(window: Duration) -> Result<i64, &'static str> {
    let millis = u64::try_from(window.as_millis()).map_err(|_| "milliseconds overflow")?;
    if !(MIN_RATE_LIMIT_WINDOW_MS..=MAX_RATE_LIMIT_WINDOW_MS).contains(&millis) {
        return Err("must be between 100ms and 2147483647ms");
    }
    if window != Duration::from_millis(millis) {
        return Err("must contain whole milliseconds");
    }
    i64::try_from(millis).map_err(|_| "milliseconds overflow")
}

/// Bound positive policy inputs and reject fractional milliseconds.
///
/// rate * window and cost * window stay below 2^51, double-window TTL below
/// 2^32, and rate + cost below 2^21. For a valid token bucket, elapsed < window
/// therefore has an exact integer product; elapsed >= window saturates the
/// bucket even when its finite product is rounded. This assumes Redis state
/// is written only by these scripts, with nonnegative timestamps/balances.
pub fn validate_rate_limit_policy(rate: u64, window: Duration) -> Result<i64, &'static str> {
    validate_rate_limit_units(rate).map_err(|_| "rate must be between 1 and 1000000")?;
    validate_rate_limit_window(window).map_err(|error| match error {
        "must be between 100ms and 2147483647ms" => "interval must be between 100ms and 2147483647ms",
        "must contain whole milliseconds" => "interval must contain whole milliseconds",
        _ => "interval milliseconds overflow",
    })
}

/// Validate request-dependent inputs before any Redis operation.
pub fn validate_rate_limit_call(cost: i64, now_ms: i64) -> Result<u32, &'static str> {
    let cost = u32::try_from(cost).map_err(|_| "invalid cost")?;
    validate_rate_limit_units(u64::from(cost)).map_err(|_| "invalid cost")?;
    if now_ms < 0 || now_ms as u64 > MAX_SAFE_LUA_INTEGER {
        return Err("invalid timestamp");
    }
    Ok(cost)
}

#[cfg(test)]
#[path = "../tests/unit/redis_rate_limit_test.rs"]
mod tests;
