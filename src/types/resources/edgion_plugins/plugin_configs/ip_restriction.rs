//! IP Restriction plugin configuration

use crate::core::matcher::ip_radix_tree::IpRadixMatcher;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// IP Restriction plugin configuration
///
/// ## Access Control Priority (same as nginx):
/// 1. **Deny takes precedence**: If IP matches deny list, reject immediately
/// 2. **Allow next**: If IP matches allow list, permit access
/// 3. **Default action**: If no match, use `default_action`
///
/// ## Usage Examples:
///
/// ### Whitelist mode (only allow specific IPs):
/// ```yaml
/// allow: ["192.168.1.0/24"]
/// # Allows only 192.168.1.x, denies all others
/// ```
///
/// ### Blacklist mode (deny specific IPs):
/// ```yaml
/// deny: ["192.168.1.100"]
/// # Denies only 192.168.1.100, allows all others (default_action: allow)
/// ```
///
/// ### Combined mode (allow subnet but deny specific IP):
/// ```yaml
/// allow: ["10.0.0.0/8"]
/// deny: ["10.0.0.100"]
/// # Allows 10.x.x.x except 10.0.0.100 (deny wins)
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpRestrictionConfig {
    /// Allow list of IP addresses or CIDR ranges (whitelist)
    ///
    /// When configured, acts as a whitelist: only IPs in this list are allowed.
    /// Can be combined with `deny` list (deny takes precedence).
    ///
    /// Examples: ["192.168.1.0/24", "10.0.0.1"]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,

    /// Deny list of IP addresses or CIDR ranges (blacklist)
    ///
    /// **Highest priority**: IPs in this list are always denied, even if in allow list.
    /// Can be used alone (blacklist mode) or combined with allow list.
    ///
    /// Examples: ["192.168.1.100", "172.16.0.0/12"]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,

    /// IP source to check (default: ClientIp)
    /// - ClientIp: Extract from X-Forwarded-For (first IP) or X-Real-IP
    /// - RemoteAddr: Use direct TCP connection peer address
    #[serde(default)]
    pub ip_source: IpSource,

    /// Custom rejection message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// HTTP status code for rejection (default: 403)
    #[serde(default = "default_status")]
    pub status: u16,

    /// Default action when no rules match (default: Allow)
    #[serde(default)]
    pub default_action: DefaultAction,

    // === Runtime-only fields (not serialized) ===

    /// Compiled allow matcher (built from allow list)
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) allow_matcher: Option<IpRadixMatcher>,

    /// Compiled deny matcher (built from deny list)
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) deny_matcher: Option<IpRadixMatcher>,
}

/// IP source for extracting client IP address
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IpSource {
    /// Extract from X-Forwarded-For (first IP) or fallback to X-Real-IP
    ClientIp,
    /// Use direct TCP connection peer address
    RemoteAddr,
}

impl Default for IpSource {
    fn default() -> Self {
        IpSource::ClientIp
    }
}

/// Default action when no rules match
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DefaultAction {
    /// Allow access when no rules match (default)
    Allow,
    /// Deny access when no rules match
    Deny,
}

impl Default for DefaultAction {
    fn default() -> Self {
        DefaultAction::Allow
    }
}

fn default_status() -> u16 {
    403
}

impl Default for IpRestrictionConfig {
    fn default() -> Self {
        Self {
            allow: None,
            deny: None,
            ip_source: IpSource::default(),
            message: None,
            status: default_status(),
            default_action: DefaultAction::default(),
            allow_matcher: None,
            deny_matcher: None,
        }
    }
}

impl IpRestrictionConfig {
    /// Create a new IpRestrictionConfig from deserialized config
    /// This initializes runtime matchers after deserialization
    pub fn new(mut config: IpRestrictionConfig) -> Result<Self, String> {
        config.validate_and_init()?;
        Ok(config)
    }

    /// Validate configuration and initialize runtime matchers
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        // Validation: must have at least one of allow or deny
        if self.allow.is_none() && self.deny.is_none() {
            return Err("At least one of 'allow' or 'deny' must be specified".to_string());
        }

        // Validate status code
        if self.status < 100 || self.status >= 600 {
            return Err(format!("Invalid status code: {}", self.status));
        }

        // Build allow matcher from allow list
        if let Some(ref allow_list) = self.allow {
            if allow_list.is_empty() {
                return Err("'allow' list cannot be empty".to_string());
            }

            let mut builder = IpRadixMatcher::builder();
            for ip_str in allow_list {
                // Validate IP/CIDR format
                Self::validate_ip_or_cidr(ip_str)?;

                // Convert single IP to CIDR format if needed
                let cidr_str = Self::to_cidr_format(ip_str)?;

                builder.insert(&cidr_str, true)
                    .map_err(|e| format!("Invalid IP/CIDR in allow list '{}': {}", ip_str, e))?;
            }

            self.allow_matcher = Some(builder.build()
                .map_err(|e| format!("Failed to build allow matcher: {}", e))?);
        }

