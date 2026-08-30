//! Test Metrics Module
//!
//! Provides detailed test data collection for metrics verification.
//! Supports different test types: LB (load balancing), Retry, Latency, etc.

use serde::Serialize;

/// Test type enumeration
///
/// Determines which test data setter function to call.
/// Configured via Gateway annotation: `edgion.io/metrics-test-type`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TestType {
    /// Not configured - production mode, no test data collected
    #[default]
    None,
    /// Load balancing test - collects ip, port, hash_key
    Lb,
    /// Retry test - collects try_count, error
    Retry,
    /// Latency test - collects latency_ms
    Latency,
}

impl TestType {
    /// Parse TestType from string
    ///
    /// Supported values: "lb", "retry", "latency" (case-insensitive)
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "lb" => TestType::Lb,
            "retry" => TestType::Retry,
            "latency" => TestType::Latency,
            _ => TestType::None,
        }
    }

    /// Check if test mode is enabled
    #[inline]
    pub fn is_enabled(&self) -> bool {
        !matches!(self, TestType::None)
    }
}

/// Test data structure
///
/// Serialized to JSON as the `test_data` label in metrics.
/// Fields are optional and only serialized when present.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TestData {
    // ========== LB test fields ==========
    /// Backend IP address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,

    /// Backend port
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// Consistent hash key (e.g., header value, cookie value)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_key: Option<String>,

    // ========== Retry test fields ==========
    /// Retry attempt count
    #[serde(skip_serializing_if = "Option::is_none")]
    pub try_count: Option<u32>,

    /// Error message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    // ========== Latency test fields ==========
    /// Latency in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

impl TestData {
    /// Create empty TestData
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if all fields are empty
    pub fn is_empty(&self) -> bool {
        self.ip.is_none()
            && self.port.is_none()
            && self.hash_key.is_none()
            && self.try_count.is_none()
            && self.error.is_none()
            && self.latency_ms.is_none()
    }

    /// Serialize to JSON string
    ///
    /// Returns empty string if all fields are None
    pub fn to_json(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_parsing() {
        assert_eq!(TestType::from_str("lb"), TestType::Lb);
        assert_eq!(TestType::from_str("LB"), TestType::Lb);
        assert_eq!(TestType::from_str("Lb"), TestType::Lb);
        assert_eq!(TestType::from_str("retry"), TestType::Retry);
        assert_eq!(TestType::from_str("RETRY"), TestType::Retry);
        assert_eq!(TestType::from_str("latency"), TestType::Latency);
        assert_eq!(TestType::from_str("LATENCY"), TestType::Latency);
        assert_eq!(TestType::from_str("unknown"), TestType::None);
        assert_eq!(TestType::from_str(""), TestType::None);
    }

    #[test]
    fn test_type_is_enabled() {
        assert!(!TestType::None.is_enabled());
        assert!(TestType::Lb.is_enabled());
        assert!(TestType::Retry.is_enabled());
        assert!(TestType::Latency.is_enabled());
    }

    #[test]
    fn test_empty_data() {
        let data = TestData::new();
        assert!(data.is_empty());
        assert_eq!(data.to_json(), "");
    }
}
