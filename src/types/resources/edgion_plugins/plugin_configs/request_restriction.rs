//! Request Restriction plugin configuration
//!
//! This plugin restricts access based on request attributes like headers, cookies,
//! query parameters, path, method, and referer.
//!
//! Design inspired by plugins_cond module, using values + regex pattern separation
//! with automatic HashSet optimization for large value lists.

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Threshold for pre-compiling values into a HashSet for O(1) lookup.
/// When values.len() > this threshold, a HashSet will be built during compilation.
pub const VALUES_HASHSET_THRESHOLD: usize = 16;

/// Request Restriction plugin configuration
///
/// ## Features:
/// - Multiple restriction rules with various data sources
/// - Support for exact match (values) and regex match (regex)
/// - Automatic HashSet optimization for large value lists (>16)
/// - Allow/Deny lists with configurable priority (deny wins)
/// - Customizable rejection response
///
/// ## Usage Examples:
///
/// ### Block specific User-Agent patterns (regex):
/// ```yaml
/// rules:
///   - name: "block-bots"
///     source: Header
///     key: "User-Agent"
///     denyRegex:
///       - "(?i).*Bot.*"
///       - "(?i).*Spider.*"
///     onMissing: Allow
/// ```
///
/// ### Whitelist allowed methods (exact):
/// ```yaml
/// rules:
///   - name: "allow-methods"
///     source: Method
///     allow:
///       - "GET"
///       - "POST"
///       - "HEAD"
///     onMissing: Deny
/// ```
///
/// ### Combined exact and regex:
/// ```yaml
/// rules:
///   - name: "block-paths"
///     source: Path
///     deny:
///       - "/admin"
///       - "/internal"
///     denyRegex:
///       - "^/api/v[0-9]+/admin/.*"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestRestrictionConfig {
    /// List of restriction rules to evaluate
    pub rules: Vec<RestrictionRule>,

    /// How to combine rule results (default: Any)
    /// - Any: Reject if ANY rule triggers denial
    /// - All: Reject only if ALL rules trigger denial
    #[serde(default)]
    pub match_mode: RuleMatchMode,

    /// HTTP status code for rejection (default: 403)
    #[serde(default = "default_status")]
    pub status: u16,

    /// Custom rejection message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Validation error message (runtime only)
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

/// A single restriction rule
///
/// Uses values + regex separation pattern (similar to plugins_cond):
/// - `allow` / `deny`: Exact match values (auto HashSet when >16)
/// - `allowRegex` / `denyRegex`: Regex patterns (merged with |)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestrictionRule {
    /// Rule name for logging and debugging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Data source to extract value from
    pub source: RestrictionSource,

    /// Key to extract (required for Header, Cookie, Query; ignored for Path, Method, Referer)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    // === Allow list (whitelist) ===
    /// Exact values to allow (if configured, only matching values pass)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,

    /// Regex patterns to allow (merged with | operator)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_regex: Option<Vec<String>>,

    // === Deny list (blacklist, takes precedence over allow) ===
    /// Exact values to deny
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,

    /// Regex patterns to deny (merged with | operator)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_regex: Option<Vec<String>>,

    /// Case sensitivity for exact match (default: true)
    /// Note: For regex, use (?i:pattern) for case-insensitive matching
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,

    /// Behavior when value is missing (default: Allow)
    #[serde(default)]
    pub on_missing: OnMissing,

    // === Runtime-only fields (compiled matchers) ===
    /// HashSet for O(1) allow lookup (when allow.len() > 16)
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) allow_set: Option<HashSet<String>>,

    /// Compiled allow regex (merged from all allowRegex patterns)
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) allow_compiled_regex: Option<Regex>,

    /// HashSet for O(1) deny lookup (when deny.len() > 16)
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) deny_set: Option<HashSet<String>>,

    /// Compiled deny regex (merged from all denyRegex patterns)
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) deny_compiled_regex: Option<Regex>,
}

/// Data source for restriction rule
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
    /// HTTP method (no key needed)
    Method,
    /// Referer header (no key needed)
    Referer,
}

/// Behavior when the target value is missing
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub enum OnMissing {
    /// Allow request when value is missing (default)
    #[default]
    Allow,
    /// Deny request when value is missing
    Deny,
    /// Skip this rule when value is missing
    Skip,
}

/// How to combine multiple rule results
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub enum RuleMatchMode {
    /// Reject if ANY rule triggers denial (default, more secure)
    #[default]
    Any,
    /// Reject only if ALL rules trigger denial
    All,
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
            rules: Vec::new(),
            match_mode: RuleMatchMode::default(),
            status: default_status(),
            message: None,
            validation_error: None,
        }
    }
}

