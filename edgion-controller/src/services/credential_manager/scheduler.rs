//! Pure scheduling policy for controller-managed credentials.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

pub const EXPIRY_MARGIN: Duration = Duration::from_secs(30);
pub const MIN_USABILITY: Duration = Duration::from_secs(10);

/// Return the consumer-visible lifetime after reserving the fixed safety margin.
pub fn usable_lifetime(provider_lifetime: Duration) -> Option<Duration> {
    let usable = provider_lifetime.checked_sub(EXPIRY_MARGIN)?;
    (usable >= MIN_USABILITY).then_some(usable)
}

/// Schedule the next normal acquisition at the earlier of the configured
/// rotation interval and token half-life, then apply stable earlier-only jitter.
pub fn refresh_delay(source_identity: &str, provider_lifetime: Duration, rotation: Duration) -> Duration {
    let base = rotation.min(provider_lifetime / 2);
    earlier_jitter(source_identity, base)
}

/// Exponential retry bounded by the source maximum. A valid Retry-After may
/// postpone a retry, but never beyond that same operator-controlled maximum.
pub fn retry_delay(
    source_identity: &str,
    failure_count: u32,
    initial: Duration,
    maximum: Duration,
    retry_after: Option<Duration>,
) -> Duration {
    let shift = failure_count.saturating_sub(1).min(31);
    let factor = 1_u32 << shift;
    let exponential = initial.checked_mul(factor).unwrap_or(maximum).min(maximum);
    let half = exponential / 2;
    let mut hasher = DefaultHasher::new();
    source_identity.hash(&mut hasher);
    failure_count.hash(&mut hasher);
    let window_nanos = exponential.saturating_sub(half).as_nanos();
    let jitter_nanos = if window_nanos == 0 {
        0
    } else {
        hasher.finish() as u128 % (window_nanos + 1)
    };
    let jittered = half.saturating_add(Duration::from_nanos(jitter_nanos.min(u64::MAX as u128) as u64));
    retry_after
        .map(|delay| jittered.max(delay.min(maximum)))
        .unwrap_or(jittered)
}

fn earlier_jitter(identity: &str, base: Duration) -> Duration {
    let window = (base / 10).min(Duration::from_secs(30));
    if window.is_zero() {
        return base;
    }
    let mut hasher = DefaultHasher::new();
    identity.hash(&mut hasher);
    let jitter_nanos = hasher.finish() as u128 % (window.as_nanos() + 1);
    base.saturating_sub(Duration::from_nanos(jitter_nanos.min(u64::MAX as u128) as u64))
}

#[cfg(test)]
#[path = "../../../tests/unit/services/credential_manager/scheduler_test.rs"]
mod tests;
