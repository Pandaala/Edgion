//! Time / timestamp formatting helpers shared across control-plane code.

use chrono::{SecondsFormat, Utc};
use std::time::{Duration, SystemTime};

/// Current wall-clock time as an RFC3339 UTC string at **second** precision with a
/// `Z` suffix (e.g. `2026-07-21T10:00:00Z`).
///
/// This is the canonical shape for every operator-/K8s-facing timestamp: status
/// `lastTransitionTime`, ACME `lastRenewalTime` / `lastFailureTime`, the
/// `EndpointSlice` `resolved_at` annotation, and so on. It matches how Kubernetes
/// `metav1.Time` — the type behind Gateway API `Condition.LastTransitionTime` —
/// serializes (`t.UTC().Format(time.RFC3339)`), so control-plane output is shaped
/// identically to K8s-native objects.
///
/// Do NOT hand-stamp `Utc::now().to_rfc3339()` (nanosecond precision + `+00:00`
/// offset) for these fields: it is still valid RFC3339, but no K8s object looks like
/// that, and mixing the two shapes on one field breaks string-diff / dedup logic
/// that assumes a single form. For hot-path wire-/access-log timestamps use epoch
/// millis instead. See `skills/03-coding/03-time-conventions.md`.
pub fn now_rfc3339() -> String {
    format_rfc3339(SystemTime::now())
}

/// Format an arbitrary wall-clock instant in the canonical Kubernetes-facing
/// RFC3339 UTC seconds shape.
pub fn format_rfc3339(time: SystemTime) -> String {
    chrono::DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parse an HTTP `Retry-After` value as delta-seconds or an HTTP date relative
/// to the supplied wall clock. Invalid values return `None`; zero and past
/// values produce `Duration::ZERO` so callers can apply their own policy.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let when = httpdate::parse_http_date(value).ok()?;
    Some(when.duration_since(now).unwrap_or(Duration::ZERO))
}

/// Parse a provider rate-limit reset hint relative to `now`.
///
/// Supported wire forms are the bounded Edgion/Gateway API duration grammar
/// used by OpenAI-style `x-ratelimit-reset-*` headers, an absolute Unix epoch
/// second, or an absolute RFC3339 timestamp. Invalid and overflowing values
/// return `None`; RFC3339 timestamps before the Unix epoch also return `None`.
/// Supported past absolute timestamps return zero so callers can reject them
/// without treating malformed data as a cooldown.
pub fn parse_rate_limit_reset(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(epoch_seconds) = value.parse::<u64>() {
        let reset = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(epoch_seconds))?;
        return Some(reset.duration_since(now).unwrap_or(Duration::ZERO));
    }
    if let Ok(duration) = edgion_resources::duration::parse_duration(value) {
        return Some(duration);
    }
    let timestamp = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    let seconds = u64::try_from(timestamp.timestamp()).ok()?;
    let reset = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))?
        .checked_add(Duration::from_nanos(u64::from(timestamp.timestamp_subsec_nanos())))?;
    Some(reset.duration_since(now).unwrap_or(Duration::ZERO))
}

/// Parse one strict RFC3339 absolute deadline relative to `now`.
///
/// Unlike [`parse_rate_limit_reset`], this accepts no duration or epoch-number
/// alternatives. Past deadlines return `Duration::ZERO`; malformed or
/// unrepresentable timestamps and timestamps before the Unix epoch return `None`.
pub fn parse_rfc3339_deadline(value: &str, now: SystemTime) -> Option<Duration> {
    let timestamp = chrono::DateTime::parse_from_rfc3339(value.trim()).ok()?;
    let seconds = u64::try_from(timestamp.timestamp()).ok()?;
    let deadline = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))?
        .checked_add(Duration::from_nanos(u64::from(timestamp.timestamp_subsec_nanos())))?;
    Some(deadline.duration_since(now).unwrap_or(Duration::ZERO))
}

#[cfg(test)]
#[path = "../tests/unit/time_test.rs"]
mod tests;