impl Default for RestrictionRule {
    fn default() -> Self {
        Self {
            name: None,
            source: RestrictionSource::Header,
            key: None,
            allow: None,
            allow_regex: None,
            deny: None,
            deny_regex: None,
            case_sensitive: default_case_sensitive(),
            on_missing: OnMissing::default(),
            allow_set: None,
            allow_compiled_regex: None,
            deny_set: None,
            deny_compiled_regex: None,
        }
    }
}

impl RequestRestrictionConfig {
    /// Validate configuration and compile matchers
    pub fn validate(&mut self) {
        if let Err(e) = self.validate_and_compile() {
            self.validation_error = Some(e);
        }
    }

    /// Check if configuration is valid
    pub fn is_valid(&self) -> bool {
        self.validation_error.is_none()
    }

    /// Get validation error message
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    /// Validate and compile all rules
    fn validate_and_compile(&mut self) -> Result<(), String> {
        // Must have at least one rule
        if self.rules.is_empty() {
            return Err("At least one rule must be specified".to_string());
        }

        // Validate status code
        if self.status < 100 || self.status >= 600 {
            return Err(format!("Invalid status code: {}", self.status));
        }

        // Validate and compile each rule
        for (i, rule) in self.rules.iter_mut().enumerate() {
            let rule_name = rule.name.clone().unwrap_or_else(|| format!("rule[{}]", i));

            // Validate key requirement based on source
            match rule.source {
                RestrictionSource::Header | RestrictionSource::Cookie | RestrictionSource::Query => {
                    if rule.key.is_none() || rule.key.as_ref().is_some_and(|k| k.is_empty()) {
                        return Err(format!(
                            "Rule '{}': 'key' is required for source {:?}",
                            rule_name, rule.source
                        ));
                    }
                }
                RestrictionSource::Path | RestrictionSource::Method | RestrictionSource::Referer => {
                    // Key is not needed for these sources
                }
            }

            // Must have at least one of allow/allowRegex or deny/denyRegex
            let has_allow = rule.allow.is_some() || rule.allow_regex.is_some();
            let has_deny = rule.deny.is_some() || rule.deny_regex.is_some();
            if !has_allow && !has_deny {
                return Err(format!(
                    "Rule '{}': at least one of 'allow', 'allowRegex', 'deny', or 'denyRegex' must be specified",
                    rule_name
                ));
            }

            // Compile rule
            rule.compile().map_err(|e| format!("Rule '{}': {}", rule_name, e))?;
        }

        Ok(())
    }
}

impl RestrictionRule {
    /// Compile the rule: build HashSets and merge regex patterns
    pub fn compile(&mut self) -> Result<(), String> {
        // Compile allow values
        if let Some(values) = &self.allow {
            if values.is_empty() {
                return Err("'allow' list cannot be empty".to_string());
            }
            if values.len() > VALUES_HASHSET_THRESHOLD {
                self.allow_set = Some(if self.case_sensitive {
                    values.iter().cloned().collect()
                } else {
                    values.iter().map(|v| v.to_lowercase()).collect()
                });
            }
        }

        // Compile allow regex
        if let Some(patterns) = &self.allow_regex {
            if patterns.is_empty() {
                return Err("'allowRegex' list cannot be empty".to_string());
            }
            let combined = patterns.join("|");
            self.allow_compiled_regex = Some(Regex::new(&combined).map_err(|e| format!("Invalid allowRegex: {}", e))?);
        }

        // Compile deny values
        if let Some(values) = &self.deny {
            if values.is_empty() {
                return Err("'deny' list cannot be empty".to_string());
            }
            if values.len() > VALUES_HASHSET_THRESHOLD {
                self.deny_set = Some(if self.case_sensitive {
                    values.iter().cloned().collect()
                } else {
                    values.iter().map(|v| v.to_lowercase()).collect()
                });
            }
        }

        // Compile deny regex
        if let Some(patterns) = &self.deny_regex {
            if patterns.is_empty() {
                return Err("'denyRegex' list cannot be empty".to_string());
            }
            let combined = patterns.join("|");
            self.deny_compiled_regex = Some(Regex::new(&combined).map_err(|e| format!("Invalid denyRegex: {}", e))?);
        }

        Ok(())
    }

    /// Check if a value matches the deny list
    fn matches_deny(&self, value: &str) -> bool {
        // Check deny HashSet (O(1) for large lists)
        if let Some(set) = &self.deny_set {
            let check_value = if self.case_sensitive {
                value.to_string()
            } else {
                value.to_lowercase()
            };
            if set.contains(&check_value) {
                return true;
            }
        } else if let Some(values) = &self.deny {
            // Check deny values (O(n) for small lists)
            for v in values {
                let matches = if self.case_sensitive {
                    value == v
                } else {
                    value.eq_ignore_ascii_case(v)
                };
                if matches {
                    return true;
                }
            }
        }

        // Check deny regex
        if let Some(regex) = &self.deny_compiled_regex {
            if regex.is_match(value) {
                return true;
            }
        }

        false
    }

