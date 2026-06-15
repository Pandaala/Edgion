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

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::path_normalization::normalize_config_path;

/// Threshold for pre-compiling values into a HashSet for O(1) lookup.
/// When values.len() > this threshold, a HashSet will be built during compilation.
pub const VALUES_HASHSET_THRESHOLD: usize = 16;

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

/// A single predicate. Answers one boolean question: does the extracted value match?
///
/// Predicate is true iff `value ∈ values` OR `value` matches `regex`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestrictionRule {
    /// Predicate name for logging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Data source to extract the value from.
    pub source: RestrictionSource,

    /// Key to extract (required for Header/Cookie/Query; ignored otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    /// Exact-match values. Auto HashSet when len > VALUES_HASHSET_THRESHOLD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,

    /// Regex patterns (merged with `|` into a single compiled regex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<Vec<String>>,

    /// Case sensitivity for exact match (default: true).
    /// For regex, use (?i:pattern) for case-insensitive matching.
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,

    /// Behavior when the source value is absent (default: NoMatch).
    #[serde(default)]
    pub on_missing: OnMissing,

    // === Runtime-only compiled matchers ===
    /// HashSet for O(1) exact lookup (when values.len() > threshold).
    #[serde(skip)]
    #[schemars(skip)]
    pub values_set: Option<HashSet<String>>,

    /// Compiled regex (merged from all patterns).
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<Regex>,
}

/// Data source for a restriction predicate.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum RestrictionSource {
    /// HTTP request header (requires key)
    Header,
    /// HTTP cookie (requires key)
    Cookie,
    /// URL query parameter (requires key)
    Query,
    /// Request path (no key needed)
    Path,
    /// Entry-time raw path (frozen). For audit / special raw-form match.
    RequestPath,
    /// HTTP method (no key needed)
    Method,
    /// Referer header (no key needed)
    Referer,
}

/// Behavior when the target value is missing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub enum OnMissing {
    /// Missing value => predicate is TRUE.
    Match,
    /// Missing value => predicate is FALSE (default).
    #[default]
    NoMatch,
    /// Missing value => predicate is DROPPED from the group's AND.
    Skip,
}

fn default_status() -> u16 {
    403
}

