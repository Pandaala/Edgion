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
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
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
/// return `None`; past absolute timestamps return zero so callers can reject
/// them without treating malformed data as a cooldown.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_rfc3339_is_second_precision_utc_z() {
        let s = now_rfc3339();
        // Z-suffixed, no numeric offset, no sub-second field — the metav1.Time shape.
        assert!(s.ends_with('Z'), "expected Z suffix: {s}");
        assert!(!s.contains('+'), "expected no numeric offset: {s}");
        assert!(!s.contains('.'), "expected no sub-second field: {s}");
        // Round-trips as RFC3339.
        assert!(
            chrono::DateTime::parse_from_rfc3339(&s).is_ok(),
            "not parseable as RFC3339: {s}"
        );
    }

    #[test]
    fn retry_after_accepts_delta_and_http_date() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(parse_retry_after(" 12 ", now), Some(Duration::from_secs(12)));
        let future = httpdate::fmt_http_date(now + Duration::from_secs(30));
        assert_eq!(parse_retry_after(&future, now), Some(Duration::from_secs(30)));
    }

    #[test]
    fn retry_after_maps_past_to_zero_and_rejects_invalid() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let past = httpdate::fmt_http_date(now - Duration::from_secs(30));
        assert_eq!(parse_retry_after(&past, now), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("later", now), None);
    }

    #[test]
    fn rate_limit_reset_accepts_delta_and_absolute_forms() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(
            parse_rate_limit_reset("1m250ms", now),
            Some(Duration::from_millis(60_250))
        );
        assert_eq!(parse_rate_limit_reset("1700000030", now), Some(Duration::from_secs(30)));
        assert_eq!(
            parse_rate_limit_reset("2023-11-14T22:13:50Z", now),
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn rate_limit_reset_rejects_malformed_and_maps_past_to_zero() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(parse_rate_limit_reset("1699999999", now), Some(Duration::ZERO));
        assert_eq!(parse_rate_limit_reset("1.5s", now), None);
        assert_eq!(parse_rate_limit_reset("later", now), None);
    }
}
