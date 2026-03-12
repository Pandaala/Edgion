//! Plugin condition types for conditional execution
//!
//! This module defines the condition structures that allow plugins to be
//! conditionally executed based on various criteria such as:
//! - Key existence/matching in headers, query params, cookies
//! - Time ranges (before/after specific timestamps)
//! - Probability-based execution (for canary/sampling)
//! - Include/exclude rules for paths, IPs, etc.
//!
//! ## Condition Evaluation Logic
//!
//! ```text
//! Request arrives
//!     │
//!     ▼
//! Check skip conditions (OR logic)
//!     │
//!     ├─ Any satisfied ──► Skip plugin
//!     │
//!     ▼ None satisfied
//! Check run conditions (AND logic)
//!     │
//!     ├─ All satisfied ──► Execute plugin
//!     │
//!     └─ Not all satisfied ──► Skip plugin
//! ```
//!
//! ## YAML Example
//!
//! ```yaml
//! conditions:
//!   skip:
//!     - keyExist:
//!         key:
//!           type: header
//!           name: "X-Internal-Request"
//!     - exclude:
//!         key:
//!           type: path
//!         values: ["/health", "/ready"]
//!   run:
//!     - timeRange:
//!         after: "2024-01-01T00:00:00Z"
//!         before: "2025-12-31T23:59:59Z"
//!     - probability:
//!         ratio: 0.1
//! ```

use crate::types::common::KeyGet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Threshold for pre-compiling values into a HashSet for O(1) lookup.
/// When values.len() > this threshold, a HashSet will be built during compilation.
pub const VALUES_HASHSET_THRESHOLD: usize = 16;

/// Plugin conditions configuration
///
/// Defines when a plugin should be executed or skipped.
/// - `skip`: If ANY condition is satisfied, skip the plugin (OR logic)
/// - `run`: ALL conditions must be satisfied to run the plugin (AND logic)
///
/// When both are specified, `skip` is evaluated first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginConditions {
    /// Conditions that will cause the plugin to be skipped (OR logic)
    /// If any condition in this list is satisfied, the plugin will not run
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<Vec<Condition>>,

    /// Conditions that must be satisfied for the plugin to run (AND logic)
    /// All conditions in this list must be satisfied for the plugin to execute
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Vec<Condition>>,
}

/// Condition type enumeration
///
/// Each variant represents a different type of condition check.
/// Uses internally tagged enum for YAML representation:
/// ```yaml
/// - type: keyExist
///   key:
///     type: header
///     name: "X-Test"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Condition {
    /// Check if a key exists in the specified source
    KeyExist(KeyExistCondition),

    /// Check if a key matches a specific value or pattern
    KeyMatch(KeyMatchCondition),

    /// Check if current time is within a specified range
    TimeRange(TimeRangeCondition),

    /// Execute with a specified probability
    Probability(ProbabilityCondition),

    /// Include only if value matches any in the list
    Include(IncludeCondition),

    /// Exclude if value matches any in the list
    Exclude(ExcludeCondition),
}

/// Key existence condition
///
/// Checks if a specified key exists in the given source.
///
/// ## Example
/// ```yaml
/// keyExist:
///   key:
///     type: header
///     name: "X-Request-ID"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyExistCondition {
    /// The key to check for existence (unified KeyGet accessor)
    pub key: KeyGet,
}

/// Key match condition
///
/// Checks if a key's value matches a specific value or regex pattern.
/// At least one of `value`, `values`, or `regex` must be specified.
///
/// ## Example (exact match - single)
/// ```yaml
/// keyMatch:
///   key:
///     type: header
///     name: "X-Environment"
///   value: "production"
/// ```
///
/// ## Example (exact match - multiple values)
/// ```yaml
/// keyMatch:
///   key:
///     type: header
///     name: "X-Environment"
///   values:
///     - "production"
///     - "staging"
/// ```
///
/// ## Example (regex match - multiple patterns)
/// ```yaml
/// keyMatch:
///   key:
///     type: header
///     name: "User-Agent"
///   regex:
///     - "^Mozilla.*"
///     - "^Chrome.*"
///     - "(?i:^safari.*)"  # case-insensitive
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyMatchCondition {
    /// The key to match (unified KeyGet accessor)
    pub key: KeyGet,

    /// Exact value to match (case-sensitive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,

    /// Multiple exact values to match (any match satisfies, OR logic)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,

    /// Regex patterns to match (merged into single regex with | operator)
    /// Use (?i:pattern) for case-insensitive matching of specific patterns
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<Vec<String>>,

    /// Pre-compiled HashSet for O(1) lookup when values.len() > 16
    #[serde(skip)]
    #[schemars(skip)]
    pub values_set: Option<HashSet<String>>,

    /// Compiled regex (runtime only, merged from all patterns)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<regex::Regex>,
}

