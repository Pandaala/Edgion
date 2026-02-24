//! Real IP plugin configuration
//!
//! Extracts the real client IP address from HTTP headers when behind trusted proxies.
//! Uses Nginx-style right-to-left traversal of X-Forwarded-For header.
//!
//! ## Algorithm (Nginx-compatible):
//! 1. If client_addr is not in trusted_ips, use client_addr as real IP
//! 2. Extract IPs from real_ip_header (e.g., X-Forwarded-For)
//! 3. Traverse from right to left, find first non-trusted IP
//! 4. Fallback to client_addr if all IPs are trusted
//!
//! ## Usage Examples:
//!
//! ### Basic usage (trust CDN/load balancer):
//! ```yaml
//! trusted_ips:
//!   - "10.0.0.0/8"
//!   - "172.16.0.0/12"
//! real_ip_header: "X-Forwarded-For"
//! recursive: true
//! ```
//!
//! ### Cloudflare setup:
//! ```yaml
//! trusted_ips:
//!   - "173.245.48.0/20"
//!   - "103.21.244.0/22"
//!   - "103.22.200.0/22"
//! real_ip_header: "CF-Connecting-IP"
//! recursive: false
//! ```

use crate::core::matcher::ip_radix_tree::IpRadixMatcher;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::Arc;

/// Real IP plugin configuration
///
/// Extracts real client IP from headers with trusted proxy support,
/// similar to nginx's ngx_http_realip_module.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RealIpConfig {
    /// Trusted proxy IP addresses or CIDR ranges
    ///
    /// Corresponds to nginx's `set_real_ip_from` directive.
    /// Only requests from these IPs will trigger real IP extraction.
    ///
    /// Examples: ["10.0.0.0/8", "192.168.1.1", "2001:db8::/32"]
    #[serde(default)]
    pub trusted_ips: Vec<String>,

    /// Header name to extract real IP from
    ///
    /// Corresponds to nginx's `real_ip_header` directive.
    /// Common values: "X-Forwarded-For", "X-Real-IP", "CF-Connecting-IP", "True-Client-IP"
    ///
    /// Default: "X-Forwarded-For"
    #[serde(default = "default_real_ip_header")]
    pub real_ip_header: String,

    /// Enable recursive search (nginx-style right-to-left traversal)
    ///
    /// Corresponds to nginx's `real_ip_recursive` directive.
    /// When true, traverses X-Forwarded-For from right to left,
    /// skipping trusted IPs until finding the first non-trusted IP.
    ///
    /// Default: true
    #[serde(default = "default_recursive")]
    pub recursive: bool,

    // === Runtime-only fields (not serialized) ===
    /// Compiled IP matcher (built from trusted_ips)
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) trusted_ip_matcher: Option<Arc<IpRadixMatcher>>,
}

fn default_real_ip_header() -> String {
    "X-Forwarded-For".to_string()
}

fn default_recursive() -> bool {
    true
}

impl Default for RealIpConfig {
    fn default() -> Self {
        Self {
            trusted_ips: Vec::new(),
            real_ip_header: default_real_ip_header(),
            recursive: default_recursive(),
            trusted_ip_matcher: None,
        }
    }
}

impl RealIpConfig {
    /// Create a new RealIpConfig with validation
    pub fn new(mut config: RealIpConfig) -> Result<Self, String> {
        config.validate_and_init()?;
        Ok(config)
    }

    /// Validate configuration and initialize runtime matcher
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        // Validation: trusted_ips must not be empty
        if self.trusted_ips.is_empty() {
            return Err("'trusted_ips' must not be empty".to_string());
        }

        // Validation: real_ip_header must not be empty
        if self.real_ip_header.trim().is_empty() {
            return Err("'real_ip_header' must not be empty".to_string());
        }

        // Build trusted IP matcher
        let mut builder = IpRadixMatcher::builder();
        let mut valid_count = 0;
        let mut invalid_count = 0;

