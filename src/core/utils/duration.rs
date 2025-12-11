//! Duration parsing utilities
//!
//! This module provides functionality to parse duration strings into `std::time::Duration`.
//! Supports various time units and formats commonly used in configuration files.

use std::time::Duration;
use thiserror::Error;

/// Error type for duration parsing failures
#[derive(Error, Debug, Clone, PartialEq)]
pub enum ParseDurationError {
    #[error("empty duration string")]
    EmptyString,
    
    #[error("invalid duration format: {0}")]
    InvalidFormat(String),
    
    #[error("invalid number: {0}")]
    InvalidNumber(String),
    
    #[error("unknown unit: {0}")]
    UnknownUnit(String),
    
    #[error("negative duration not allowed")]
    NegativeDuration,
}

/// Parse a duration string into a `Duration`.
///
/// Supported formats:
/// - Pure numbers (default to seconds): "30" -> 30 seconds
/// - With units:
///   - "ms" or "millis" for milliseconds: "500ms"
///   - "s" or "sec" or "secs" for seconds: "30s"
///   - "m" or "min" or "mins" for minutes: "5m"
///   - "h" or "hour" or "hours" for hours: "1h"
/// - Combined formats: "1h30m", "1m30s", "1h30m15s"
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use edgion::core::utils::parse_duration;
///
/// assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
/// assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
/// assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
/// assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
/// assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
/// ```
pub fn parse_duration(s: &str) -> Result<Duration, ParseDurationError> {
    let s = s.trim();
    
    if s.is_empty() {
        return Err(ParseDurationError::EmptyString);
    }
    
    // Try to parse as combined format first (e.g., "1h30m", "1m30s")
    if let Ok(duration) = parse_combined_duration(s) {
        return Ok(duration);
    }
    
    // Try to parse as single unit format
    parse_single_duration(s)
}

