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

#[test]
fn rfc3339_deadline_is_strict_and_maps_past_to_zero() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    assert_eq!(
        parse_rfc3339_deadline("2023-11-14T22:13:50Z", now),
        Some(Duration::from_secs(30))
    );
    assert_eq!(
        parse_rfc3339_deadline("2023-11-14T22:12:50Z", now),
        Some(Duration::ZERO)
    );
    assert_eq!(parse_rfc3339_deadline("30s", now), None);
    assert_eq!(parse_rfc3339_deadline("1700000030", now), None);
}
