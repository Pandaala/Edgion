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

#[test]
fn reset_timestamp_routing_matches_original_parser_chain() {
    fn original(value: &str, now: SystemTime) -> Option<Duration> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if let Ok(seconds) = value.parse::<u64>() {
            let reset = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))?;
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
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut cases: Vec<String> = [
        "",
        "0",
        "1700000030",
        "18446744073709551615",
        "18446744073709551616",
        "1h30m250ms",
        "0h0m0s0ms",
        "1s1s1s1s1s",
        "100000s",
        "1.5s",
        "-1s",
        "2023-11-14T22:13:50Z",
        "2023-11-14T22:13:50.123456789Z",
        "2023-11-14T23:13:50+01:00",
        "2023-11-14T21:13:50-01:00",
        "2023-11-14t22:13:50z",
        "2023-11-14T22:13:19Z",
        "1969-12-31T23:59:59Z",
        "2023-99-14T22:13:50Z",
        "2023-",
        "1234-5s",
        "éé-1s",
        "later",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for unit in ["h", "m", "s", "ms"] {
        for number in ["0", "1", "99999", "100000"] {
            cases.push(format!("{number}{unit}"));
        }
    }
    for byte in 0..=127u8 {
        cases.push(format!("1234{}1s", char::from(byte)));
        cases.push(format!("2023-11-14T22:13:50{}", char::from(byte)));
    }
    for value in cases {
        for input in [value.clone(), format!(" \t{value}\r\n")] {
            assert_eq!(parse_rate_limit_reset(&input, now), original(&input, now), "{input:?}");
        }
    }
}

#[test]
fn kubernetes_epoch_millis_floor_negative_fractions() {
    for (source, expected) in [
        ("1969-12-31T23:59:59.999999999Z", -1),
        ("1969-12-31T23:59:59.998999999Z", -2),
        ("1970-01-01T00:00:00Z", 0),
        ("1970-01-01T00:00:00.000000001Z", 0),
        ("1970-01-01T00:00:00.001Z", 1),
    ] {
        assert_eq!(kubernetes_timestamp_millis(source.parse().unwrap()), expected);
    }
    for timestamp in [Timestamp::MIN, Timestamp::MAX] {
        let expected = i64::try_from(timestamp.as_nanosecond().div_euclid(1_000_000)).unwrap();
        assert_eq!(kubernetes_timestamp_millis(timestamp), expected);
    }
}
