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
//!         source: header
//!         key: "X-Internal-Request"
//!     - exclude:
//!         source: path
//!         values: ["/health", "/ready"]
//!   run:
//!     - timeRange:
//!         after: "2024-01-01T00:00:00Z"
//!         before: "2025-12-31T23:59:59Z"
//!     - probability:
//!         ratio: 0.1
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
///   source: header
///   key: "X-Test"
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

/// Data source for condition evaluation
///
/// Specifies where to look for the value being checked
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConditionSource {
    /// HTTP request header
    Header,

    /// URL query parameter
    Query,

    /// HTTP cookie
    Cookie,

    /// Request path (e.g., "/api/v1/users")
    Path,

    /// Client IP address (after real IP extraction)
    ClientIp,

    /// HTTP method (GET, POST, etc.)
    Method,

    /// Context variable (set by other plugins or system)
    Ctx,
}

impl ConditionSource {
    /// Get a short string representation for logging
    pub fn as_str(&self) -> &'static str {
        match self {
            ConditionSource::Header => "hdr",
            ConditionSource::Query => "qry",
            ConditionSource::Cookie => "cke",
            ConditionSource::Path => "path",
            ConditionSource::ClientIp => "ip",
            ConditionSource::Method => "mtd",
            ConditionSource::Ctx => "ctx",
        }
    }
}

/// Key existence condition
///
/// Checks if a specified key exists in the given source.
///
/// ## Example
/// ```yaml
/// keyExist:
///   source: header
///   key: "X-Request-ID"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyExistCondition {
    /// Where to look for the key
    pub source: ConditionSource,

    /// The key name to check for existence
    pub key: String,
}

/// Key match condition
///
/// Checks if a key's value matches a specific value or regex pattern.
/// At least one of `value` or `regex` must be specified.
///
/// ## Example (exact match)
/// ```yaml
/// keyMatch:
///   source: header
///   key: "X-Environment"
///   value: "production"
/// ```
///
/// ## Example (regex match)
/// ```yaml
/// keyMatch:
///   source: header
///   key: "User-Agent"
///   regex: "^Mozilla.*"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyMatchCondition {
    /// Where to look for the key
    pub source: ConditionSource,

    /// The key name to match
    pub key: String,

    /// Exact value to match (case-sensitive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,

    /// Regex pattern to match against the value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,

    /// Compiled regex (runtime only, not serialized)
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
///   key: "X-User-ID"
///   keySource: header
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProbabilityCondition {
    /// Probability ratio (0.0 to 1.0)
    /// 0.1 means 10% chance of execution
    pub ratio: f64,

    /// Optional key for deterministic sampling
    /// When specified, the same key value will always produce the same result
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    /// Source for the deterministic key (defaults to Header if key is specified)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_source: Option<ConditionSource>,
}

/// Include condition
///
/// Condition is satisfied if the source value matches ANY value in the list.
/// For path matching, supports prefix matching with `*` suffix.
///
/// ## Example (path include)
/// ```yaml
/// include:
///   source: path
///   values:
///     - "/api/*"
///     - "/admin/*"
/// ```
///
/// ## Example (method include)
/// ```yaml
/// include:
///   source: method
///   values: ["GET", "POST"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncludeCondition {
    /// Where to get the value to check
    pub source: ConditionSource,

    /// List of values to match against (any match satisfies the condition)
    pub values: Vec<String>,
}

/// Exclude condition
///
/// Condition is satisfied if the source value does NOT match ANY value in the list.
/// This is the inverse of IncludeCondition.
///
/// ## Example (exclude health check paths)
/// ```yaml
/// exclude:
///   source: path
///   values:
///     - "/health"
///     - "/ready"
///     - "/metrics"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExcludeCondition {
    /// Where to get the value to check
    pub source: ConditionSource,

    /// List of values to exclude (any match means condition is NOT satisfied)
    pub values: Vec<String>,
}

impl PluginConditions {
    /// Create an empty conditions (no skip, no run conditions)
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if there are any conditions defined
    pub fn is_empty(&self) -> bool {
        self.skip.as_ref().map_or(true, |v| v.is_empty())
            && self.run.as_ref().map_or(true, |v| v.is_empty())
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
    /// Compile the regex pattern if specified
    /// Returns Err if the regex is invalid
    pub fn compile_regex(&mut self) -> Result<(), regex::Error> {
        if let Some(pattern) = &self.regex {
            self.compiled_regex = Some(regex::Regex::new(pattern)?);
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
                source: ConditionSource::Header,
                key: "X-Internal".to_string(),
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
                source: ConditionSource::Header,
                key: "X-Test".to_string(),
            })]),
            run: Some(vec![Condition::Probability(ProbabilityCondition {
                ratio: 0.5,
                key: None,
                key_source: None,
            })]),
        };

        let yaml = serde_yaml::to_string(&conditions).unwrap();
        let deserialized: PluginConditions = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(deserialized.skip.as_ref().unwrap().len(), 1);
        assert_eq!(deserialized.run.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_key_match_compile_regex() {
        let mut condition = KeyMatchCondition {
            source: ConditionSource::Header,
            key: "User-Agent".to_string(),
            value: None,
            regex: Some(r"^Mozilla.*".to_string()),
            compiled_regex: None,
        };

        assert!(condition.compile_regex().is_ok());
        assert!(condition.compiled_regex.is_some());

        let regex = condition.compiled_regex.unwrap();
        assert!(regex.is_match("Mozilla/5.0"));
        assert!(!regex.is_match("Chrome/100.0"));
    }

    #[test]
    fn test_condition_source_serialization() {
        assert_eq!(
            serde_json::to_string(&ConditionSource::Header).unwrap(),
            "\"header\""
        );
        assert_eq!(
            serde_json::to_string(&ConditionSource::ClientIp).unwrap(),
            "\"client_ip\""
        );
    }
}
