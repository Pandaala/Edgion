use super::*;

#[test]
fn parse_byte_size_units() {
    assert_eq!(parse_byte_size("1024"), Some(1024));
    assert_eq!(parse_byte_size("1k"), Some(1024));
    assert_eq!(parse_byte_size("1kb"), Some(1024));
    assert_eq!(parse_byte_size("1KiB"), Some(1024));
    assert_eq!(parse_byte_size("1m"), Some(1024 * 1024));
    assert_eq!(parse_byte_size("64M"), Some(64 * 1024 * 1024));
    assert_eq!(parse_byte_size("1g"), Some(1024 * 1024 * 1024));
    assert_eq!(parse_byte_size("1.5m"), Some(1024 * 1024 + 512 * 1024));
    assert_eq!(parse_byte_size("512b"), Some(512));
    assert_eq!(parse_byte_size(""), None);
    assert_eq!(parse_byte_size("abc"), None);
    assert_eq!(parse_byte_size("-1m"), None);
}

#[test]
fn parse_byte_size_saturates_and_rejects_non_finite() {
    // Overflow saturates to u64::MAX rather than panicking (float→int cast).
    assert_eq!(parse_byte_size("99999999999999999999999g"), Some(u64::MAX));
    // Non-finite is rejected (guards against a NaN/inf cast).
    assert_eq!(parse_byte_size("1e400"), None);
}
