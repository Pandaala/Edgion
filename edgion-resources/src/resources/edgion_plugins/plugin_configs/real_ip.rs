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
//!   - name: internal
//!     cidrs:
//!       - "10.0.0.0/8"
//!       - "172.16.0.0/12"
//! real_ip_header: "X-Forwarded-For"
//! recursive: true
//! ```
//!
//! ### Cloudflare setup:
//! ```yaml
//! trusted_ips:
//!   - name: cloudflare
//!     cidrs:
//!       - "173.245.48.0/20"
//!       - "103.21.244.0/22"
//!       - "103.22.200.0/22"
//! real_ip_header: "CF-Connecting-IP"
//! recursive: false
//! ```

use crate::resources::data::IpGroup;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Real IP plugin configuration
///
/// Extracts real client IP from headers with trusted proxy support,
/// similar to nginx's ngx_http_realip_module.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RealIpConfig {
    /// Trusted proxy IP groups.
    ///
    /// Each `IpGroup` carries a `name` (surfaced in debug logs) and
    /// `cidrs` (IP/CIDR strings). Only requests from trusted IPs will
    /// trigger real IP extraction.
    pub trusted_ips: Vec<IpGroup>,

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

    /// Maximum number of trusted-proxy hops to traverse when `recursive=true`.
    ///
    /// Caps the right-to-left traversal depth — acts as a safety net when
    /// `trusted_ips` is broader than the real proxy chain (e.g., the
    /// entire `10.0.0.0/8` is trusted but attackers can also reach the
    /// gateway from that range and forge arbitrarily long XFF chains).
    ///
    /// Matches Envoy's `xff_num_trusted_hops` concept. `None` (default)
    /// means no cap (nginx-compatible). When set to `N`, traversal stops
    /// after skipping `N` trusted hops; the resulting IP at that position
    /// is returned even if trusted.
    ///
    /// Only takes effect when `recursive=true`. Default: `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_trusted_hops: Option<u32>,
    // NOTE: trusted_ip_matcher field DELETED in this refactor.
    // No production consumers existed (is_trusted_ip had zero non-test callers).
    // The matcher is now built by RealIpExtractor::new from `trusted_ips`.
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
            max_trusted_hops: None,
        }
    }
}

impl RealIpConfig {
    /// Create a new RealIpConfig with validation
    pub fn new(mut config: RealIpConfig) -> Result<Self, String> {
        config.validate_and_init()?;
        Ok(config)
    }

    /// Validate configuration. Does NOT build a matcher — `RealIpExtractor`
    /// builds its own from `trusted_ips` at runtime.
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        use crate::matcher::ip_cidr_helpers::validate_groups_structure;

        if self.trusted_ips.is_empty() {
            return Err("'trusted_ips' must not be empty".to_string());
        }
        if self.real_ip_header.trim().is_empty() {
            return Err("'real_ip_header' must not be empty".to_string());
        }
        validate_groups_structure(&self.trusted_ips).map_err(|e| format!("trusted_ips: {}", e))?;
        Ok(())
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for RealIpConfig {
    fn validation_error(&self) -> Option<String> {
        use crate::matcher::ip_cidr_helpers::validate_groups_structure;

        if self.trusted_ips.is_empty() {
            return Some("'trusted_ips' must not be empty".to_string());
        }
        if self.real_ip_header.trim().is_empty() {
            return Some("'real_ip_header' must not be empty".to_string());
        }
        if let Err(e) = validate_groups_structure(&self.trusted_ips) {
            return Some(format!("trusted_ips: {}", e));
        }
        None
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn test_plugin_validation_empty_trusted_ips() {
        let config = RealIpConfig::default();
        let err = config.validation_error();
        assert!(err.is_some());
        assert_eq!(err.unwrap(), "'trusted_ips' must not be empty");
    }

    #[test]
    fn test_plugin_validation_empty_real_ip_header() {
        let config = RealIpConfig {
            trusted_ips: vec![IpGroup {
                name: "default".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            }],
            real_ip_header: "   ".to_string(),
            ..Default::default()
        };
        let err = config.validation_error();
        assert!(err.is_some());
        assert_eq!(err.unwrap(), "'real_ip_header' must not be empty");
    }

    #[test]
    fn test_plugin_validation_all_invalid_ips() {
        let config = RealIpConfig {
            trusted_ips: vec![IpGroup {
                name: "default".into(),
                description: None,
                cidrs: vec!["not-an-ip".to_string(), "999.999.999.999".to_string()],
            }],
            ..Default::default()
        };
        let err = config.validation_error();
        assert!(err.is_some());
        let msg = err.unwrap();
        assert!(msg.starts_with("trusted_ips:"), "unexpected message: {msg}");
        assert!(msg.contains("Invalid IP"), "unexpected message: {msg}");
    }

    #[test]
    fn test_plugin_validation_valid_config() {
        let config = RealIpConfig {
            trusted_ips: vec![IpGroup {
                name: "default".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".to_string(), "192.168.1.1".to_string()],
            }],
            real_ip_header: "X-Forwarded-For".to_string(),
            ..Default::default()
        };
        assert!(config.validation_error().is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(name: &str, cidrs: &[&str]) -> IpGroup {
        IpGroup {
            name: name.to_string(),
            description: None,
            cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_default_config() {
        let config = RealIpConfig::default();
        assert_eq!(config.trusted_ips.len(), 0);
        assert_eq!(config.real_ip_header, "X-Forwarded-For");
        assert!(config.recursive);
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
            trusted_ips: vec![group("default", &["10.0.0.0/8"])],
            real_ip_header: "".to_string(),
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("real_ip_header"));
    }

    #[test]
    fn test_invalid_ip() {
        let mut config = RealIpConfig {
            trusted_ips: vec![group("default", &["192.168.1.256"])],
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
        // New error path: validation surfaces the parse error via validate_groups_structure.
        let msg = result.unwrap_err();
        assert!(msg.contains("Invalid IP"), "unexpected message: {msg}");
    }

    #[test]
    fn test_invalid_cidr_prefix() {
        let mut config = RealIpConfig {
            trusted_ips: vec![group("default", &["192.168.1.0/33"])],
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
    }

    #[test]
    fn test_custom_header() {
        let mut config = RealIpConfig {
            trusted_ips: vec![group("default", &["10.0.0.0/8"])],
            real_ip_header: "CF-Connecting-IP".to_string(),
            recursive: false,
            ..Default::default()
        };
        assert!(config.validate_and_init().is_ok());
        assert_eq!(config.real_ip_header, "CF-Connecting-IP");
        assert!(!config.recursive);
    }

    #[test]
    fn test_serialization() {
        let config = RealIpConfig {
            trusted_ips: vec![group("default", &["10.0.0.0/8", "192.168.1.1"])],
            real_ip_header: "X-Real-IP".to_string(),
            recursive: false,
            ..Default::default()
        };

        let json = serde_json::to_value(&config).unwrap();
        let deserialized: RealIpConfig = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.trusted_ips.len(), 1);
        assert_eq!(deserialized.trusted_ips[0].name, "default");
        assert_eq!(
            deserialized.trusted_ips[0].cidrs,
            vec!["10.0.0.0/8".to_string(), "192.168.1.1".to_string()]
        );
        assert_eq!(deserialized.real_ip_header, "X-Real-IP");
        assert!(!deserialized.recursive);
    }
}