/// Time range condition
///
/// Checks if the current time falls within a specified range.
/// Both `after` and `before` are optional - if omitted, that bound is not checked.
///
/// ## Example
/// ```yaml
/// timeRange:
///   after: "2024-01-01T00:00:00Z"
///   before: "2024-12-31T23:59:59Z"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeRangeCondition {
    /// Condition is satisfied only after this time (RFC3339 format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    /// Condition is satisfied only before this time (RFC3339 format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// Probability condition
///
/// Executes with a specified probability. Useful for:
/// - Canary deployments (gradual rollout)
/// - Sampling for debugging/logging
/// - A/B testing
///
/// ## Example (10% probability)
/// ```yaml
/// probability:
///   ratio: 0.1
/// ```
///
/// ## Example (deterministic sampling based on user ID)
/// ```yaml
/// probability:
///   ratio: 0.1
///   key:
///     type: header
///     name: "X-User-ID"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProbabilityCondition {
    /// Probability ratio (0.0 to 1.0)
    /// 0.1 means 10% chance of execution
    pub ratio: f64,

    /// Optional key for deterministic sampling (unified KeyGet accessor)
    /// When specified, the same key value will always produce the same result
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeyGet>,
}

/// Include condition
///
/// Condition is satisfied if the key value matches ANY value in the list
/// or any regex pattern.
/// For path matching, supports prefix matching with `*` suffix.
///
/// ## Example (path include with wildcards)
/// ```yaml
/// include:
///   key:
///     type: path
///   values:
///     - "/api/*"
///     - "/admin/*"
/// ```
///
/// ## Example (method include)
/// ```yaml
/// include:
///   key:
///     type: method
///   values: ["GET", "POST"]
/// ```
///
/// ## Example (with regex patterns)
/// ```yaml
/// include:
///   key:
///     type: path
///   values:
///     - "/static/*"
///   regex:
///     - "^/api/v[0-9]+/.*"
///     - "(?i:^/internal/.*)"  # case-insensitive
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncludeCondition {
    /// The key to check (unified KeyGet accessor)
    pub key: KeyGet,

    /// List of values/patterns to match against (supports wildcards: *, prefix*, *suffix)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,

    /// Regex patterns to match (merged into single regex with | operator)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<Vec<String>>,

    /// Pre-compiled HashSet for O(1) lookup of exact values (no wildcards)
    #[serde(skip)]
    #[schemars(skip)]
    pub values_set: Option<HashSet<String>>,

    /// Compiled regex (runtime only, merged from all patterns)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<regex::Regex>,
}

/// Exclude condition
///
/// Condition is satisfied if the key value does NOT match ANY value in the list
/// or any regex pattern.
/// This is the inverse of IncludeCondition.
///
/// ## Example (exclude health check paths)
/// ```yaml
/// exclude:
///   key:
///     type: path
///   values:
///     - "/health"
///     - "/ready"
///     - "/metrics"
/// ```
///
/// ## Example (with regex patterns)
/// ```yaml
/// exclude:
///   key:
///     type: path
///   values:
///     - "/internal/*"
///   regex:
///     - "^/debug/.*"
///     - "^/admin/.*"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExcludeCondition {
    /// The key to check (unified KeyGet accessor)
    pub key: KeyGet,

    /// List of values/patterns to exclude (supports wildcards: *, prefix*, *suffix)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,

    /// Regex patterns to exclude (merged into single regex with | operator)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<Vec<String>>,

    /// Pre-compiled HashSet for O(1) lookup of exact values (no wildcards)
    #[serde(skip)]
    #[schemars(skip)]
    pub values_set: Option<HashSet<String>>,

    /// Compiled regex (runtime only, merged from all patterns)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<regex::Regex>,
}

impl PluginConditions {
    /// Create an empty conditions (no skip, no run conditions)
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if there are any conditions defined
    pub fn is_empty(&self) -> bool {
        self.skip.as_ref().is_none_or(|v| v.is_empty()) && self.run.as_ref().is_none_or(|v| v.is_empty())
    }

