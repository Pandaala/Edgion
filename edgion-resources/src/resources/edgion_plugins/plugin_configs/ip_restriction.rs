//! IP Restriction plugin configuration

use crate::matcher::ip_radix_tree::IpRadixMatcher;
use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::AccessDecision;
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

    /// IP source to check (default: ClientIp)
    /// - ClientIp: Reads `remote_addr` — pre-processed by RealIp plugin or global `spec.realIp`.
    ///   Falls back to TCP IP if neither is configured (safe, not forgeable in that case).
    ///   To extract the true client IP from XFF/X-Real-IP headers, configure RealIp first.
    /// - RemoteAddr: Always the TCP connection peer address (`client_addr`), never forgeable.
    #[serde(default)]
    pub ip_source: IpSource,

    /// Custom rejection message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// HTTP status code for rejection (default: 403)
    #[serde(default = "default_status")]
    #[schemars(range(min = 100, max = 599))]
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
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub enum IpSource {
    /// Reads `remote_addr` (pre-processed by RealIp plugin or global `spec.realIp`).
    /// Without RealIp configured, falls back to TCP connection IP — not forgeable.
    /// Configure RealIp to extract the true client IP from XFF/X-Real-IP headers.
    #[default]
    ClientIp,
    /// Always reads TCP connection peer address (`client_addr`), never forgeable.
    RemoteAddr,
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
        use crate::matcher::ip_cidr_helpers::build_grouped_matcher;

        let any_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let any_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !any_allow && !any_deny {
            return Err("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string());
        }

        if self.status < 100 || self.status >= 600 {
            return Err(format!("Invalid status code: {}", self.status));
        }

        if let Some(groups) = &self.allow {
            if !groups.is_empty() {
                self.allow_matcher = Some(build_grouped_matcher(groups)?);
            }
        }
        if let Some(groups) = &self.deny {
            if !groups.is_empty() {
                self.deny_matcher = Some(build_grouped_matcher(groups)?);
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
mod validation_trait_tests {
    use super::*;
    use crate::resources::edgion_plugins::PluginValidation;

    fn group(name: &str, cidrs: &[&str]) -> IpGroup {
        IpGroup {
            name: name.to_string(),
            description: None,
            cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn validation_error_both_none() {
        let config = IpRestrictionConfig::default();
        let result = PluginValidation::validation_error(&config);
        assert_eq!(
            result,
            Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string())
        );
    }

    #[test]
    fn validation_error_empty_allow_list() {
        let config = IpRestrictionConfig {
            allow: Some(vec![]),
            ..Default::default()
        };
        let result = PluginValidation::validation_error(&config);
        assert_eq!(
            result,
            Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string())
        );
    }

    #[test]
    fn validation_error_empty_deny_list() {
        let config = IpRestrictionConfig {
            deny: Some(vec![]),
            ..Default::default()
        };
        let result = PluginValidation::validation_error(&config);
        assert_eq!(
            result,
            Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string())
        );
    }

    #[test]
    fn validation_error_invalid_ip_string() {
        let config = IpRestrictionConfig {
            allow: Some(vec![group("default", &["not-an-ip"])]),
            ..Default::default()
        };
        let result = PluginValidation::validation_error(&config);
        assert!(result.is_some());
        assert!(result.unwrap().contains("not-an-ip"));
    }

    #[test]
    fn validation_error_valid_status_200() {
        let config = IpRestrictionConfig {
            allow: Some(vec![group("default", &["192.168.1.1"])]),
            status: 200,
            ..Default::default()
        };
        let result = PluginValidation::validation_error(&config);
        assert!(result.is_none());
    }

    #[test]
    fn validation_error_status_600() {
        let config = IpRestrictionConfig {
            allow: Some(vec![group("default", &["192.168.1.1"])]),
            status: 600,
            ..Default::default()
        };
        let result = PluginValidation::validation_error(&config);
        assert_eq!(result, Some("Invalid status code: 600".to_string()));
    }

    #[test]
    fn validation_error_valid_config() {
        let config = IpRestrictionConfig {
            allow: Some(vec![group("a", &["192.168.1.0/24"])]),
            deny: Some(vec![group("b", &["192.168.1.100"])]),
            status: 403,
            ..Default::default()
        };
        let result = PluginValidation::validation_error(&config);
        assert!(result.is_none());
    }

    #[test]
    fn status_field_has_range_in_json_schema() {
        let schema = serde_json::to_value(schemars::schema_for!(IpRestrictionConfig)).unwrap();
        let props = &schema["properties"]["status"];
        assert_eq!(props["minimum"].as_f64(), Some(100.0), "status minimum should be 100");
        assert_eq!(props["maximum"].as_f64(), Some(599.0), "status maximum should be 599");
    }
}

impl PluginValidation for IpRestrictionConfig {
    fn validation_error(&self) -> Option<String> {
        use crate::matcher::ip_cidr_helpers::{validate_groups_structure, validate_refs_structure};

        let any_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let any_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !any_allow && !any_deny {
            return Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string());
        }
        if self.status < 100 || self.status >= 600 {
            return Some(format!("Invalid status code: {}", self.status));
        }
        if let Some(groups) = &self.allow {
            if !groups.is_empty() {
                if let Err(e) = validate_groups_structure(groups) {
                    return Some(format!("allow: {}", e));
                }
            }
        }
        if let Some(groups) = &self.deny {
            if !groups.is_empty() {
                if let Err(e) = validate_groups_structure(groups) {
                    return Some(format!("deny: {}", e));
                }
            }
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
mod tests {
    use super::*;

    fn group(name: &str, cidrs: &[&str]) -> IpGroup {
        IpGroup {
            name: name.to_string(),
            description: None,
            cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Convenience: `check_ip_access` returns an `AccessDecision`; the legacy
    /// tests were boolean. This helper preserves their semantic intent.
    fn is_allowed(decision: AccessDecision) -> bool {
        matches!(
            decision,
            AccessDecision::AllowByGroup(_) | AccessDecision::Default(DefaultAction::Allow)
        )
    }

    #[test]
    fn test_default_config() {
        let config = IpRestrictionConfig::default();
        assert!(config.allow.is_none());
        assert!(config.deny.is_none());
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
            allow: Some(vec![group("default", &["192.168.1.1"])]),
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
            allow: Some(vec![group("default", &["192.168.1.1", "10.0.0.0/8"])]),
            ..Default::default()
        };
        assert!(config.validate_and_init().is_ok());
    }

    #[test]
    fn test_deny_list_validation() {
        let mut config = IpRestrictionConfig {
            deny: Some(vec![group("default", &["192.168.1.100", "172.16.0.0/12"])]),
            ..Default::default()
        };
        assert!(config.validate_and_init().is_ok());
    }

    #[test]
    fn test_invalid_ip() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![group("default", &["192.168.1.256"])]),
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_cidr_prefix() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![group("default", &["192.168.1.0/33"])]),
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
    }

    #[test]
    fn test_allow_list_access() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![group("default", &["192.168.1.0/24", "10.0.0.1"])]),
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        assert!(is_allowed(config.check_ip_access(&"192.168.1.50".parse().unwrap())));
        assert!(is_allowed(config.check_ip_access(&"10.0.0.1".parse().unwrap())));

        // Not in allow list -> DenyByWhitelistMiss
        assert!(matches!(
            config.check_ip_access(&"8.8.8.8".parse().unwrap()),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    #[test]
    fn test_deny_list_access() {
        let mut config = IpRestrictionConfig {
            deny: Some(vec![group("default", &["192.168.1.100", "10.0.0.0/8"])]),
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        assert!(matches!(
            config.check_ip_access(&"192.168.1.100".parse().unwrap()),
            AccessDecision::DenyByGroup(_)
        ));
        assert!(matches!(
            config.check_ip_access(&"10.0.0.1".parse().unwrap()),
            AccessDecision::DenyByGroup(_)
        ));

        // Not in deny list -> Default(Allow)
        assert!(is_allowed(config.check_ip_access(&"8.8.8.8".parse().unwrap())));
    }

    #[test]
    fn test_allow_deny_combination() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![group("a", &["10.0.0.0/8"])]),
            deny: Some(vec![group("b", &["10.0.0.100"])]),
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        assert!(is_allowed(config.check_ip_access(&"10.0.0.50".parse().unwrap())));
        assert!(matches!(
            config.check_ip_access(&"10.0.0.100".parse().unwrap()),
            AccessDecision::DenyByGroup(_)
        ));
        // Outside allow list -> DenyByWhitelistMiss
        assert!(matches!(
            config.check_ip_access(&"8.8.8.8".parse().unwrap()),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    #[test]
    fn test_default_action_deny() {
        let mut config = IpRestrictionConfig {
            deny: Some(vec![group("default", &["192.168.1.100"])]),
            default_action: DefaultAction::Deny,
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        assert!(matches!(
            config.check_ip_access(&"192.168.1.100".parse().unwrap()),
            AccessDecision::DenyByGroup(_)
        ));
        // Not in deny list -> Default(Deny)
        assert!(matches!(
            config.check_ip_access(&"8.8.8.8".parse().unwrap()),
            AccessDecision::Default(DefaultAction::Deny)
        ));
    }

    #[test]
    fn test_ipv6_support() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![group("default", &["::1", "2001:db8::/32"])]),
            ..Default::default()
        };
        config.validate_and_init().unwrap();

        assert!(is_allowed(config.check_ip_access(&"::1".parse().unwrap())));
        assert!(is_allowed(config.check_ip_access(&"2001:db8::1".parse().unwrap())));
        assert!(matches!(
            config.check_ip_access(&"2001:db9::1".parse().unwrap()),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    #[test]
    fn test_empty_allow_list() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![]),
            ..Default::default()
        };
        let result = config.validate_and_init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("At least one"));
    }

    #[test]
    fn test_serialization() {
        let config = IpRestrictionConfig {
            allow: Some(vec![group("a", &["192.168.1.0/24"])]),
            deny: Some(vec![group("b", &["192.168.1.100"])]),
            ip_source: IpSource::RemoteAddr,
            message: Some("Access denied".to_string()),
            status: 404,
            default_action: DefaultAction::Deny,
            ..Default::default()
        };

        let json = serde_json::to_value(&config).unwrap();
        let deserialized: IpRestrictionConfig = serde_json::from_value(json).unwrap();

        assert_eq!(
            deserialized.allow.as_ref().map(|v| v.len()),
            config.allow.as_ref().map(|v| v.len())
        );
        assert_eq!(
            deserialized.deny.as_ref().map(|v| v.len()),
            config.deny.as_ref().map(|v| v.len())
        );
        assert_eq!(deserialized.allow.as_ref().unwrap()[0].name, "a");
        assert_eq!(deserialized.deny.as_ref().unwrap()[0].name, "b");
        assert_eq!(deserialized.ip_source, IpSource::RemoteAddr);
        assert_eq!(deserialized.message, Some("Access denied".to_string()));
        assert_eq!(deserialized.status, 404);
        assert_eq!(deserialized.default_action, DefaultAction::Deny);
    }

    #[test]
    fn whitelist_miss_returns_specific_variant() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![IpGroup {
                name: "office".into(),
                description: None,
                cidrs: vec!["192.168.0.0/16".into()],
            }]),
            deny: None,
            ip_source: IpSource::ClientIp,
            message: None,
            status: 403,
            default_action: DefaultAction::Allow,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        config.validate_and_init().unwrap();
        let outside: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(matches!(
            config.check_ip_access(&outside),
            AccessDecision::DenyByWhitelistMiss
        ));
    }

    #[test]
    fn deny_priority_returns_deny_group() {
        let mut config = IpRestrictionConfig {
            allow: Some(vec![IpGroup {
                name: "allow_all".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            }]),
            deny: Some(vec![IpGroup {
                name: "blocked".into(),
                description: None,
                cidrs: vec!["10.0.0.100/32".into()],
            }]),
            ip_source: IpSource::ClientIp,
            message: None,
            status: 403,
            default_action: DefaultAction::Allow,
            allow_refs: None,
            deny_refs: None,
            allow_matcher: None,
            deny_matcher: None,
        };
        config.validate_and_init().unwrap();
        let blocked: IpAddr = "10.0.0.100".parse().unwrap();
        match config.check_ip_access(&blocked) {
            AccessDecision::DenyByGroup(g) => assert_eq!(&*g, "blocked"),
            other => panic!("expected DenyByGroup, got {:?}", other),
        }
    }

    #[test]
    fn allow_mode_true_when_only_allow_refs_set() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let config = IpRestrictionConfig {
            allow_refs: Some(vec![EdgionConfigDataRef {
                name: "corp".into(),
                namespace: None,
            }]),
            ..Default::default()
        };
        assert!(config.allow_mode());
    }

    #[test]
    fn refs_only_config_passes_validation() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let mut config = IpRestrictionConfig {
            deny_refs: Some(vec![EdgionConfigDataRef {
                name: "blocklist".into(),
                namespace: None,
            }]),
            ..Default::default()
        };
        // No inline allow/deny, only denyRefs -> must be accepted now.
        assert!(config.validate_and_init().is_ok());
        assert_eq!(PluginValidation::validation_error(&config), None);
    }

    #[test]
    fn with_refs_deny_union_blocks_referenced_ip() {
        use crate::matcher::ip_cidr_helpers::build_grouped_matcher;
        let mut config = IpRestrictionConfig {
            deny: Some(vec![group("inline", &["1.1.1.1"])]),
            ..Default::default()
        };
        config.validate_and_init().unwrap();
        let ref_deny = build_grouped_matcher(&[group("reflist", &["2.2.2.2"])]).unwrap();
        // referenced deny IP
        assert!(matches!(
            config.check_ip_access_with_refs(&"2.2.2.2".parse().unwrap(), &[&ref_deny], &[]),
            AccessDecision::DenyByGroup(_)
        ));
        // inline deny IP still blocked
        assert!(matches!(
            config.check_ip_access_with_refs(&"1.1.1.1".parse().unwrap(), &[&ref_deny], &[]),
            AccessDecision::DenyByGroup(_)
        ));
        // unrelated IP allowed (default Allow)
        assert!(is_allowed(config.check_ip_access_with_refs(
            &"8.8.8.8".parse().unwrap(),
            &[&ref_deny],
            &[]
        )));
    }

    #[test]
    fn allow_refs_missing_denies_all_in_allow_mode() {
        use crate::resources::edgion_config_data::EdgionConfigDataRef;
        let config = IpRestrictionConfig {
            allow_refs: Some(vec![EdgionConfigDataRef {
                name: "missing".into(),
                namespace: None,
            }]),
            ..Default::default()
        };
        // allow_mode true, but no resolved allow matchers -> whitelist miss (fail-closed).
        assert!(matches!(
            config.check_ip_access_with_refs(&"8.8.8.8".parse().unwrap(), &[], &[]),
            AccessDecision::DenyByWhitelistMiss
        ));
    }
}
