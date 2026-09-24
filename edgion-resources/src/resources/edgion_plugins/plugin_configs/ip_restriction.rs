//! IP Restriction plugin configuration

use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::AccessDecision;
use edgion_algo::matcher::radix_ip::IpRadixMatcher;
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
/// allow:
///   - name: local-network
///     description: Local network clients
///     cidrs: ["192.168.1.0/24"]
/// # Allows only 192.168.1.x, denies all others
/// ```
///
/// ### Blacklist mode (deny specific IPs):
/// ```yaml
/// deny:
///   - name: blocked-hosts
///     cidrs: ["192.168.1.100"]
/// # Denies only 192.168.1.100, allows all others (default_action: allow)
/// ```
///
/// ### Combined mode (allow subnet but deny specific IP):
/// ```yaml
/// allow:
///   - name: private-network
///     cidrs: ["10.0.0.0/8"]
/// deny:
///   - name: blocked-hosts
///     cidrs: ["10.0.0.100"]
/// # Allows 10.x.x.x except 10.0.0.100 (deny wins)
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpRestrictionConfig {
    /// Allow list groups (whitelist)
    ///
    /// Each group carries a `name` (surfaced in deny logs) and `cidrs`
    /// (IP/CIDR strings). When configured, acts as a whitelist: only IPs
    /// in any allow group are allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<IpGroup>>,

    /// Deny list groups (blacklist)
    ///
    /// Highest priority: IPs in any deny group are always denied, even if
    /// matched by an allow group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<IpGroup>>,

    /// IP source to check (default: RemoteIp)
    /// - RemoteIp: Reads the resolved remote address, pre-processed by RealIp plugin or global `spec.realIp`.
    ///   Falls back to TCP IP if neither is configured (safe, not forgeable in that case).
    ///   To extract the true client IP from XFF/X-Real-IP headers, configure RealIp first.
    /// - DirectPeerIp: Always the raw physical TCP connection peer address, never forgeable.
    #[serde(default)]
    pub ip_source: IpSource,

    /// Custom rejection message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// HTTP status code for rejection (default: 403)
    #[serde(default = "default_status")]
    #[schemars(range(min = 200, max = 599))]
    pub status: u16,

    /// Default action when no rules match (default: Allow)
    #[serde(default)]
    pub default_action: DefaultAction,

    /// Allow-list references to `IpList` EdgionConfigData resources (whole list).
    /// Resolved live in the gateway; merged into the allow set. Configuring any
    /// entry puts the plugin in whitelist mode (see `allow_mode`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_refs: Option<Vec<EdgionConfigDataRef>>,

    /// Deny-list references to `IpList` EdgionConfigData resources (whole list).
    /// Resolved live in the gateway; merged into the deny set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_refs: Option<Vec<EdgionConfigDataRef>>,

    // === Runtime-only fields (not serialized) ===
    /// Compiled allow matcher (built from allow list)
    #[serde(skip)]
    #[schemars(skip)]
    pub allow_matcher: Option<IpRadixMatcher>,

    /// Compiled deny matcher (built from deny list)
    #[serde(skip)]
    #[schemars(skip)]
    pub deny_matcher: Option<IpRadixMatcher>,
}

/// IP source for extracting client IP address
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
#[derive(Default)]
pub enum IpSource {
    /// Reads the resolved remote address (pre-processed by RealIp plugin or global `spec.realIp`).
    /// Without RealIp configured, falls back to TCP connection IP — not forgeable.
    /// Configure RealIp to extract the true client IP from XFF/X-Real-IP headers.
    #[default]
    RemoteIp,
    /// Always reads the raw physical TCP connection peer address, never forgeable.
    DirectPeerIp,
}