    /// Add a skip condition
    pub fn add_skip(mut self, condition: Condition) -> Self {
        self.skip.get_or_insert_with(Vec::new).push(condition);
        self
    }

    /// Add a run condition
    pub fn add_run(mut self, condition: Condition) -> Self {
        self.run.get_or_insert_with(Vec::new).push(condition);
        self
    }
}

impl KeyMatchCondition {
    /// Compile the condition: merge regex patterns and build HashSet for large value lists
    /// Returns Err if any regex pattern is invalid
    pub fn compile(&mut self) -> Result<(), regex::Error> {
        // 1. Merge multiple regex patterns into a single regex with | operator
        if let Some(patterns) = &self.regex {
            if !patterns.is_empty() {
                let combined = patterns.join("|");
                self.compiled_regex = Some(regex::Regex::new(&combined)?);
            }
        }

        // 2. Build HashSet for O(1) lookup when values exceed threshold
        if let Some(values) = &self.values {
            if values.len() > VALUES_HASHSET_THRESHOLD {
                self.values_set = Some(values.iter().cloned().collect());
            }
        }

        Ok(())
    }

    /// Backward compatible alias for compile()
    #[deprecated(note = "Use compile() instead")]
    pub fn compile_regex(&mut self) -> Result<(), regex::Error> {
        self.compile()
    }
}

impl IncludeCondition {
    /// Compile the condition: merge regex patterns and build HashSet for exact values
    /// Returns Err if any regex pattern is invalid
    pub fn compile(&mut self) -> Result<(), regex::Error> {
        // 1. Merge multiple regex patterns into a single regex with | operator
        if let Some(patterns) = &self.regex {
            if !patterns.is_empty() {
                let combined = patterns.join("|");
                self.compiled_regex = Some(regex::Regex::new(&combined)?);
            }
        }

        // 2. Extract exact values (without wildcards) and build HashSet if above threshold
        if let Some(values) = &self.values {
            let exact_values: Vec<_> = values.iter().filter(|v| !v.contains('*')).cloned().collect();
            if exact_values.len() > VALUES_HASHSET_THRESHOLD {
                self.values_set = Some(exact_values.into_iter().collect());
            }
        }

        Ok(())
    }
}