        for ip_str in &self.trusted_ips {
            // Validate and convert to CIDR format
            match Self::validate_and_to_cidr(ip_str) {
                Ok(cidr_str) => match builder.insert(&cidr_str, true) {
                    Ok(_) => valid_count += 1,
                    Err(e) => {
                        invalid_count += 1;
                        tracing::warn!(
                            ip = %ip_str,
                            error = ?e,
                            "Invalid IP/CIDR in trusted_ips, skipping"
                        );
                    }
                },
                Err(e) => {
                    invalid_count += 1;
                    tracing::warn!(
                        ip = %ip_str,
                        error = %e,
                        "Invalid IP/CIDR format in trusted_ips, skipping"
                    );
                }
            }
        }

        if valid_count == 0 {
            return Err(format!(
                "No valid IPs in trusted_ips (total: {}, invalid: {})",
                self.trusted_ips.len(),
                invalid_count
            ));
        }

        if invalid_count > 0 {
            tracing::warn!(
                valid_ips = valid_count,
                invalid_ips = invalid_count,
                "Some IPs in trusted_ips were invalid and skipped"
            );
        }

        self.trusted_ip_matcher = Some(Arc::new(
            builder
                .build()
                .map_err(|e| format!("Failed to build trusted IP matcher: {}", e))?,
        ));

