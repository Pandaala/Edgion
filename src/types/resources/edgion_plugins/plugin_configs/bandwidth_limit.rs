//! BandwidthLimit plugin configuration
//!
//! Controls downstream response bandwidth by throttling body chunk delivery.
//! Uses Pingora's upstream_response_body_filter return value (Option<Duration>)
//! to delay chunk transmission, achieving bandwidth limiting.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Bandwidth limit configuration
///
/// ## YAML Examples
///
/// ```yaml
/// # Limit to 1MB/s
/// type: BandwidthLimit
/// config:
///   rate: "1mb"
///
/// # Limit to 512KB/s
/// type: BandwidthLimit
/// config:
///   rate: "512kb"
///
/// # Limit to 100 bytes/s (exact)
/// type: BandwidthLimit
/// config:
///   rate: "100"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BandwidthLimitConfig {
    /// Bandwidth rate limit
    ///
    /// Supports human-readable formats:
    /// - Bytes: "1024" or "1024b" (1024 bytes/s)
    /// - Kilobytes: "512kb" (512 * 1024 bytes/s)
    /// - Megabytes: "1mb" (1 * 1024 * 1024 bytes/s)
    /// - Gigabytes: "1gb" (1 * 1024 * 1024 * 1024 bytes/s)
    ///
    /// Case insensitive. Decimals supported: "1.5mb" = 1.5 * 1024 * 1024 bytes/s
    pub rate: String,

    /// Parsed rate in bytes per second (runtime only, not serialized)
    #[serde(skip)]
    #[schemars(skip)]
    pub rate_bytes_per_second: Option<u64>,
}

impl BandwidthLimitConfig {
    /// Parse the rate string into bytes per second
    ///
    /// Returns None if the rate string is invalid
    pub fn parse_rate(&self) -> Option<u64> {
        parse_bandwidth_rate(&self.rate)
    }

    /// Get validation error if config is invalid
    pub fn get_validation_error(&self) -> Option<&'static str> {
        if self.rate.is_empty() {
            return Some("rate is required");
        }
        match parse_bandwidth_rate(&self.rate) {
            Some(0) => Some("rate must be greater than 0"),
            Some(_) => None,
            None => Some("invalid rate format, expected: '1mb', '512kb', '1024', etc."),
        }
    }
}

/// Parse a bandwidth rate string into bytes per second
///
/// Supported formats:
/// - Pure number: "1024" -> 1024 bytes/s
/// - With 'b' suffix: "1024b" -> 1024 bytes/s
/// - With 'kb' suffix: "512kb" -> 524288 bytes/s
/// - With 'mb' suffix: "1mb" -> 1048576 bytes/s
/// - With 'gb' suffix: "1gb" -> 1073741824 bytes/s
///
/// Decimals are supported: "1.5mb" -> 1572864 bytes/s
/// Case insensitive.
pub fn parse_bandwidth_rate(rate: &str) -> Option<u64> {
    let rate = rate.trim().to_lowercase();
    if rate.is_empty() {
        return None;
    }

    // Try to split into number and unit
    let (num_str, multiplier) = if rate.ends_with("gb") {
        (&rate[..rate.len() - 2], 1024u64 * 1024 * 1024)
    } else if rate.ends_with("mb") {
        (&rate[..rate.len() - 2], 1024u64 * 1024)
    } else if rate.ends_with("kb") {
        (&rate[..rate.len() - 2], 1024u64)
    } else if rate.ends_with('b') {
        (&rate[..rate.len() - 1], 1u64)
    } else {
        // Pure number (bytes)
        (rate.as_str(), 1u64)
    };

    let num_str = num_str.trim();
    if num_str.is_empty() {
        return None;
    }

    // Parse as f64 to support decimals like "1.5mb"
    let num: f64 = num_str.parse().ok()?;
    if num < 0.0 || !num.is_finite() {
        return None;
    }

    Some((num * multiplier as f64) as u64)
}

#[cfg(test)]
mod tests {
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
}