impl ExcludeCondition {
    /// Compile the condition: merge regex patterns and build HashSet for exact values
    /// Returns Err if any regex pattern is invalid
    pub fn compile(&mut self) -> Result<(), regex::Error> {
        // 1. Merge multiple regex patterns into a single regex with | operator
        if let Some(patterns) = &self.regex {
            if !patterns.is_empty() {
                let combined = patterns.join("|");
                self.compiled_regex = Some(regex::Regex::new(&combined)?);
            }
        }

        // 2. Extract exact values (without wildcards) and build HashSet if above threshold
        if let Some(values) = &self.values {
            let exact_values: Vec<_> = values.iter().filter(|v| !v.contains('*')).cloned().collect();
            if exact_values.len() > VALUES_HASHSET_THRESHOLD {
                self.values_set = Some(exact_values.into_iter().collect());
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_conditions_default() {
        let conditions = PluginConditions::default();
        assert!(conditions.is_empty());
        assert!(conditions.skip.is_none());
        assert!(conditions.run.is_none());
    }

    #[test]
    fn test_plugin_conditions_builder() {
        let conditions = PluginConditions::new()
            .add_skip(Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
            }))
            .add_run(Condition::TimeRange(TimeRangeCondition {
                after: Some("2024-01-01T00:00:00Z".to_string()),
                before: None,
            }));

        assert!(!conditions.is_empty());
        assert_eq!(conditions.skip.as_ref().unwrap().len(), 1);
        assert_eq!(conditions.run.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_serialize_deserialize() {
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Test".to_string(),
                },
            })]),
            run: Some(vec![Condition::Probability(ProbabilityCondition {
                ratio: 0.5,
                key: None,
            })]),
        };

        let yaml = serde_yaml::to_string(&conditions).unwrap();
        let deserialized: PluginConditions = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(deserialized.skip.as_ref().unwrap().len(), 1);
        assert_eq!(deserialized.run.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_key_match_compile_single_regex() {
        let mut condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "User-Agent".to_string(),
            },
            value: None,
            values: None,
            regex: Some(vec![r"^Mozilla.*".to_string()]),
            values_set: None,
            compiled_regex: None,
        };

        assert!(condition.compile().is_ok());
        assert!(condition.compiled_regex.is_some());

        let regex = condition.compiled_regex.as_ref().unwrap();
        assert!(regex.is_match("Mozilla/5.0"));
        assert!(!regex.is_match("Chrome/100.0"));
    }

    #[test]
    fn test_key_match_compile_multi_regex() {
        let mut condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "User-Agent".to_string(),
            },
            value: None,
            values: None,
            regex: Some(vec![
                r"^Mozilla.*".to_string(),
                r"^Chrome.*".to_string(),
                r"(?i:^safari.*)".to_string(), // case-insensitive
            ]),
            values_set: None,
            compiled_regex: None,
        };

        assert!(condition.compile().is_ok());
        assert!(condition.compiled_regex.is_some());

        let regex = condition.compiled_regex.as_ref().unwrap();
        // Should match Mozilla
        assert!(regex.is_match("Mozilla/5.0"));
        // Should match Chrome
        assert!(regex.is_match("Chrome/100.0"));
        // Should match Safari (case-insensitive)
        assert!(regex.is_match("Safari/605.1"));
        assert!(regex.is_match("safari/605.1"));
        assert!(regex.is_match("SAFARI/605.1"));
        // Should not match curl
        assert!(!regex.is_match("curl/7.64.1"));
    }

    #[test]
    fn test_key_match_compile_hashset() {
        // Create more than 16 values to trigger HashSet compilation
        let values: Vec<String> = (0..20).map(|i| format!("value{}", i)).collect();

        let mut condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
            value: None,
            values: Some(values.clone()),
            regex: None,
            values_set: None,
            compiled_regex: None,
        };

        assert!(condition.compile().is_ok());
        assert!(condition.values_set.is_some());

        let set = condition.values_set.as_ref().unwrap();
        assert_eq!(set.len(), 20);
        assert!(set.contains("value0"));
        assert!(set.contains("value19"));
        assert!(!set.contains("value20"));
    }

    #[test]
    fn test_key_match_no_hashset_below_threshold() {
        // Create exactly 16 values (threshold, should NOT trigger HashSet)
        let values: Vec<String> = (0..16).map(|i| format!("value{}", i)).collect();

        let mut condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
            value: None,
            values: Some(values),
            regex: None,
            values_set: None,
            compiled_regex: None,
        };

        assert!(condition.compile().is_ok());
        assert!(condition.values_set.is_none()); // Should NOT be compiled
    }

    #[test]
    fn test_include_compile_regex() {
        let mut condition = IncludeCondition {
            key: KeyGet::Path,
            values: Some(vec!["/static/*".to_string()]),
            regex: Some(vec![r"^/api/v[0-9]+/.*".to_string(), r"^/internal/.*".to_string()]),
            values_set: None,
            compiled_regex: None,
        };

        assert!(condition.compile().is_ok());
        assert!(condition.compiled_regex.is_some());

        let regex = condition.compiled_regex.as_ref().unwrap();
        assert!(regex.is_match("/api/v1/users"));
        assert!(regex.is_match("/api/v2/orders"));
        assert!(regex.is_match("/internal/debug"));
        assert!(!regex.is_match("/public/index.html"));
    }

    #[test]
    fn test_exclude_compile_regex() {
        let mut condition = ExcludeCondition {
            key: KeyGet::Path,
            values: Some(vec!["/health".to_string()]),
            regex: Some(vec![r"^/debug/.*".to_string(), r"^/metrics/.*".to_string()]),
            values_set: None,
            compiled_regex: None,
        };

        assert!(condition.compile().is_ok());
        assert!(condition.compiled_regex.is_some());

        let regex = condition.compiled_regex.as_ref().unwrap();
        assert!(regex.is_match("/debug/pprof"));
        assert!(regex.is_match("/metrics/prometheus"));
        assert!(!regex.is_match("/api/v1/users"));
    }

    #[test]
    fn test_key_get_serialization() {
        // KeyGet uses tagged enum with camelCase
        assert_eq!(
            serde_json::to_string(&KeyGet::ClientIp).unwrap(),
            r#"{"type":"clientIp"}"#
        );
        assert_eq!(
            serde_json::to_string(&KeyGet::Header {
                name: "X-Test".to_string()
            })
            .unwrap(),
            r#"{"type":"header","name":"X-Test"}"#
        );
        assert_eq!(serde_json::to_string(&KeyGet::Path).unwrap(), r#"{"type":"path"}"#);
    }
}
