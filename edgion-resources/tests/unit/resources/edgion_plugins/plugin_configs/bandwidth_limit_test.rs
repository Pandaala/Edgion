use super::*;

#[test]
fn test_parse_bandwidth_rate_pure_number() {
    assert_eq!(parse_bandwidth_rate("1024"), Some(1024));
    assert_eq!(parse_bandwidth_rate("0"), Some(0));
    assert_eq!(parse_bandwidth_rate("100"), Some(100));
}

#[test]
fn test_parse_bandwidth_rate_bytes() {
    assert_eq!(parse_bandwidth_rate("1024b"), Some(1024));
    assert_eq!(parse_bandwidth_rate("512B"), Some(512));
}

#[test]
fn test_parse_bandwidth_rate_kilobytes() {
    assert_eq!(parse_bandwidth_rate("1kb"), Some(1024));
    assert_eq!(parse_bandwidth_rate("512kb"), Some(524288));
    assert_eq!(parse_bandwidth_rate("512KB"), Some(524288));
}

#[test]
fn test_parse_bandwidth_rate_megabytes() {
    assert_eq!(parse_bandwidth_rate("1mb"), Some(1048576));
    assert_eq!(parse_bandwidth_rate("10mb"), Some(10485760));
    assert_eq!(parse_bandwidth_rate("1MB"), Some(1048576));
}

#[test]
fn test_parse_bandwidth_rate_gigabytes() {
    assert_eq!(parse_bandwidth_rate("1gb"), Some(1073741824));
    assert_eq!(parse_bandwidth_rate("1GB"), Some(1073741824));
}

#[test]
fn test_parse_bandwidth_rate_decimal() {
    assert_eq!(parse_bandwidth_rate("1.5mb"), Some(1572864));
    assert_eq!(parse_bandwidth_rate("0.5kb"), Some(512));
    assert_eq!(parse_bandwidth_rate("2.5gb"), Some(2684354560));
}

#[test]
fn test_parse_bandwidth_rate_whitespace() {
    assert_eq!(parse_bandwidth_rate(" 1mb "), Some(1048576));
    assert_eq!(parse_bandwidth_rate("  512kb  "), Some(524288));
}

#[test]
fn test_parse_bandwidth_rate_invalid() {
    assert_eq!(parse_bandwidth_rate(""), None);
    assert_eq!(parse_bandwidth_rate("abc"), None);
    assert_eq!(parse_bandwidth_rate("mb"), None);
    assert_eq!(parse_bandwidth_rate("-1mb"), None);
}

#[test]
fn test_config_validation() {
    let valid = BandwidthLimitConfig {
        rate: "1mb".to_string(),
        rate_bytes_per_second: None,
    };
    assert!(valid.get_validation_error().is_none());

    let empty = BandwidthLimitConfig {
        rate: "".to_string(),
        rate_bytes_per_second: None,
    };
    assert_eq!(empty.get_validation_error(), Some("rate is required"));

    let zero = BandwidthLimitConfig {
        rate: "0".to_string(),
        rate_bytes_per_second: None,
    };
    assert_eq!(zero.get_validation_error(), Some("rate must be greater than 0"));

    let invalid = BandwidthLimitConfig {
        rate: "abc".to_string(),
        rate_bytes_per_second: None,
    };
    assert_eq!(
        invalid.get_validation_error(),
        Some("invalid rate format, expected: '1mb', '512kb', '1024', etc.")
    );
}
