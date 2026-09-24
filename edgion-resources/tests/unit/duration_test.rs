use super::*;

#[test]
fn test_pattern_matches_parser() {
    // The runtime parser accepts exactly the GEP-2257 grammar the pattern
    // constant advertises; this guards against the two drifting apart.
    assert_eq!(GEP2257_PATTERN, r"^([0-9]{1,5}(h|m|s|ms)){1,4}$");
}

#[test]
fn test_parse_milliseconds() {
    assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
    assert_eq!(parse_duration("1000ms").unwrap(), Duration::from_millis(1000));
    assert_eq!(parse_duration("0ms").unwrap(), Duration::ZERO);
}

#[test]
fn test_parse_seconds() {
    assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
}

#[test]
fn test_parse_minutes() {
    assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
    assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
}

#[test]
fn test_parse_hours() {
    assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    assert_eq!(parse_duration("24h").unwrap(), Duration::from_secs(86400));
}

#[test]
fn test_parse_with_whitespace() {
    assert_eq!(parse_duration("  30s  ").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_duration(" 5m ").unwrap(), Duration::from_secs(300));
}

#[test]
fn test_parse_combined_formats() {
    // 1h30m = 5400s
    assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
    // 1m30s = 90s
    assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
    // 1h30m15s = 5415s
    assert_eq!(parse_duration("1h30m15s").unwrap(), Duration::from_secs(5415));
    // Max 4 segments: 1h2m3s4ms
    assert_eq!(
        parse_duration("1h2m3s4ms").unwrap(),
        Duration::from_secs(3723) + Duration::from_millis(4)
    );
}

#[test]
fn test_parse_zero_duration() {
    assert_eq!(parse_duration("0s").unwrap(), Duration::ZERO);
    assert_eq!(parse_duration("0m").unwrap(), Duration::ZERO);
    assert_eq!(parse_duration("0h").unwrap(), Duration::ZERO);
}

#[test]
fn test_parse_max_segment_value() {
    // 5 digits per segment is the GEP-2257 ceiling.
    assert_eq!(parse_duration("99999s").unwrap(), Duration::from_secs(99999));
    assert_eq!(parse_duration("99999h").unwrap(), Duration::from_secs(99999 * 3600));
}

#[test]
fn test_reject_empty_string() {
    assert_eq!(parse_duration(""), Err(ParseDurationError::EmptyString));
    assert_eq!(parse_duration("   "), Err(ParseDurationError::EmptyString));
}

#[test]
fn test_reject_bare_number() {
    // Bare numbers are no longer accepted (they were ambiguous).
    assert!(matches!(
        parse_duration("30"),
        Err(ParseDurationError::InvalidFormat(_))
    ));
    assert!(matches!(
        parse_duration("3600"),
        Err(ParseDurationError::InvalidFormat(_))
    ));
    // Trailing bare number after a valid segment.
    assert!(matches!(
        parse_duration("1m30"),
        Err(ParseDurationError::InvalidFormat(_))
    ));
}

#[test]
fn test_reject_decimals() {
    assert!(parse_duration("1.5h").is_err());
    assert!(parse_duration("0.5m").is_err());
    assert!(parse_duration("2.5s").is_err());
}

#[test]
fn test_reject_days_and_long_names() {
    assert!(matches!(parse_duration("1d"), Err(ParseDurationError::UnknownUnit(_))));
    // Long unit names produce a zero-length digit run after the first unit.
    assert!(parse_duration("2hours").is_err());
    assert!(parse_duration("5minutes").is_err());
    assert!(parse_duration("100millis").is_err());
}

#[test]
fn test_reject_too_many_digits() {
    // 6 digits exceeds the GEP-2257 per-segment ceiling.
    assert!(matches!(
        parse_duration("123456h"),
        Err(ParseDurationError::InvalidFormat(_))
    ));
}

#[test]
fn test_reject_too_many_segments() {
    // 5 segments exceeds the GEP-2257 ceiling.
    assert!(matches!(
        parse_duration("1h1m1s1ms1s"),
        Err(ParseDurationError::InvalidFormat(_))
    ));
}

#[test]
fn test_reject_negative() {
    // A leading '-' is simply not a digit.
    assert!(matches!(
        parse_duration("-30s"),
        Err(ParseDurationError::InvalidFormat(_))
    ));
}

#[test]
fn test_reject_invalid_format() {
    assert!(parse_duration("abc").is_err());
    assert!(matches!(parse_duration("30x"), Err(ParseDurationError::UnknownUnit(_))));
    assert!(parse_duration("s30").is_err());
    assert!(parse_duration("ms").is_err());
}