        Ok(())
    }

    /// Validate IP/CIDR format and convert to CIDR if needed
    fn validate_and_to_cidr(ip_str: &str) -> Result<String, String> {
        let trimmed = ip_str.trim();

        if trimmed.contains('/') {
            // CIDR format - validate both parts
            let parts: Vec<&str> = trimmed.split('/').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid CIDR format: {}", trimmed));
            }

            // Validate IP part
            let ip: IpAddr = parts[0]
                .parse()
                .map_err(|_| format!("Invalid IP address in CIDR: {}", parts[0]))?;

            // Validate prefix length
            let prefix_len: u8 = parts[1]
                .parse()
                .map_err(|_| format!("Invalid prefix length in CIDR: {}", parts[1]))?;

            let max_prefix = if ip.is_ipv4() { 32 } else { 128 };
            if prefix_len > max_prefix {
                return Err(format!(
                    "Prefix length {} exceeds maximum {} for {:?}",
                    prefix_len, max_prefix, ip
                ));
            }

            Ok(trimmed.to_string())
        } else {
            // Single IP - convert to CIDR
            let ip: IpAddr = trimmed
                .parse()
                .map_err(|_| format!("Invalid IP address: {}", trimmed))?;

            let prefix_len = if ip.is_ipv4() { 32 } else { 128 };
            Ok(format!("{}/{}", trimmed, prefix_len))
        }
    }

    /// Check if an IP address is trusted
    pub fn is_trusted_ip(&self, ip: &IpAddr) -> bool {
        self.trusted_ip_matcher
            .as_ref()
            .and_then(|m| m.match_ip(ip))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RealIpConfig::default();
        assert_eq!(config.trusted_ips.len(), 0);
        assert_eq!(config.real_ip_header, "X-Forwarded-For");
        assert_eq!(config.recursive, true);
    }

    #[test]
    fn test_empty_trusted_ips() {
        let mut config = RealIpConfig::default();
        let result = config.validate_and_init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be empty"));
    }

    #[test]
    fn test_empty_real_ip_header() {
        let mut config = RealIpConfig {
            trusted_ips: vec!["10.0.0.0/8".to_string()],
            real_ip_header: "".to_string(),
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("real_ip_header"));
    }

    #[test]
    fn test_valid_ipv4_single() {
        let mut config = RealIpConfig {
            trusted_ips: vec!["192.168.1.1".to_string()],
            ..Default::default()
        };
        assert!(config.validate_and_init().is_ok());
        assert!(config.is_trusted_ip(&"192.168.1.1".parse().unwrap()));
        assert!(!config.is_trusted_ip(&"192.168.1.2".parse().unwrap()));
    }

    #[test]
    fn test_valid_ipv4_cidr() {
        let mut config = RealIpConfig {
            trusted_ips: vec!["10.0.0.0/8".to_string(), "192.168.1.0/24".to_string()],
            ..Default::default()
        };
        assert!(config.validate_and_init().is_ok());
        assert!(config.is_trusted_ip(&"10.0.0.1".parse().unwrap()));
        assert!(config.is_trusted_ip(&"192.168.1.50".parse().unwrap()));
        assert!(!config.is_trusted_ip(&"172.16.0.1".parse().unwrap()));
    }

    #[test]
    fn test_valid_ipv6() {
        let mut config = RealIpConfig {
            trusted_ips: vec!["::1".to_string(), "2001:db8::/32".to_string()],
            ..Default::default()
        };
        assert!(config.validate_and_init().is_ok());
        assert!(config.is_trusted_ip(&"::1".parse().unwrap()));
        assert!(config.is_trusted_ip(&"2001:db8::1".parse().unwrap()));
        assert!(!config.is_trusted_ip(&"2001:db9::1".parse().unwrap()));
    }

    #[test]
    fn test_invalid_ip() {
        let mut config = RealIpConfig {
            trusted_ips: vec!["192.168.1.256".to_string()],
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No valid IPs"));
    }

    #[test]
    fn test_invalid_cidr_prefix() {
        let mut config = RealIpConfig {
            trusted_ips: vec!["192.168.1.0/33".to_string()],
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
    }

    #[test]
    fn test_mixed_valid_invalid() {
        let mut config = RealIpConfig {
            trusted_ips: vec![
                "10.0.0.0/8".to_string(),    // valid
                "192.168.1.256".to_string(), // invalid
                "172.16.0.0/12".to_string(), // valid
            ],
            ..Default::default()
        };
        // Should succeed with warnings (valid IPs used)
        assert!(config.validate_and_init().is_ok());
        assert!(config.is_trusted_ip(&"10.0.0.1".parse().unwrap()));
        assert!(config.is_trusted_ip(&"172.16.0.1".parse().unwrap()));
    }

    #[test]
    fn test_custom_header() {
        let mut config = RealIpConfig {
            trusted_ips: vec!["10.0.0.0/8".to_string()],
            real_ip_header: "CF-Connecting-IP".to_string(),
            recursive: false,
            ..Default::default()
        };
        assert!(config.validate_and_init().is_ok());
        assert_eq!(config.real_ip_header, "CF-Connecting-IP");
        assert_eq!(config.recursive, false);
    }

    #[test]
    fn test_serialization() {
        let config = RealIpConfig {
            trusted_ips: vec!["10.0.0.0/8".to_string(), "192.168.1.1".to_string()],
            real_ip_header: "X-Real-IP".to_string(),
            recursive: false,
            ..Default::default()
        };

        let json = serde_json::to_value(&config).unwrap();
        let deserialized: RealIpConfig = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.trusted_ips, config.trusted_ips);
        assert_eq!(deserialized.real_ip_header, "X-Real-IP");
        assert_eq!(deserialized.recursive, false);
    }

    #[test]
    fn test_validate_and_to_cidr() {
        assert_eq!(
            RealIpConfig::validate_and_to_cidr("192.168.1.1").unwrap(),
            "192.168.1.1/32"
        );
        assert_eq!(RealIpConfig::validate_and_to_cidr("10.0.0.0/8").unwrap(), "10.0.0.0/8");
        assert_eq!(RealIpConfig::validate_and_to_cidr("::1").unwrap(), "::1/128");
        assert_eq!(
            RealIpConfig::validate_and_to_cidr("2001:db8::/32").unwrap(),
            "2001:db8::/32"
        );

        assert!(RealIpConfig::validate_and_to_cidr("192.168.1.256").is_err());
        assert!(RealIpConfig::validate_and_to_cidr("192.168.1.0/33").is_err());
        assert!(RealIpConfig::validate_and_to_cidr("invalid").is_err());
    }
}
