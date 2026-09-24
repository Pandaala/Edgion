//! Real IP plugin configuration
//!
//! Extracts the real client IP address from HTTP headers when behind trusted proxies.
//! Uses Nginx-style right-to-left traversal of X-Forwarded-For header.
//!
//! ## Algorithm (Nginx-compatible):
//! 1. If the downstream peer is not in trusted_ips, use it as RemoteIp
//! 2. Extract IPs from real_ip_header (e.g., X-Forwarded-For)
//! 3. Traverse from right to left, find first non-trusted IP
//! 4. Fall back to the downstream peer if all IPs are trusted
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

use crate::resources::edgion_config_data::IpGroup;
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
    #[schemars(
        pattern(crate::gwapi_types::HEADER_NAME_PATTERN),
        length(max = crate::gwapi_types::HEADER_NAME_MAX_LEN)
    )]
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
        self.check()
    }

    /// Single validation body shared by [`Self::validate_and_init`] and the
    /// [`PluginValidation`] admission path, so the global
    /// `EdgionGatewayConfig.spec.realIp` consumer and the RealIp plugin
    /// consumer can never drift apart.
    fn check(&self) -> Result<(), String> {
        use crate::matcher::ip_cidr_helpers::validate_groups_structure;

        if self.trusted_ips.is_empty() {
            return Err("'trusted_ips' must not be empty".to_string());
        }
        if self.real_ip_header.trim().is_empty() {
            return Err("'real_ip_header' must not be empty".to_string());
        }
        // A non-token or oversized name can never match a parsed header at
        // runtime, so extraction would be silently inert and `RemoteIp` would
        // stay pinned to the trusted proxy. Reject it before publication.
        // The reserved-name list of `validate_remote_ip_header_name` does not
        // apply here: this is the read direction, where `X-Forwarded-For` is
        // the default and must stay valid.
        if self.real_ip_header.len() > crate::gwapi_types::HEADER_NAME_MAX_LEN as usize
            || !crate::gwapi_types::is_valid_header_token(&self.real_ip_header)
        {
            return Err("'real_ip_header' must be a valid HTTP header name".to_string());
        }
        validate_groups_structure(&self.trusted_ips).map_err(|e| format!("trusted_ips: {}", e))?;
        Ok(())
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for RealIpConfig {
    fn validation_error(&self) -> Option<String> {
        self.check().err()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/real_ip_test/validation_tests.rs"]
mod validation_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/real_ip_test.rs"]
mod tests;
