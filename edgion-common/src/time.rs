//! Time / timestamp formatting helpers shared across control-plane code.

use chrono::{SecondsFormat, Utc};

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
}