/// Parse a single duration component (e.g., "30s", "5m", "500ms")
fn parse_single_duration(s: &str) -> Result<Duration, ParseDurationError> {
    let s = s.trim();
    
    // Check for negative sign first
    if s.starts_with('-') {
        return Err(ParseDurationError::NegativeDuration);
    }
    
    // Find where the number ends and unit begins
    let split_pos = s
        .chars()
        .position(|c| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    
    let (num_str, unit_str) = s.split_at(split_pos);
    
    if num_str.is_empty() {
        return Err(ParseDurationError::InvalidFormat(s.to_string()));
    }
    
    // Parse the number
    let num: f64 = num_str.parse()
        .map_err(|_| ParseDurationError::InvalidNumber(num_str.to_string()))?;
    
    if num < 0.0 {
        return Err(ParseDurationError::NegativeDuration);
    }
    
    // If no unit, default to seconds
    let unit = unit_str.trim();
    if unit.is_empty() {
        return Ok(Duration::from_secs_f64(num));
    }
    
    // Parse the unit
    let duration = match unit {
        "ms" | "millis" | "millisecond" | "milliseconds" => {
            Duration::from_millis(num as u64)
        }
        "s" | "sec" | "secs" | "second" | "seconds" => {
            Duration::from_secs_f64(num)
        }
        "m" | "min" | "mins" | "minute" | "minutes" => {
            Duration::from_secs_f64(num * 60.0)
        }
        "h" | "hr" | "hrs" | "hour" | "hours" => {
            Duration::from_secs_f64(num * 3600.0)
        }
        _ => {
            return Err(ParseDurationError::UnknownUnit(unit.to_string()));
        }
    };
    
    Ok(duration)
}

/// Parse a combined duration format (e.g., "1h30m", "1m30s", "1h30m15s")
fn parse_combined_duration(s: &str) -> Result<Duration, ParseDurationError> {
    let s = s.trim();
    let mut total = Duration::ZERO;
    let mut current_num = String::new();
    let mut i = 0;
    let chars: Vec<char> = s.chars().collect();
    
    while i < chars.len() {
        let c = chars[i];
        
        if c.is_ascii_digit() || c == '.' {
            current_num.push(c);
            i += 1;
        } else if c.is_alphabetic() {
            if current_num.is_empty() {
                return Err(ParseDurationError::InvalidFormat(s.to_string()));
            }
            
            // Find the complete unit (could be multiple chars)
            let mut unit = String::new();
            while i < chars.len() && chars[i].is_alphabetic() {
                unit.push(chars[i]);
                i += 1;
            }
            
            // Parse this component
            let component_str = format!("{}{}", current_num, unit);
            let component_duration = parse_single_duration(&component_str)?;
            total += component_duration;
            
            current_num.clear();
        } else if c.is_whitespace() {
            i += 1;
        } else {
            return Err(ParseDurationError::InvalidFormat(s.to_string()));
        }
    }
    
    // If there's leftover number without unit, it's an error for combined format
    if !current_num.is_empty() {
        return Err(ParseDurationError::InvalidFormat(s.to_string()));
    }
    
    if total.is_zero() {
        return Err(ParseDurationError::InvalidFormat(s.to_string()));
    }
    
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_pure_number_defaults_to_seconds() {
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("0").unwrap(), Duration::from_secs(0));
        assert_eq!(parse_duration("1").unwrap(), Duration::from_secs(1));
        assert_eq!(parse_duration("3600").unwrap(), Duration::from_secs(3600));
    }
    
    #[test]
    fn test_parse_with_whitespace() {
        assert_eq!(parse_duration("  30s  ").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration(" 5m ").unwrap(), Duration::from_secs(300));
    }
    
    #[test]
    fn test_parse_milliseconds() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("1000ms").unwrap(), Duration::from_millis(1000));
        assert_eq!(parse_duration("100millis").unwrap(), Duration::from_millis(100));
    }
    
    #[test]
    fn test_parse_seconds() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("60sec").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration("5secs").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("1second").unwrap(), Duration::from_secs(1));
    }
    
    #[test]
    fn test_parse_minutes() {
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("1min").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration("10mins").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_duration("2minutes").unwrap(), Duration::from_secs(120));
    }
    
    #[test]
    fn test_parse_hours() {
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("2hr").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("24hours").unwrap(), Duration::from_secs(86400));
    }
    
    #[test]
    fn test_parse_combined_formats() {
        // 1h30m = 90 minutes = 5400 seconds
        assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
        
        // 1m30s = 90 seconds
        assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
        
        // 1h30m15s = 5415 seconds
        assert_eq!(parse_duration("1h30m15s").unwrap(), Duration::from_secs(5415));
        
        // 2h30m = 9000 seconds
        assert_eq!(parse_duration("2h30m").unwrap(), Duration::from_secs(9000));
    }
    
    #[test]
    fn test_parse_decimal_numbers() {
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
        assert_eq!(parse_duration("0.5m").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("2.5h").unwrap(), Duration::from_secs(9000));
    }
    
    #[test]
    fn test_parse_zero_duration() {
        assert_eq!(parse_duration("0s").unwrap(), Duration::ZERO);
        assert_eq!(parse_duration("0m").unwrap(), Duration::ZERO);
        assert_eq!(parse_duration("0h").unwrap(), Duration::ZERO);
    }
    
    #[test]
    fn test_parse_empty_string() {
        assert_eq!(
            parse_duration(""),
            Err(ParseDurationError::EmptyString)
        );
        assert_eq!(
            parse_duration("   "),
            Err(ParseDurationError::EmptyString)
        );
    }
    
    #[test]
    fn test_parse_invalid_format() {
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("30x").is_err());
        assert!(parse_duration("s30").is_err());
    }
    
    #[test]
    fn test_parse_negative_duration() {
        assert_eq!(
            parse_duration("-30s"),
            Err(ParseDurationError::NegativeDuration)
        );
        assert_eq!(
            parse_duration("-5m"),
            Err(ParseDurationError::NegativeDuration)
        );
    }
    
    #[test]
    fn test_parse_unknown_unit() {
        assert!(matches!(
            parse_duration("30y"),
            Err(ParseDurationError::UnknownUnit(_))
        ));
        assert!(matches!(
            parse_duration("5d"),
            Err(ParseDurationError::UnknownUnit(_))
        ));
    }
    
    #[test]
    fn test_large_values() {
        // Test large but valid values
        assert_eq!(parse_duration("999999s").unwrap(), Duration::from_secs(999999));
        assert_eq!(parse_duration("1000h").unwrap(), Duration::from_secs(3600000));
    }
}
