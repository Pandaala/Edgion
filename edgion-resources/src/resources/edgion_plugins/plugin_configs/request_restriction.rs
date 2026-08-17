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
//! carries no per-rule allow/deny. Predicates match request attributes
//! (header/cookie/query/path/method/referer) and, additionally,
//! post-authentication identity: mTLS client-cert subject / SAN / CN
//! (`MtlsSubject` / `MtlsSan` / `MtlsCn`) and verified JWT claims (`JwtClaim`).
//! It is still a local allow/deny matcher, not a full RBAC orchestration layer.
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
/// allow/deny). Predicates match request attributes and post-authentication
/// identity (mTLS subject/SAN/CN, verified JWT claims); it remains a local
/// allow/deny matcher, not a full RBAC orchestration layer.
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

    /// Disposition when **no GeoIP database is loaded at all**, for every
    /// `ClientGeo`/`DirectGeo` rule across every group in this plugin.
    /// Required (no default — the operator must state the intent explicitly
    /// and own the consequence) precisely when this plugin contains at least
    /// one geo rule; rejected when it contains none.
    ///
    /// Plugin-level rather than per-rule: the library-unavailable disposition
    /// is a single fail-open-or-closed decision for the whole plugin (a
    /// `false` rule short-circuits the whole plugin to rejection regardless
    /// of `mode` or any other rule/group — see `RequestRestriction::
    /// has_fail_closed_geo_rule` in the gateway), so a per-rule value could
    /// only ever agree with itself across every geo rule in the plugin.
    ///
    /// This is deliberately separate from a rule's `onMissing`, which cannot
    /// express fail-closed: in blacklist mode a false predicate means
    /// *allow*, so "the database is not loaded" and "block everything until
    /// it is" cannot both be said with `onMissing`.
    ///
    /// - `false` — the plugin short-circuits to rejection, regardless of `mode`.
    /// - `true`  — every geo predicate is dropped from its AND group, like `Skip`.
    ///
    /// `onMissing` still governs the *other* case: a database is loaded but
    /// has no record for this address, or the record lacks the requested
    /// dimension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_open: Option<bool>,
}

/// `true` when `config` has at least one `ClientGeo`/`DirectGeo` rule in any
/// group. Determines whether `fail_open` is required (`true`) or forbidden
/// (`false`).
fn has_geo_rule(rule_groups: &[RuleGroup]) -> bool {
    rule_groups.iter().flat_map(|g| &g.rules).any(RestrictionRule::is_geo)
}

/// `'failOpen' is only valid when the plugin has at least one ClientGeo /
/// DirectGeo rule` — shared text so the "forbidden" branch of the plugin's
/// `failOpen` check reads identically wherever it fires.
const FAIL_OPEN_NOT_GEO_ERR: &str =
    "'failOpen' is only valid when the plugin has at least one ClientGeo/DirectGeo rule";
const FAIL_OPEN_REQUIRED_ERR: &str =
    "at least one ClientGeo/DirectGeo rule is present; the plugin must declare an explicit 'failOpen'";

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
            fail_open: None,
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
        match (has_geo_rule(&self.rule_groups), self.fail_open) {
            (true, None) => return Some(FAIL_OPEN_REQUIRED_ERR.to_string()),
            (false, Some(_)) => return Some(FAIL_OPEN_NOT_GEO_ERR.to_string()),
            _ => {}
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

    fn geo_rule() -> RestrictionRule {
        RestrictionRule {
            source: RestrictionSource::ClientGeo,
            key: Some("country".to_string()),
            values: Some(vec!["CN".to_string()]),
            ..Default::default()
        }
    }

    /// `failOpen` moved from per-rule to plugin-level: a plugin with a geo
    /// rule anywhere in any group must declare it explicitly.
    #[test]
    fn plugin_requires_fail_open_when_it_has_a_geo_rule() {
        let mut config = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![geo_rule()],
            }],
            fail_open: None,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("failOpen"), "got: {err}");
    }

    /// A plugin with no geo rule at all must not be forced to set `failOpen`
    /// — this is the "not every plugin pays for the geo knob" requirement.
    #[test]
    fn plugin_without_geo_rule_does_not_require_fail_open() {
        let mut config = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X".to_string()),
                    values: Some(vec!["v".to_string()]),
                    ..Default::default()
                }],
            }],
            fail_open: None,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    /// `failOpen` set without any geo rule in the plugin is rejected — the
    /// field is meaningless (and would be silently misleading) otherwise.
    #[test]
    fn plugin_rejects_fail_open_when_it_has_no_geo_rule() {
        let mut config = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X".to_string()),
                    values: Some(vec!["v".to_string()]),
                    ..Default::default()
                }],
            }],
            fail_open: Some(false),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("failOpen"), "got: {err}");
    }

    #[test]
    fn plugin_with_geo_rule_and_explicit_fail_open_is_valid() {
        let mut config = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![geo_rule()],
            }],
            fail_open: Some(false),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    /// `check_fields()` (controller ingest) and `validate()` (gateway
    /// compile, which calls `check_fields()` first) must reject the same
    /// plugin-level `failOpen` misconfiguration with identical text — the
    /// same ingest/gateway parity invariant the per-rule geo checks already
    /// guarantee.
    #[test]
    fn check_fields_and_validate_agree_on_fail_open_errors() {
        let missing = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![geo_rule()],
            }],
            fail_open: None,
            ..Default::default()
        };
        let mut compiled = missing.clone();
        assert_eq!(missing.check_fields(), compiled.validate().err());

        let stray = RequestRestrictionConfig {
            rule_groups: vec![RuleGroup {
                name: None,
                rules: vec![RestrictionRule {
                    source: RestrictionSource::Header,
                    key: Some("X".to_string()),
                    values: Some(vec!["v".to_string()]),
                    ..Default::default()
                }],
            }],
            fail_open: Some(true),
            ..Default::default()
        };
        let mut compiled_stray = stray.clone();
        assert_eq!(stray.check_fields(), compiled_stray.validate().err());
    }
}
