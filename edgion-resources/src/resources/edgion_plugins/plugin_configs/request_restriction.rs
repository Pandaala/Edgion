//! Request Restriction plugin configuration
//!
//! This plugin restricts access based on request attributes like headers, cookies,
//! query parameters, path, method, and referer.
//!
//! ## Model (DNF: OR-of-AND)
//!
//! - `ruleGroups` is an array combined with **OR**: the policy fires if **any**
//!   group matches.
//! - Each group's `rules` is combined with **AND**: a group matches only when all
//!   of its (non-skipped) predicates are true.
//! - A single global `mode` decides the outcome polarity for the whole config:
//!   - `Blacklist` (default): any matching group => reject; otherwise pass.
//!   - `Whitelist`: any matching group => pass; otherwise reject.
//!
//! A rule is a pure predicate (`value ∈ values` OR `value` matches `regex`); it
//! carries no per-rule allow/deny. This is a request-attribute **restriction**
//! plugin, not RBAC — there are no roles, principals, or identity subjects.
//!
//! ## Configuration Examples
//!
//! ### Blacklist: block bots OR /admin paths
//! ```yaml
//! requestRestriction:
//!   mode: Blacklist
//!   ruleGroups:
//!     - rules:
//!         - source: Header
//!           key: "User-Agent"
//!           regex: ["(?i).*Bot.*"]
//!           onMissing: NoMatch
//!     - rules:
//!         - source: Path
//!           values: ["/admin"]
//!   message: "Access denied"
//! ```
//!
//! ### Whitelist: allow only read-only methods
//! ```yaml
//! requestRestriction:
//!   mode: Whitelist
//!   ruleGroups:
//!     - rules:
//!         - source: Method
//!           values: ["GET", "HEAD", "OPTIONS"]
//!   status: 405
//! ```
//!
//! ### AND within a group: deny only DELETE to /admin
//! ```yaml
//! requestRestriction:
//!   mode: Blacklist
//!   ruleGroups:
//!     - rules:
//!         - source: Method
//!           values: ["DELETE"]
//!         - source: Path
//!           values: ["/admin"]
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::request_match::RestrictionRule;
#[cfg(test)]
use super::request_match::{OnMissing, RestrictionSource};

/// Global restriction polarity.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub enum RestrictionMode {
    /// Any matching group => reject; no group matches => pass. (default)
    #[default]
    Blacklist,
    /// Any matching group => pass; no group matches => reject.
    Whitelist,
}

/// Request Restriction plugin configuration.
///
/// DNF model: `ruleGroups` is OR; each group's `rules` is AND. A single global
/// `mode` decides the outcome polarity. A rule is a pure predicate (no per-rule
/// allow/deny). This is a request-attribute restriction plugin, not RBAC.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestRestrictionConfig {
    /// Global polarity (default: Blacklist).
    #[serde(default)]
    pub mode: RestrictionMode,

    /// Rule groups. The array is OR: the policy fires if ANY group matches.
    pub rule_groups: Vec<RuleGroup>,

    /// HTTP status code for rejection (default: 403).
    #[serde(default = "default_status")]
    #[schemars(range(min = 100, max = 599))]
    pub status: u16,

    /// Custom rejection message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A group of predicates combined with AND.
///
/// The group matches only when all non-skipped predicates are true and at least
/// one predicate was considered.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleGroup {
    /// Group name for logging/debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Predicates ANDed together (non-empty).
    pub rules: Vec<RestrictionRule>,
}

fn default_status() -> u16 {
    403
}

impl Default for RequestRestrictionConfig {
    fn default() -> Self {
        Self {
            mode: RestrictionMode::default(),
            rule_groups: Vec::new(),
            status: default_status(),
            message: None,
        }
    }
}

impl RequestRestrictionConfig {
    /// Validate configuration and compile matchers.
    ///
    /// Returns `Err(message)` if any predicate is malformed; on success every
    /// predicate has its compiled matchers (HashSet, regex) populated.
    pub fn validate(&mut self) -> Result<(), String> {
        self.validate_and_compile()
    }

    /// Stateless field validation — mirrors every error branch of validate_and_compile
    /// without mutating self or compiling any matchers.
    pub fn check_fields(&self) -> Option<String> {
        if self.rule_groups.is_empty() {
            return Some("At least one rule group must be specified".to_string());
        }
        if self.status < 100 || self.status >= 600 {
            return Some(format!("Invalid status code: {}", self.status));
        }
        for (gi, group) in self.rule_groups.iter().enumerate() {
            let group_name = group.name.clone().unwrap_or_else(|| format!("group[{}]", gi));
            if group.rules.is_empty() {
                return Some(format!("Group '{}': at least one rule must be specified", group_name));
            }
            for rule in group.rules.iter() {
                if let Some(err) = rule.field_error() {
                    return Some(format!("Group '{}' {}", group_name, err));
                }
            }
        }
        None
    }

    /// Validate and compile all predicates across all groups.
    fn validate_and_compile(&mut self) -> Result<(), String> {
        if let Some(err) = self.check_fields() {
            return Err(err);
        }
        for (gi, group) in self.rule_groups.iter_mut().enumerate() {
            let group_name = group.name.clone().unwrap_or_else(|| format!("group[{}]", gi));
            for (ri, rule) in group.rules.iter_mut().enumerate() {
                let rule_name = rule.name.clone().unwrap_or_else(|| format!("rule[{}]", ri));
                rule.compile()
                    .map_err(|e| format!("Group '{}' rule '{}': {}", group_name, rule_name, e))?;
            }
        }
        Ok(())
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for RequestRestrictionConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RequestRestrictionConfig::default();
        assert!(config.rule_groups.is_empty());
        assert_eq!(config.mode, RestrictionMode::Blacklist);
        assert_eq!(config.status, 403);
        assert_eq!(config.message, None);
    }

    #[test]
    fn test_empty_groups_validation() {
        let mut config = RequestRestrictionConfig::default();
        let err = config.validate().unwrap_err();
        assert!(err.contains("rule group"));
    }

    #[test]
    fn test_empty_rules_in_group_validation() {
        let mut config = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![],
            }],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("at least one rule"));
    }

    #[test]
    fn test_missing_key_for_header() {
        let mut config = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: None,
                    values: Some(vec!["bad".to_string()]),
                    ..Default::default()
                }],
            }],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("key"));
    }

    #[test]
    fn test_path_no_key_required() {
        let mut config = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Path,
                    values: Some(vec!["/admin".to_string()]),
                    ..Default::default()
                }],
            }],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_predicate_needs_match_set_unless_on_missing_match() {
        // No values/regex + onMissing NoMatch => invalid.
        let mut bad = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X".to_string()),
                    on_missing: OnMissing::NoMatch,
                    ..Default::default()
                }],
            }],
            ..Default::default()
        };
        assert!(bad.validate().is_err());

        // No values/regex + onMissing Match => valid (pure presence check).
        let mut ok = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X".to_string()),
                    on_missing: OnMissing::Match,
                    ..Default::default()
                }],
            }],
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn test_invalid_regex() {
        let mut config = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X-Test".to_string()),
                    regex: Some(vec!["[invalid".to_string()]),
                    ..Default::default()
                }],
            }],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("Invalid"));
    }

    #[test]
    fn test_empty_values_list() {
        let mut config = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X-Test".to_string()),
                    values: Some(vec![]),
                    ..Default::default()
                }],
            }],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("cannot be empty"));
    }
}
