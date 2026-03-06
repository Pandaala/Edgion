//! Test Metrics Module
//!
//! Provides detailed test data collection for metrics verification.
//! Supports different test types: LB (load balancing), Retry, Latency, etc.

use pingora_core::protocols::l4::socket::SocketAddr;
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

// ==================== Test data setter functions ====================

/// Set LB test data: backend address and optional hash key
pub fn set_lb_test_data(test_data: &mut TestData, addr: &SocketAddr, hash_key: Option<&str>) {
    // Extract IP and port from pingora's SocketAddr
    if let Some(inet_addr) = addr.as_inet() {
        test_data.ip = Some(inet_addr.ip().to_string());
        test_data.port = Some(inet_addr.port());
    }
    if let Some(hk) = hash_key {
        test_data.hash_key = Some(hk.to_string());
    }
}

/// Set retry test data: try count and optional error message
pub fn set_retry_test_data(test_data: &mut TestData, try_count: u32, error: Option<&str>) {
    test_data.try_count = Some(try_count);
    if let Some(e) = error {
        test_data.error = Some(e.to_string());
    }
}

/// Set latency test data: latency in milliseconds
pub fn set_latency_test_data(test_data: &mut TestData, latency_ms: u64) {
    test_data.latency_ms = Some(latency_ms);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr as StdSocketAddr};

    /// Helper: create pingora SocketAddr from std::net::SocketAddr
    fn make_addr(ip: [u8; 4], port: u16) -> SocketAddr {
        let std_addr = StdSocketAddr::new(IpAddr::V4(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])), port);
        SocketAddr::from(std_addr)
    }

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

    #[test]
    fn test_lb_data_without_hash_key() {
        let addr = make_addr([10, 0, 0, 5], 8080);
        let mut data = TestData::new();
        set_lb_test_data(&mut data, &addr, None);

        assert!(!data.is_empty());
        let json = data.to_json();
        assert!(json.contains("\"ip\":\"10.0.0.5\""));
        assert!(json.contains("\"port\":8080"));
        assert!(!json.contains("hash_key"));
    }

    #[test]
    fn test_lb_data_with_hash_key() {
        let addr = make_addr([10, 0, 0, 5], 8080);
        let mut data = TestData::new();
        set_lb_test_data(&mut data, &addr, Some("user-123"));

        let json = data.to_json();
        assert!(json.contains("\"ip\":\"10.0.0.5\""));
        assert!(json.contains("\"port\":8080"));
        assert!(json.contains("\"hash_key\":\"user-123\""));
    }

    #[test]
    fn test_retry_data() {
        let mut data = TestData::new();
        set_retry_test_data(&mut data, 3, Some("connection_timeout"));

        let json = data.to_json();
        assert!(json.contains("\"try_count\":3"));
        assert!(json.contains("\"error\":\"connection_timeout\""));
    }

    #[test]
    fn test_latency_data() {
        let mut data = TestData::new();
        set_latency_test_data(&mut data, 150);

        let json = data.to_json();
        assert!(json.contains("\"latency_ms\":150"));
    }
}