        // Build deny matcher from deny list
        if let Some(ref deny_list) = self.deny {
            if deny_list.is_empty() {
                return Err("'deny' list cannot be empty".to_string());
            }

            let mut builder = IpRadixMatcher::builder();
            for ip_str in deny_list {
                // Validate IP/CIDR format
                Self::validate_ip_or_cidr(ip_str)?;

                // Convert single IP to CIDR format if needed
                let cidr_str = Self::to_cidr_format(ip_str)?;

                builder.insert(&cidr_str, true)
                    .map_err(|e| format!("Invalid IP/CIDR in deny list '{}': {}", ip_str, e))?;
            }

            self.deny_matcher = Some(builder.build()
                .map_err(|e| format!("Failed to build deny matcher: {}", e))?);
        }

        Ok(())
    }

    /// Validate IP address or CIDR range format
    fn validate_ip_or_cidr(ip_str: &str) -> Result<(), String> {
        // Check if it's a CIDR range
        if ip_str.contains('/') {
            let parts: Vec<&str> = ip_str.split('/').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid CIDR format: {}", ip_str));
            }

            // Validate IP part
            if parts[0].parse::<IpAddr>().is_err() {
                return Err(format!("Invalid IP address in CIDR: {}", parts[0]));
            }

            // Validate prefix length
            let prefix_len: u8 = parts[1].parse()
                .map_err(|_| format!("Invalid prefix length in CIDR: {}", parts[1]))?;

            // Check prefix length bounds (0-32 for IPv4, 0-128 for IPv6)
            let ip: IpAddr = parts[0].parse().unwrap();
            let max_prefix = if ip.is_ipv4() { 32 } else { 128 };
            if prefix_len > max_prefix {
                return Err(format!("Prefix length {} exceeds maximum {} for {:?}",
                                   prefix_len, max_prefix, ip));
            }
        } else {
            // Validate single IP address
            if ip_str.parse::<IpAddr>().is_err() {
                return Err(format!("Invalid IP address: {}", ip_str));
            }
        }

        Ok(())
    }

    /// Convert IP address to CIDR format if it's not already
    /// Single IPv4 becomes /32, single IPv6 becomes /128
    fn to_cidr_format(ip_str: &str) -> Result<String, String> {
        if ip_str.contains('/') {
            // Already in CIDR format
            Ok(ip_str.to_string())
        } else {
            // Single IP address - convert to CIDR
            let ip: IpAddr = ip_str.parse()
                .map_err(|_| format!("Invalid IP address: {}", ip_str))?;

            let prefix_len = if ip.is_ipv4() { 32 } else { 128 };
            Ok(format!("{}/{}", ip_str, prefix_len))
        }
    }

    /// Check if an IP address should be allowed or denied
    ///
    /// ## Priority Logic (nginx-compatible):
    /// 1. **Deny matcher** (highest priority): Check if IP is in deny list
    ///    - If matched → return `false` (denied)
    /// 2. **Allow matcher**: Check if IP is in allow list
    ///    - If matched → return `true` (allowed)
    ///    - If allow list exists but IP not matched → return `false` (denied)
    /// 3. **Default action**: No rules matched
    ///    - Return based on `default_action` setting
    ///
    /// ## Examples:
    ///
    /// Config: `allow: ["10.0.0.0/8"], deny: ["10.0.0.100"]`
    /// - `10.0.0.50` → `true` (in allow list)
    /// - `10.0.0.100` → `false` (in deny list, deny wins)
    /// - `8.8.8.8` → `false` (not in allow list, allow acts as whitelist)
    ///
    /// Config: `deny: ["192.168.1.100"], default_action: allow`
    /// - `192.168.1.100` → `false` (in deny list)
    /// - `192.168.1.50` → `true` (not in deny, default allows)
    pub fn check_ip_access(&self, ip: &IpAddr) -> bool {
        // Priority 1: Check deny list (denial takes precedence)
        if let Some(ref deny_matcher) = self.deny_matcher {
            if deny_matcher.match_ip(ip) == Some(true) {
                return false; // Explicitly denied
            }
        }

        // Priority 2: Check allow list
        if let Some(ref allow_matcher) = self.allow_matcher {
            if allow_matcher.match_ip(ip) == Some(true) {
                return true; // Explicitly allowed
            } else {
                // IP not in allow list means blocked (when allow list is configured)
                return false;
            }
        }

        // Priority 3: No rules matched, use default action
        matches!(self.default_action, DefaultAction::Allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IpRestrictionConfig::default();
        assert_eq!(config.allow, None);
        assert_eq!(config.deny, None);
        assert_eq!(config.ip_source, IpSource::ClientIp);
        assert_eq!(config.message, None);
        assert_eq!(config.status, 403);
        assert_eq!(config.default_action, DefaultAction::Allow);
    }

    #[test]
    fn test_missing_both_lists() {
        let mut config = IpRestrictionConfig::default();
        let result = config.validate_and_init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("At least one"));
    }

    #[test]
    fn test_invalid_status_code() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec!["192.168.1.1".to_string()]),
            status: 999,
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid status code"));
    }

    #[test]
    fn test_allow_list_validation() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec!["192.168.1.1".to_string(), "10.0.0.0/8".to_string()]),
            ..Default::default()
        };
        assert!(config.validate_and_init().is_ok());
    }

    #[test]
    fn test_deny_list_validation() {
        let mut config = IpRestrictionConfig {
            deny: Some(vec!["192.168.1.100".to_string(), "172.16.0.0/12".to_string()]),
            ..Default::default()
        };
        assert!(config.validate_and_init().is_ok());
    }

    #[test]
    fn test_invalid_ip() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec!["192.168.1.256".to_string()]), // Invalid IP
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_cidr_prefix() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec!["192.168.1.0/33".to_string()]), // Invalid prefix length
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
    }

    #[test]
    fn test_allow_list_access() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec!["192.168.1.0/24".to_string(), "10.0.0.1".to_string()]),
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        // IPs in allow list should be allowed
        assert!(config.check_ip_access(&"192.168.1.50".parse().unwrap()));
        assert!(config.check_ip_access(&"10.0.0.1".parse().unwrap()));

        // IPs not in allow list should be denied
        assert!(!config.check_ip_access(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_deny_list_access() {
        let mut config = IpRestrictionConfig {
            deny: Some(vec!["192.168.1.100".to_string(), "10.0.0.0/8".to_string()]),
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        // IPs in deny list should be denied
        assert!(!config.check_ip_access(&"192.168.1.100".parse().unwrap()));
        assert!(!config.check_ip_access(&"10.0.0.1".parse().unwrap()));

        // IPs not in deny list should be allowed (default action)
        assert!(config.check_ip_access(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_allow_deny_combination() {
        // Allow entire subnet but deny specific IP
        let mut config = IpRestrictionConfig {
            allow: Some(vec!["10.0.0.0/8".to_string()]),
            deny: Some(vec!["10.0.0.100".to_string()]),
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        // IP in allow list should be allowed
        assert!(config.check_ip_access(&"10.0.0.50".parse().unwrap()));

        // IP in deny list should be denied (deny takes precedence)
        assert!(!config.check_ip_access(&"10.0.0.100".parse().unwrap()));

        // IP not in either list should be denied (allow list is configured)
        assert!(!config.check_ip_access(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_default_action_deny() {
        let mut config = IpRestrictionConfig {
            deny: Some(vec!["192.168.1.100".to_string()]),
            default_action: DefaultAction::Deny,
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        // IP in deny list should be denied
        assert!(!config.check_ip_access(&"192.168.1.100".parse().unwrap()));

        // IP not in deny list should also be denied (default action)
        assert!(!config.check_ip_access(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_ipv6_support() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec!["::1".to_string(), "2001:db8::/32".to_string()]),
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        assert!(config.check_ip_access(&"::1".parse().unwrap()));
        assert!(config.check_ip_access(&"2001:db8::1".parse().unwrap()));
        assert!(!config.check_ip_access(&"2001:db9::1".parse().unwrap()));
    }

    #[test]
    fn test_empty_allow_list() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![]),
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be empty"));
    }

    #[test]
    fn test_serialization() {
        let config = IpRestrictionConfig {
            allow: Some(vec!["192.168.1.0/24".to_string()]),
            deny: Some(vec!["192.168.1.100".to_string()]),
            ip_source: IpSource::RemoteAddr,
            message: Some("Access denied".to_string()),
            status: 404,
            default_action: DefaultAction::Deny,
            ..Default::default()
        };

        let json = serde_json::to_value(&config).unwrap();
        let deserialized: IpRestrictionConfig = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.allow, config.allow);
        assert_eq!(deserialized.deny, config.deny);
        assert_eq!(deserialized.ip_source, IpSource::RemoteAddr);
        assert_eq!(deserialized.message, Some("Access denied".to_string()));
        assert_eq!(deserialized.status, 404);
        assert_eq!(deserialized.default_action, DefaultAction::Deny);
    }
}