    /// Check if a value matches the allow list
    fn matches_allow(&self, value: &str) -> bool {
        // Check allow HashSet (O(1) for large lists)
        if let Some(set) = &self.allow_set {
            let check_value = if self.case_sensitive {
                value.to_string()
            } else {
                value.to_lowercase()
            };
            if set.contains(&check_value) {
                return true;
            }
        } else if let Some(values) = &self.allow {
            // Check allow values (O(n) for small lists)
            for v in values {
                let matches = if self.case_sensitive {
                    value == v
                } else {
                    value.eq_ignore_ascii_case(v)
                };
                if matches {
                    return true;
                }
            }
        }

        // Check allow regex
        if let Some(regex) = &self.allow_compiled_regex {
            if regex.is_match(value) {
                return true;
            }
        }

        false
    }

    /// Check if allow list is configured (values or regex)
    fn has_allow_list(&self) -> bool {
        self.allow.is_some() || self.allow_regex.is_some()
    }

    /// Check if a value should be denied by this rule
    /// Returns: Some(true) = denied, Some(false) = allowed, None = skip rule
    pub fn check_value(&self, value: Option<&str>) -> Option<bool> {
        // Handle missing value
        let value = match value {
            Some(v) => v,
            None => {
                return match self.on_missing {
                    OnMissing::Allow => Some(false), // Allow
                    OnMissing::Deny => Some(true),   // Deny
                    OnMissing::Skip => None,         // Skip this rule
                };
            }
        };

        // Priority 1: Check deny list (denial takes precedence)
        if self.matches_deny(value) {
            return Some(true); // Denied
        }

        // Priority 2: Check allow list (if configured)
        if self.has_allow_list() {
            if self.matches_allow(value) {
                return Some(false); // Allowed
            } else {
                // Value not in allow list = denied (whitelist mode)
                return Some(true);
            }
        }

        // No deny match and no allow list configured = allowed
        Some(false)
    }