fn default_case_sensitive() -> bool {
    true
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

impl Default for RestrictionRule {
    fn default() -> Self {
        Self {
            name: None,
            source: RestrictionSource::Header,
            key: None,
            values: None,
            regex: None,
            case_sensitive: default_case_sensitive(),
            on_missing: OnMissing::default(),
            values_set: None,
            compiled_regex: None,
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
            for (ri, rule) in group.rules.iter().enumerate() {
                let rule_name = rule.name.clone().unwrap_or_else(|| format!("rule[{}]", ri));

                match rule.source {
                    RestrictionSource::Header | RestrictionSource::Cookie | RestrictionSource::Query => {
                        if rule.key.is_none() || rule.key.as_ref().is_some_and(|k| k.is_empty()) {
                            return Some(format!(
                                "Group '{}' rule '{}': 'key' is required for source {:?}",
                                group_name, rule_name, rule.source
                            ));
                        }
                    }
                    RestrictionSource::Path
                    | RestrictionSource::RequestPath
                    | RestrictionSource::Method
                    | RestrictionSource::Referer => {}
                }

                // A predicate needs a match set, unless it is a pure presence check
                // (onMissing: Match with no values/regex => matches only when absent).
                let has_match = rule.values.is_some() || rule.regex.is_some();
                if !has_match && rule.on_missing != OnMissing::Match {
                    return Some(format!(
                        "Group '{}' rule '{}': at least one of 'values' or 'regex' must be specified (unless onMissing is Match)",
                        group_name, rule_name
                    ));
                }

                if let Some(values) = &rule.values {
                    if values.is_empty() {
                        return Some(format!(
                            "Group '{}' rule '{}': 'values' list cannot be empty",
                            group_name, rule_name
                        ));
                    }
                }
                if let Some(patterns) = &rule.regex {
                    if patterns.is_empty() {
                        return Some(format!(
                            "Group '{}' rule '{}': 'regex' list cannot be empty",
                            group_name, rule_name
                        ));
                    }
                    let combined = patterns
                        .iter()
                        .map(|p| format!("({})", p))
                        .collect::<Vec<_>>()
                        .join("|");
                    if let Err(e) = Regex::new(&combined) {
                        return Some(format!(
                            "Group '{}' rule '{}': Invalid regex: {}",
                            group_name, rule_name, e
                        ));
                    }
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

impl RestrictionRule {
    /// Compile the predicate: normalize Path exact values, build the HashSet,
    /// and merge regex patterns.
    ///
    /// For `Path`-source rules, exact-match `values` are normalized at load time
    /// via [`normalize_config_path`] so they inhabit the same normalized space as
    /// the request path produced by `session.get_path()`. `RequestPath`-source and
    /// all other sources are left as-is (raw form). Regex patterns are never
    /// normalized.
    pub fn compile(&mut self) -> Result<(), String> {
        let rule_name = self.name.as_deref().unwrap_or("<unnamed>");

        if self.source == RestrictionSource::Path {
            let path_cfg = crate::path_normalization::get_path_normalization_config();
            if let Some(values) = self.values.as_mut() {
                for v in values.iter_mut() {
                    let normalized = normalize_config_path(v, path_cfg);
                    if normalized != *v {
                        tracing::info!(
                            "request_restriction rule '{}': Path value normalized: {:?} -> {:?}",
                            rule_name,
                            v,
                            normalized
                        );
                        *v = normalized;
                    }
                }
            }
        }

        if let Some(values) = &self.values {
            if values.is_empty() {
                return Err("'values' list cannot be empty".to_string());
            }
            if values.len() > VALUES_HASHSET_THRESHOLD {
                self.values_set = Some(if self.case_sensitive {
                    values.iter().cloned().collect()
                } else {
                    // ASCII-only case folding to stay consistent with the linear
                    // path's `eq_ignore_ascii_case` regardless of list size.
                    values.iter().map(|v| v.to_ascii_lowercase()).collect()
                });
            }
        }

        if let Some(patterns) = &self.regex {
            if patterns.is_empty() {
                return Err("'regex' list cannot be empty".to_string());
            }
            let combined = patterns
                .iter()
                .map(|p| format!("({})", p))
                .collect::<Vec<_>>()
                .join("|");
            self.compiled_regex = Some(Regex::new(&combined).map_err(|e| format!("Invalid regex: {}", e))?);
        }

        Ok(())
    }

    /// True iff the present value is in `values` OR matches `regex`.
    fn matches_value(&self, value: &str) -> bool {
        if let Some(set) = &self.values_set {
            // ASCII-only folding mirrors the linear path's `eq_ignore_ascii_case`.
            let cv = if self.case_sensitive {
                value.to_string()
            } else {
                value.to_ascii_lowercase()
            };
            if set.contains(&cv) {
                return true;
            }
        } else if let Some(values) = &self.values {
            for v in values {
                let m = if self.case_sensitive {
                    value == v
                } else {
                    value.eq_ignore_ascii_case(v)
                };
                if m {
                    return true;
                }
            }
        }

        if let Some(re) = &self.compiled_regex {
            if re.is_match(value) {
                return true;
            }
        }

        false
    }

    /// Evaluate the predicate.
    ///
    /// Returns: `Some(true)` = holds, `Some(false)` = does not hold,
    /// `None` = skip (drop from the group's AND).
    pub fn evaluate(&self, value: Option<&str>) -> Option<bool> {
        match value {
            None => match self.on_missing {
                OnMissing::Match => Some(true),
                OnMissing::NoMatch => Some(false),
                OnMissing::Skip => None,
            },
            Some(v) => Some(self.matches_value(v)),
        }
    }

    /// Get rule display name.
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{:?}:{}", self.source, self.key.as_deref().unwrap_or("")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_rule(key: &str, values: &[&str]) -> RestrictionRule {
        RestrictionRule {
            source: RestrictionSource::Header,
            key: Some(key.to_string()),
            values: Some(values.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

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
    fn test_evaluate_values_and_regex() {
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("User-Agent".to_string()),
            values: Some(vec!["curl".to_string()]),
            regex: Some(vec!["(?i).*bot.*".to_string()]),
            ..Default::default()
        };
        r.compile().unwrap();
        assert_eq!(r.evaluate(Some("curl")), Some(true)); // exact
        assert_eq!(r.evaluate(Some("Googlebot")), Some(true)); // regex
        assert_eq!(r.evaluate(Some("Mozilla")), Some(false)); // neither
    }

    #[test]
    fn test_evaluate_on_missing_three_states() {
        let mut m = header_rule("X", &["a"]);
        m.on_missing = OnMissing::Match;
        let mut n = header_rule("X", &["a"]);
        n.on_missing = OnMissing::NoMatch;
        let mut s = header_rule("X", &["a"]);
        s.on_missing = OnMissing::Skip;
        m.compile().unwrap();
        n.compile().unwrap();
        s.compile().unwrap();
        assert_eq!(m.evaluate(None), Some(true));
        assert_eq!(n.evaluate(None), Some(false));
        assert_eq!(s.evaluate(None), None);
    }

    #[test]
    fn test_case_insensitive_values() {
        let mut r = RestrictionRule {
            source: RestrictionSource::Method,
            values: Some(vec!["get".to_string()]),
            case_sensitive: false,
            ..Default::default()
        };
        r.compile().unwrap();
        assert_eq!(r.evaluate(Some("GET")), Some(true));
    }

    #[test]
    fn test_case_insensitive_ascii_only_folding_on_hashset_path() {
        // case_sensitive=false uses ASCII-only folding on BOTH the linear (<=16)
        // and the HashSet (>16) path. Build a >16 list to force the HashSet path
        // and confirm non-ASCII chars are not folded (consistent with the linear
        // path's eq_ignore_ascii_case).
        let mut values: Vec<String> = (0..20).map(|i| format!("v{}", i)).collect();
        values.push("ABC".to_string());
        values.push("CAFÉ".to_string()); // É (U+00C9) is not folded by ASCII lowercasing
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X".to_string()),
            values: Some(values),
            case_sensitive: false,
            ..Default::default()
        };
        r.compile().unwrap();
        assert!(r.values_set.is_some()); // HashSet path active
                                         // ASCII letters fold: "abc" matches "ABC".
        assert_eq!(r.evaluate(Some("abc")), Some(true));
        // Same-case non-ASCII still matches itself.
        assert_eq!(r.evaluate(Some("CAFÉ")), Some(true));
        // Non-ASCII is NOT folded: lowercase "café" does not match "CAFÉ".
        assert_eq!(r.evaluate(Some("café")), Some(false));
    }

    #[test]
    fn test_hashset_optimization() {
        let values: Vec<String> = (0..20).map(|i| format!("value{}", i)).collect();
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X-Value".to_string()),
            values: Some(values),
            ..Default::default()
        };
        r.compile().unwrap();
        assert!(r.values_set.is_some()); // HashSet compiled above threshold
        assert_eq!(r.evaluate(Some("value5")), Some(true));
        assert_eq!(r.evaluate(Some("value99")), Some(false));
    }

    #[test]
    fn test_no_hashset_below_threshold() {
        let values: Vec<String> = (0..16).map(|i| format!("value{}", i)).collect();
        let mut r = RestrictionRule {
            source: RestrictionSource::Header,
            key: Some("X-Value".to_string()),
            values: Some(values),
            ..Default::default()
        };
        r.compile().unwrap();
        assert!(r.values_set.is_none()); // 16 == threshold, not above
        assert_eq!(r.evaluate(Some("value5")), Some(true));
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

    // ------------------------------------------------------------------
    // normalize_config_path integration
    // ------------------------------------------------------------------

    #[test]
    fn path_values_normalized_at_compile() {
        // "/admin/./" → mid-string `/./` removed; trailing `/` preserved.
        // "/api%2Fv1" → %2F decoded to "/api/v1"
        let mut rule = RestrictionRule {
            name: Some("test-path-norm".to_string()),
            source: RestrictionSource::Path,
            values: Some(vec!["/admin/./".to_string(), "/api%2Fv1".to_string()]),
            ..Default::default()
        };
        rule.compile().expect("compile should succeed");

        let values = rule.values.as_ref().expect("values should be Some");
        assert_eq!(values[0], "/admin/", "mid-string `/./` removed; trailing `/` preserved");
        assert_eq!(values[1], "/api/v1", "encoded slash should be decoded");
    }

    #[test]
    fn path_values_percent_decoded_at_compile() {
        let mut rule = RestrictionRule {
            name: Some("test-path-allow-norm".to_string()),
            source: RestrictionSource::Path,
            values: Some(vec!["/%61dmin".to_string()]),
            ..Default::default()
        };
        rule.compile().expect("compile should succeed");

        let values = rule.values.as_ref().expect("values should be Some");
        assert_eq!(values[0], "/admin", "unreserved percent-encoded char should be decoded");
    }

    #[test]
    fn request_path_values_kept_raw() {
        // RequestPath source: values must NOT be normalized.
        let raw = "/api%2Fv1".to_string();
        let mut rule = RestrictionRule {
            name: Some("test-rp-raw".to_string()),
            source: RestrictionSource::RequestPath,
            values: Some(vec![raw.clone()]),
            ..Default::default()
        };
        rule.compile().expect("compile should succeed");

        let values = rule.values.as_ref().expect("values should be Some");
        assert_eq!(values[0], raw, "RequestPath values must be kept as-is (raw form)");
    }

    #[test]
    fn path_regex_not_normalized() {
        let pattern = "^/api%2Fv[0-9]+/.*".to_string();
        let mut rule = RestrictionRule {
            name: Some("test-regex-untouched".to_string()),
            source: RestrictionSource::Path,
            regex: Some(vec![pattern.clone()]),
            ..Default::default()
        };
        rule.compile().expect("compile should succeed");

        let regex = rule.regex.as_ref().expect("regex should be Some");
        assert_eq!(regex[0], pattern, "regex patterns must not be normalized");
    }
}