/// Default action when no rules match
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum DefaultAction {
    /// Allow access when no rules match (default)
    #[default]
    Allow,
    /// Deny access when no rules match
    Deny,
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
            allow_refs: None,
            deny_refs: None,
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
        use crate::matcher::ip_cidr_helpers::build_grouped_matcher_partial;

        let any_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let any_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !any_allow && !any_deny {
            return Err("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string());
        }

        if !crate::http_status::is_valid_final_http_status(self.status) {
            return Err(format!("Invalid status code: {}", self.status));
        }

        if let Some(groups) = &self.allow {
            if !groups.is_empty() {
                let (m, _errs) = build_grouped_matcher_partial(groups);
                self.allow_matcher = Some(m);
            }
        }
        if let Some(groups) = &self.deny {
            if !groups.is_empty() {
                let (m, _errs) = build_grouped_matcher_partial(groups);
                self.deny_matcher = Some(m);
            }
        }

        use crate::matcher::ip_cidr_helpers::validate_refs_structure;
        if let Some(refs) = &self.allow_refs {
            validate_refs_structure(refs)?;
        }
        if let Some(refs) = &self.deny_refs {
            validate_refs_structure(refs)?;
        }

        Ok(())
    }

    /// Check IP and return the full access decision including matched
    /// group name.
    ///
    /// Priority: deny > allow > default.
    /// Delegates to the shared `evaluate_ip_access` so the HTTP, L4, and
    /// global-connection IP-restriction paths cannot drift.
    pub fn check_ip_access(&self, ip: &IpAddr) -> AccessDecision {
        use crate::matcher::ip_cidr_helpers::evaluate_ip_access;
        evaluate_ip_access(
            self.deny_matcher.as_ref(),
            self.allow_matcher.as_ref(),
            &self.default_action,
            ip,
        )
    }

    /// Whitelist mode is decided by configured intent (spec Decision 5): true iff
    /// an inline allow group OR an allow ref is configured.
    pub fn allow_mode(&self) -> bool {
        self.allow.as_ref().is_some_and(|v| !v.is_empty()) || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty())
    }

    /// Decision over the union of inline matchers and gateway-resolved reference
    /// matchers. `*_ref_matchers` are the already-resolved `IpList` matchers; an
    /// unresolved ref simply contributes nothing (fail-closed for allow per
    /// `allow_mode`). Delegates to the shared `evaluate_ip_access_multi`.
    pub fn check_ip_access_with_refs(
        &self,
        ip: &IpAddr,
        deny_ref_matchers: &[&IpRadixMatcher],
        allow_ref_matchers: &[&IpRadixMatcher],
    ) -> AccessDecision {
        use crate::matcher::ip_cidr_helpers::evaluate_ip_access_multi;
        let mut deny: Vec<&IpRadixMatcher> = self.deny_matcher.iter().collect();
        deny.extend_from_slice(deny_ref_matchers);
        let mut allow: Vec<&IpRadixMatcher> = self.allow_matcher.iter().collect();
        allow.extend_from_slice(allow_ref_matchers);
        evaluate_ip_access_multi(&deny, &allow, self.allow_mode(), &self.default_action, ip)
    }
}

use crate::resources::edgion_plugins::PluginValidation;

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/ip_restriction_test/validation_trait_tests.rs"]
mod validation_trait_tests;

impl PluginValidation for IpRestrictionConfig {
    fn validation_error(&self) -> Option<String> {
        use crate::matcher::ip_cidr_helpers::validate_refs_structure;

        let any_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let any_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !any_allow && !any_deny {
            return Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string());
        }
        if !crate::http_status::is_valid_final_http_status(self.status) {
            return Some(format!("Invalid status code: {}", self.status));
        }
        if let Some(refs) = &self.allow_refs {
            if let Err(e) = validate_refs_structure(refs) {
                return Some(format!("allowRefs: {}", e));
            }
        }
        if let Some(refs) = &self.deny_refs {
            if let Err(e) = validate_refs_structure(refs) {
                return Some(format!("denyRefs: {}", e));
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/ip_restriction_test.rs"]
mod tests;