    /// Get rule display name
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{:?}:{}", self.source, self.key.as_deref().unwrap_or("")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RequestRestrictionConfig::default();
        assert!(config.rules.is_empty());
        assert_eq!(config.match_mode, RuleMatchMode::Any);
        assert_eq!(config.status, 403);
        assert_eq!(config.message, None);
    }

    #[test]
    fn test_empty_rules_validation() {
        let mut config = RequestRestrictionConfig::default();
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("At least one rule"));
    }

    #[test]
    fn test_missing_key_for_header() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: None,
                deny: Some(vec!["bad".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("key"));
    }

    #[test]
    fn test_path_no_key_required() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Path,
                key: None, // Not required for Path
                deny_regex: Some(vec!["^/admin/.*".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());
    }

    #[test]
    fn test_exact_match() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Test".to_string()),
                deny: Some(vec!["bad".to_string(), "evil".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());

        let rule = &config.rules[0];
        assert_eq!(rule.check_value(Some("bad")), Some(true)); // Denied
        assert_eq!(rule.check_value(Some("evil")), Some(true)); // Denied
        assert_eq!(rule.check_value(Some("good")), Some(false)); // Allowed (not in deny list)
    }

    #[test]
    fn test_regex_match() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("User-Agent".to_string()),
                deny_regex: Some(vec!["(?i).*Bot.*".to_string(), "(?i).*Spider.*".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());

        let rule = &config.rules[0];
        assert_eq!(rule.check_value(Some("Googlebot/2.1")), Some(true)); // Denied
        assert_eq!(rule.check_value(Some("BaiduSpider")), Some(true)); // Denied
        assert_eq!(rule.check_value(Some("Mozilla/5.0")), Some(false)); // Allowed
    }

    #[test]
    fn test_combined_exact_and_regex() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Path,
                deny: Some(vec!["/admin".to_string(), "/internal".to_string()]),
                deny_regex: Some(vec!["^/api/.*/admin/.*".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());

        let rule = &config.rules[0];
        assert_eq!(rule.check_value(Some("/admin")), Some(true)); // Denied (exact)
        assert_eq!(rule.check_value(Some("/internal")), Some(true)); // Denied (exact)
        assert_eq!(rule.check_value(Some("/api/v1/admin/users")), Some(true)); // Denied (regex)
        assert_eq!(rule.check_value(Some("/api/users")), Some(false)); // Allowed
    }

    #[test]
    fn test_allow_list_whitelist_mode() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Method,
                allow: Some(vec!["GET".to_string(), "HEAD".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());

        let rule = &config.rules[0];
        assert_eq!(rule.check_value(Some("GET")), Some(false)); // Allowed
        assert_eq!(rule.check_value(Some("HEAD")), Some(false)); // Allowed
        assert_eq!(rule.check_value(Some("POST")), Some(true)); // Denied (not in whitelist)
    }

    #[test]
    fn test_allow_regex() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Path,
                allow_regex: Some(vec!["^/api/v[0-9]+/.*".to_string(), "^/health$".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());

        let rule = &config.rules[0];
        assert_eq!(rule.check_value(Some("/api/v1/users")), Some(false)); // Allowed
        assert_eq!(rule.check_value(Some("/api/v2/orders")), Some(false)); // Allowed
        assert_eq!(rule.check_value(Some("/health")), Some(false)); // Allowed
        assert_eq!(rule.check_value(Some("/admin")), Some(true)); // Denied (not in allow)
    }

    #[test]
    fn test_on_missing_behavior() {
        // Test OnMissing::Allow
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Auth".to_string()),
                deny: Some(vec!["invalid".to_string()]),
                on_missing: OnMissing::Allow,
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        let rule = &config.rules[0];
        assert_eq!(rule.check_value(None), Some(false)); // Allow when missing

        // Test OnMissing::Deny
        let mut config2 = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Auth".to_string()),
                deny: Some(vec!["invalid".to_string()]),
                on_missing: OnMissing::Deny,
                ..Default::default()
            }],
            ..Default::default()
        };
        config2.validate();
        let rule2 = &config2.rules[0];
        assert_eq!(rule2.check_value(None), Some(true)); // Deny when missing

        // Test OnMissing::Skip
        let mut config3 = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Auth".to_string()),
                deny: Some(vec!["invalid".to_string()]),
                on_missing: OnMissing::Skip,
                ..Default::default()
            }],
            ..Default::default()
        };
        config3.validate();
        let rule3 = &config3.rules[0];
        assert_eq!(rule3.check_value(None), None); // Skip rule
    }

    #[test]
    fn test_case_insensitive_exact() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Test".to_string()),
                deny: Some(vec!["BadValue".to_string()]),
                case_sensitive: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());

        let rule = &config.rules[0];
        assert_eq!(rule.check_value(Some("badvalue")), Some(true)); // Denied (case insensitive)
        assert_eq!(rule.check_value(Some("BADVALUE")), Some(true)); // Denied
        assert_eq!(rule.check_value(Some("BadValue")), Some(true)); // Denied
    }

    #[test]
    fn test_deny_takes_precedence() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Path,
                allow_regex: Some(vec!["^/api/.*".to_string()]),
                deny_regex: Some(vec!["^/api/admin/.*".to_string()]),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());

        let rule = &config.rules[0];
        assert_eq!(rule.check_value(Some("/api/users")), Some(false)); // Allowed
        assert_eq!(rule.check_value(Some("/api/admin/users")), Some(true)); // Denied (deny wins)
    }

    #[test]
    fn test_hashset_optimization() {
        // Create more than 16 values to trigger HashSet compilation
        let values: Vec<String> = (0..20).map(|i| format!("value{}", i)).collect();

        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Value".to_string()),
                deny: Some(values),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());

        let rule = &config.rules[0];
        assert!(rule.deny_set.is_some()); // HashSet should be compiled

        assert_eq!(rule.check_value(Some("value5")), Some(true)); // Denied
        assert_eq!(rule.check_value(Some("value19")), Some(true)); // Denied
        assert_eq!(rule.check_value(Some("value99")), Some(false)); // Allowed
    }

    #[test]
    fn test_no_hashset_below_threshold() {
        // Create exactly 16 values (threshold, should NOT trigger HashSet)
        let values: Vec<String> = (0..16).map(|i| format!("value{}", i)).collect();

        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Value".to_string()),
                deny: Some(values),
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());

        let rule = &config.rules[0];
        assert!(rule.deny_set.is_none()); // HashSet should NOT be compiled
    }

    #[test]
    fn test_invalid_regex() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Test".to_string()),
                deny_regex: Some(vec!["[invalid".to_string()]), // Invalid regex
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("Invalid"));
    }

    #[test]
    fn test_empty_deny_list() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Test".to_string()),
                deny: Some(vec![]), // Empty list
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("cannot be empty"));
    }

    #[test]
    fn test_no_allow_or_deny() {
        let mut config = RequestRestrictionConfig {
            rules: vec![RestrictionRule {
                source: RestrictionSource::Header,
                key: Some("X-Test".to_string()),
                // No allow or deny specified
                ..Default::default()
            }],
            ..Default::default()
        };
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("at least one"));
    }
}
