//! Condition evaluator for plugin conditional execution
//!
//! This module provides the logic to evaluate conditions at runtime
//! to determine whether a plugin should be executed or skipped.

use super::{
    Condition, ExcludeCondition, IncludeCondition, KeyExistCondition, KeyMatchCondition, PluginConditions,
    ProbabilityCondition, TimeRangeCondition,
};
use crate::core::gateway::plugins::runtime::log::PluginLog;
use crate::core::gateway::plugins::runtime::PluginSession;
use chrono::Utc;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};

/// Result of condition evaluation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationResult {
    /// Plugin should run
    Run,
    /// Plugin should be skipped
    Skip,
}

/// Result of condition evaluation with matched condition info
pub struct ConditionEvalResult<'a> {
    pub result: EvaluationResult,
    /// The action that caused skip: "skip" or "!run"
    pub action: &'static str,
    /// The condition that matched (for logging)
    pub matched: Option<&'a Condition>,
}

impl PluginConditions {
    /// Evaluate whether the plugin should run based on conditions
    pub async fn evaluate(&self, session: &dyn PluginSession) -> EvaluationResult {
        let mut log = PluginLog::new("cond");
        self.evaluate_detail(session, &mut log).await.result
    }

    /// Evaluate conditions and return matched condition for logging
    pub async fn evaluate_detail<'a>(
        &'a self,
        session: &dyn PluginSession,
        log: &mut PluginLog,
    ) -> ConditionEvalResult<'a> {
        // Check skip conditions (OR logic) - any match causes skip
        if let Some(skip_conditions) = &self.skip {
            for condition in skip_conditions {
                if condition.evaluate(session, log).await {
                    return ConditionEvalResult {
                        result: EvaluationResult::Skip,
                        action: "skip",
                        matched: Some(condition),
                    };
                }
            }
        }

        // Check run conditions (AND logic) - all must match to run
        if let Some(run_conditions) = &self.run {
            for condition in run_conditions {
                if !condition.evaluate(session, log).await {
                    return ConditionEvalResult {
                        result: EvaluationResult::Skip,
                        action: "!run",
                        matched: Some(condition),
                    };
                }
            }
        }

        ConditionEvalResult {
            result: EvaluationResult::Run,
            action: "",
            matched: None,
        }
    }

    /// Synchronous, log-less variant of `evaluate` for body-stage filters.
    ///
    /// Body filters run per-chunk and do not surface PluginLog entries, so this
    /// path dispatches through `Condition::evaluate_sync_silent` and never
    /// allocates a PluginLog. The only diagnostic that the regular sync path
    /// would emit ("regex not compiled") is silently dropped — that condition
    /// indicates a build-time compile failure and should already be visible
    /// through `EdgionPlugin::validation_error`.
    pub fn evaluate_sync(&self, session: &dyn PluginSession) -> EvaluationResult {
        if let Some(skip_conditions) = &self.skip {
            for condition in skip_conditions {
                if condition.evaluate_sync_silent(session) {
                    return EvaluationResult::Skip;
                }
            }
        }

        if let Some(run_conditions) = &self.run {
            for condition in run_conditions {
                if !condition.evaluate_sync_silent(session) {
                    return EvaluationResult::Skip;
                }
            }
        }

        EvaluationResult::Run
    }

    /// Synchronous version of evaluate_detail for use in sync filter contexts
    /// (UpstreamResponseFilter, UpstreamResponseBodyFilter).
    ///
    /// This uses key_get_local() which only supports local key sources.
    /// Conditions should never use Webhook as a key source, so this is always safe.
    pub fn evaluate_detail_sync<'a>(
        &'a self,
        session: &dyn PluginSession,
        log: &mut PluginLog,
    ) -> ConditionEvalResult<'a> {
        // Check skip conditions (OR logic) - any match causes skip
        if let Some(skip_conditions) = &self.skip {
            for condition in skip_conditions {
                if condition.evaluate_sync(session, log) {
                    return ConditionEvalResult {
                        result: EvaluationResult::Skip,
                        action: "skip",
                        matched: Some(condition),
                    };
                }
            }
        }

        // Check run conditions (AND logic) - all must match to run
        if let Some(run_conditions) = &self.run {
            for condition in run_conditions {
                if !condition.evaluate_sync(session, log) {
                    return ConditionEvalResult {
                        result: EvaluationResult::Skip,
                        action: "!run",
                        matched: Some(condition),
                    };
                }
            }
        }

        ConditionEvalResult {
            result: EvaluationResult::Run,
            action: "",
            matched: None,
        }
    }

    /// Check if conditions should be evaluated
    pub fn should_evaluate(&self) -> bool {
        !self.is_empty()
    }
}

impl Condition {
    /// Evaluate a single condition
    pub async fn evaluate(&self, session: &dyn PluginSession, log: &mut PluginLog) -> bool {
        match self {
            Condition::KeyExist(c) => c.evaluate(session).await,
            Condition::KeyMatch(c) => c.evaluate(session, log).await,
            Condition::TimeRange(c) => c.evaluate(),
            Condition::Probability(c) => c.evaluate(session).await,
            Condition::Include(c) => c.evaluate(session, log).await,
            Condition::Exclude(c) => c.evaluate(session, log).await,
        }
    }

    /// Synchronous version of evaluate for use in sync filter contexts.
    /// Uses key_get_local() which only supports local key sources.
    pub fn evaluate_sync(&self, session: &dyn PluginSession, log: &mut PluginLog) -> bool {
        match self {
            Condition::KeyExist(c) => c.evaluate_sync(session),
            Condition::KeyMatch(c) => c.evaluate_sync(session, log),
            Condition::TimeRange(c) => c.evaluate(),
            Condition::Probability(c) => c.evaluate_sync(session),
            Condition::Include(c) => c.evaluate_sync(session, log),
            Condition::Exclude(c) => c.evaluate_sync(session, log),
        }
    }

    /// Log-less synchronous evaluation for body-stage filters.
    ///
    /// KeyExist / TimeRange / Probability never write to a log; KeyMatch /
    /// Include / Exclude only push "regex not compiled" on a build-time bug
    /// path, which is intentionally suppressed here.
    pub fn evaluate_sync_silent(&self, session: &dyn PluginSession) -> bool {
        match self {
            Condition::KeyExist(c) => c.evaluate_sync(session),
            Condition::KeyMatch(c) => c.evaluate_sync_silent(session),
            Condition::TimeRange(c) => c.evaluate(),
            Condition::Probability(c) => c.evaluate_sync(session),
            Condition::Include(c) => c.evaluate_sync_silent(session),
            Condition::Exclude(c) => c.evaluate_sync_silent(session),
        }
    }

    /// Get condition type name
    pub fn cond_type(&self) -> &'static str {
        match self {
            Condition::KeyExist(_) => "keyExist",
            Condition::KeyMatch(_) => "keyMatch",
            Condition::TimeRange(_) => "timeRange",
            Condition::Probability(_) => "prob",
            Condition::Include(_) => "include",
            Condition::Exclude(_) => "exclude",
        }
    }

    /// Get brief condition detail: key info
    pub fn cond_detail(&self) -> String {
        match self {
            Condition::KeyExist(c) => c.key.as_log_str(),
            Condition::KeyMatch(c) => c.key.as_log_str(),
            Condition::TimeRange(_) => "time".to_string(),
            Condition::Probability(c) => format!("{:.0}%", c.ratio * 100.0),
            Condition::Include(c) => c.key.as_log_str(),
            Condition::Exclude(c) => c.key.as_log_str(),
        }
    }
}

impl KeyExistCondition {
    /// Evaluate key existence condition
    pub async fn evaluate(&self, session: &dyn PluginSession) -> bool {
        session.key_get(&self.key).await.is_some()
    }

    /// Sync version using key_get_local
    pub fn evaluate_sync(&self, session: &dyn PluginSession) -> bool {
        session.key_get_local(&self.key).is_some()
    }
}

impl KeyMatchCondition {
    /// Evaluate key match condition
    pub async fn evaluate(&self, session: &dyn PluginSession, log: &mut PluginLog) -> bool {
        let value = match session.key_get(&self.key).await {
            Some(v) => v,
            None => return false,
        };
        self.match_value(&value, Some(log))
    }

    /// Sync version using key_get_local
    pub fn evaluate_sync(&self, session: &dyn PluginSession, log: &mut PluginLog) -> bool {
        let value = match session.key_get_local(&self.key) {
            Some(v) => v,
            None => return false,
        };
        self.match_value(&value, Some(log))
    }

    /// Sync version with no log surface (body-stage path)
    pub fn evaluate_sync_silent(&self, session: &dyn PluginSession) -> bool {
        let value = match session.key_get_local(&self.key) {
            Some(v) => v,
            None => return false,
        };
        self.match_value(&value, None)
    }

    /// Check if a value matches any of the configured patterns
    fn match_value(&self, value: &str, log: Option<&mut PluginLog>) -> bool {
        // 1. Check single value (backward compatible)
        if let Some(expected) = &self.value {
            if value == expected {
                return true;
            }
        }

        // 2. Check HashSet first for O(1) lookup (when values > 16)
        if let Some(set) = &self.values_set {
            if set.contains(value) {
                return true;
            }
        } else if let Some(values) = &self.values {
            // 3. Iterate Vec for small lists (O(n))
            for expected in values {
                if value == expected {
                    return true;
                }
            }
        }

        // 4. Check compiled regex (merged from all patterns)
        if let Some(compiled) = &self.compiled_regex {
            if compiled.is_match(value) {
                return true;
            }
        } else if self.regex.as_ref().map_or(false, |p| !p.is_empty()) {
            if let Some(log) = log {
                log.push_err("regex not compiled");
            }
        }

        false
    }
}

impl TimeRangeCondition {
    /// Evaluate time range condition
    pub fn evaluate(&self) -> bool {
        let now = Utc::now();

        // Check 'after' bound
        if let Some(after_str) = &self.after {
            if let Ok(after_time) = chrono::DateTime::parse_from_rfc3339(after_str) {
                if now < after_time {
                    return false;
                }
            }
        }

        // Check 'before' bound
        if let Some(before_str) = &self.before {
            if let Ok(before_time) = chrono::DateTime::parse_from_rfc3339(before_str) {
                if now >= before_time {
                    return false;
                }
            }
        }

        true
    }
}

impl ProbabilityCondition {
    /// Evaluate probability condition
    pub async fn evaluate(&self, session: &dyn PluginSession) -> bool {
        // Clamp ratio to valid range
        let ratio = self.ratio.clamp(0.0, 1.0);

        // If ratio is 0, never execute; if 1, always execute
        if ratio <= 0.0 {
            return false;
        }
        if ratio >= 1.0 {
            return true;
        }

        // Deterministic sampling if key is specified
        if let Some(key) = &self.key {
            if let Some(key_value) = session.key_get(key).await {
                // Use hash for deterministic sampling
                let mut hasher = DefaultHasher::new();
                key_value.hash(&mut hasher);
                let hash = hasher.finish();
                let normalized = (hash as f64) / (u64::MAX as f64);
                return normalized < ratio;
            }
        }

        // Random sampling
        rand::random::<f64>() < ratio
    }

    /// Sync version using key_get_local
    pub fn evaluate_sync(&self, session: &dyn PluginSession) -> bool {
        let ratio = self.ratio.clamp(0.0, 1.0);
        if ratio <= 0.0 {
            return false;
        }
        if ratio >= 1.0 {
            return true;
        }
        if let Some(key) = &self.key {
            if let Some(key_value) = session.key_get_local(key) {
                let mut hasher = DefaultHasher::new();
                key_value.hash(&mut hasher);
                let hash = hasher.finish();
                let normalized = (hash as f64) / (u64::MAX as f64);
                return normalized < ratio;
            }
        }
        rand::random::<f64>() < ratio
    }
}

impl IncludeCondition {
    /// Evaluate include condition
    /// Returns true if value matches ANY item in the list or any regex pattern
    pub async fn evaluate(&self, session: &dyn PluginSession, log: &mut PluginLog) -> bool {
        let value = session.key_get(&self.key).await.unwrap_or_default();
        self.matches_value(&value, Some(log))
    }

    /// Sync version using key_get_local
    pub fn evaluate_sync(&self, session: &dyn PluginSession, log: &mut PluginLog) -> bool {
        let value = session.key_get_local(&self.key).unwrap_or_default();
        self.matches_value(&value, Some(log))
    }

    /// Sync version with no log surface (body-stage path)
    pub fn evaluate_sync_silent(&self, session: &dyn PluginSession) -> bool {
        let value = session.key_get_local(&self.key).unwrap_or_default();
        self.matches_value(&value, None)
    }

    /// Check if a value matches any of the configured patterns
    fn matches_value(&self, value: &str, log: Option<&mut PluginLog>) -> bool {
        matches_pattern_set(
            value,
            self.values_set.as_ref(),
            self.wildcard_patterns.as_deref(),
            self.values.as_deref(),
            self.compiled_regex.as_ref(),
            self.regex.as_deref(),
            log,
        )
    }
}

impl ExcludeCondition {
    /// Evaluate exclude condition
    /// Returns true if value does NOT match ANY item in the list or any regex pattern
    pub async fn evaluate(&self, session: &dyn PluginSession, log: &mut PluginLog) -> bool {
        let value = session.key_get(&self.key).await.unwrap_or_default();
        !self.matches_value(&value, Some(log))
    }

    /// Sync version using key_get_local
    pub fn evaluate_sync(&self, session: &dyn PluginSession, log: &mut PluginLog) -> bool {
        let value = session.key_get_local(&self.key).unwrap_or_default();
        !self.matches_value(&value, Some(log))
    }

    /// Sync version with no log surface (body-stage path)
    pub fn evaluate_sync_silent(&self, session: &dyn PluginSession) -> bool {
        let value = session.key_get_local(&self.key).unwrap_or_default();
        !self.matches_value(&value, None)
    }

    /// Check if a value matches any of the configured patterns (excluded)
    fn matches_value(&self, value: &str, log: Option<&mut PluginLog>) -> bool {
        matches_pattern_set(
            value,
            self.values_set.as_ref(),
            self.wildcard_patterns.as_deref(),
            self.values.as_deref(),
            self.compiled_regex.as_ref(),
            self.regex.as_deref(),
            log,
        )
    }
}

/// Shared pattern-set matcher used by both `IncludeCondition::matches_value` and
/// `ExcludeCondition::matches_value`. The two conditions store identical pattern data
/// and run identical lookup logic — Include returns this directly, Exclude negates it
/// in its evaluate wrappers.
#[inline]
fn matches_pattern_set(
    value: &str,
    values_set: Option<&HashSet<String>>,
    wildcard_patterns: Option<&[String]>,
    values: Option<&[String]>,
    compiled_regex: Option<&regex::Regex>,
    regex: Option<&[String]>,
    log: Option<&mut PluginLog>,
) -> bool {
    // 1. Check HashSet first for O(1) exact match lookup
    if let Some(set) = values_set {
        if set.contains(value) {
            return true;
        }
    }

    // 2. Iterate the wildcard-only bucket when compile() split the values; otherwise
    //    fall back to scanning the full `values` list (small list, no HashSet built).
    //    The `else if` is critical: when `wildcard_patterns` is present, `values_set`
    //    already covers every exact entry, so re-iterating `values` here would
    //    repeat the exact-match work for nothing on the miss path.
    if let Some(patterns) = wildcard_patterns {
        for pattern in patterns {
            if matches_pattern(value, pattern) {
                return true;
            }
        }
    } else if let Some(patterns) = values {
        for pattern in patterns {
            if matches_pattern(value, pattern) {
                return true;
            }
        }
    }

    // 3. Check compiled regex (merged from all patterns)
    if let Some(compiled) = compiled_regex {
        if compiled.is_match(value) {
            return true;
        }
    } else if regex.map_or(false, |p| !p.is_empty()) {
        if let Some(log) = log {
            log.push_err("regex not compiled");
        }
    }

    false
}

/// Match value against pattern
/// Supports:
/// - Exact match: "value"
/// - Prefix match: "prefix*"
/// - Suffix match: "*suffix"
/// - Contains: "*substring*"
fn matches_pattern(value: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let starts_with_wildcard = pattern.starts_with('*');
    let ends_with_wildcard = pattern.ends_with('*');

    match (starts_with_wildcard, ends_with_wildcard) {
        (true, true) => {
            // Contains: *substring*
            let inner = &pattern[1..pattern.len() - 1];
            value.contains(inner)
        }
        (true, false) => {
            // Suffix match: *suffix
            let suffix = &pattern[1..];
            value.ends_with(suffix)
        }
        (false, true) => {
            // Prefix match: prefix*
            let prefix = &pattern[..pattern.len() - 1];
            value.starts_with(prefix)
        }
        (false, false) => {
            // Exact match
            value == pattern
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;
    use crate::types::common::KeyGet;

    /// Create a mock session with key_get that returns values based on the key
    fn create_mock_session_with_key_get(key_values: Vec<(KeyGet, Option<String>)>) -> MockPluginSession {
        let mut mock = MockPluginSession::new();

        // Convert to owned map
        let values_map: std::collections::HashMap<String, Option<String>> =
            key_values.into_iter().map(|(k, v)| (format!("{:?}", k), v)).collect();

        mock.expect_key_get().returning(move |key| {
            let key_str = format!("{:?}", key);
            values_map.get(&key_str).cloned().flatten()
        });

        mock
    }

    #[tokio::test]
    async fn test_empty_conditions_run() {
        let conditions = PluginConditions::default();
        let mut mock = MockPluginSession::new();
        mock.expect_key_get().returning(|_| None);
        assert_eq!(conditions.evaluate(&mock).await, EvaluationResult::Run);
    }

    #[tokio::test]
    async fn test_skip_condition_matched() {
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
            })]),
            run: None,
        };

        let session = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Internal".to_string(),
            },
            Some("true".to_string()),
        )]);
        assert_eq!(conditions.evaluate(&session).await, EvaluationResult::Skip);
    }

    #[tokio::test]
    async fn test_skip_condition_not_matched() {
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
            })]),
            run: None,
        };

        let mut mock = MockPluginSession::new();
        mock.expect_key_get().returning(|_| None);
        assert_eq!(conditions.evaluate(&mock).await, EvaluationResult::Run);
    }

    #[tokio::test]
    async fn test_run_condition_all_satisfied() {
        let conditions = PluginConditions {
            skip: None,
            run: Some(vec![
                Condition::KeyExist(KeyExistCondition {
                    key: KeyGet::Header {
                        name: "Authorization".to_string(),
                    },
                }),
                Condition::Include(IncludeCondition {
                    key: KeyGet::Method,
                    values: Some(vec!["GET".to_string(), "POST".to_string()]),
                    regex: None,
                    values_set: None,
                    wildcard_patterns: None,
                    compiled_regex: None,
                }),
            ]),
        };

        let session = create_mock_session_with_key_get(vec![
            (
                KeyGet::Header {
                    name: "Authorization".to_string(),
                },
                Some("Bearer token".to_string()),
            ),
            (KeyGet::Method, Some("GET".to_string())),
        ]);
        assert_eq!(conditions.evaluate(&session).await, EvaluationResult::Run);
    }

    #[tokio::test]
    async fn test_run_condition_not_all_satisfied() {
        let conditions = PluginConditions {
            skip: None,
            run: Some(vec![
                Condition::KeyExist(KeyExistCondition {
                    key: KeyGet::Header {
                        name: "Authorization".to_string(),
                    },
                }),
                Condition::Include(IncludeCondition {
                    key: KeyGet::Method,
                    values: Some(vec!["POST".to_string()]),
                    regex: None,
                    values_set: None,
                    wildcard_patterns: None,
                    compiled_regex: None,
                }),
            ]),
        };

        // Method is GET, not POST
        let session = create_mock_session_with_key_get(vec![
            (
                KeyGet::Header {
                    name: "Authorization".to_string(),
                },
                Some("Bearer token".to_string()),
            ),
            (KeyGet::Method, Some("GET".to_string())),
        ]);
        assert_eq!(conditions.evaluate(&session).await, EvaluationResult::Skip);
    }

    #[tokio::test]
    async fn test_key_match_exact() {
        let condition = KeyMatchCondition {
            key: KeyGet::Header {
                name: "X-Environment".to_string(),
            },
            value: Some("production".to_string()),
            values: None,
            regex: None,
            values_set: None,
            compiled_regex: None,
        };

        let session = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Environment".to_string(),
            },
            Some("production".to_string()),
        )]);
        let mut log = PluginLog::new("test");
        assert!(condition.evaluate(&session, &mut log).await);

        let session2 = create_mock_session_with_key_get(vec![(
            KeyGet::Header {
                name: "X-Environment".to_string(),
            },
            Some("staging".to_string()),
        )]);
        assert!(!condition.evaluate(&session2, &mut log).await);
    }

    #[test]
    fn test_time_range_within() {
        let condition = TimeRangeCondition {
            after: Some("2020-01-01T00:00:00Z".to_string()),
            before: Some("2030-12-31T23:59:59Z".to_string()),
        };

        assert!(condition.evaluate());
    }

    #[test]
    fn test_time_range_past() {
        let condition = TimeRangeCondition {
            after: Some("2020-01-01T00:00:00Z".to_string()),
            before: Some("2020-12-31T23:59:59Z".to_string()),
        };

        assert!(!condition.evaluate());
    }

    #[test]
    fn test_time_range_validate_rejects_malformed_after() {
        // Invalid month (13) — operator typo. Must surface as a preparse error
        // (caller pushes into preparse_errors → seal_fail_closed) instead of
        // being silently dropped at evaluation time, which would leave the
        // time-gate permanently inert. See F-002.
        let condition = TimeRangeCondition {
            after: Some("2024-13-01T00:00:00Z".to_string()),
            before: None,
        };
        assert!(condition.validate().is_err());
    }

    #[test]
    fn test_time_range_validate_rejects_malformed_before() {
        let condition = TimeRangeCondition {
            after: None,
            before: Some("not-a-timestamp".to_string()),
        };
        assert!(condition.validate().is_err());
    }

    #[test]
    fn test_time_range_validate_accepts_well_formed_bounds() {
        let condition = TimeRangeCondition {
            after: Some("2020-01-01T00:00:00Z".to_string()),
            before: Some("2030-12-31T23:59:59Z".to_string()),
        };
        assert!(condition.validate().is_ok());
    }

    #[test]
    fn test_time_range_validate_accepts_unset_bounds() {
        let condition = TimeRangeCondition {
            after: None,
            before: None,
        };
        assert!(condition.validate().is_ok());
    }

    #[tokio::test]
    async fn test_probability_always() {
        let condition = ProbabilityCondition { ratio: 1.0, key: None };

        let mut mock = MockPluginSession::new();
        mock.expect_key_get().returning(|_| None);
        assert!(condition.evaluate(&mock).await);
    }

    #[tokio::test]
    async fn test_probability_never() {
        let condition = ProbabilityCondition { ratio: 0.0, key: None };

        let mut mock = MockPluginSession::new();
        mock.expect_key_get().returning(|_| None);
        assert!(!condition.evaluate(&mock).await);
    }

    #[tokio::test]
    async fn test_include_path() {
        let condition = IncludeCondition {
            key: KeyGet::Path,
            values: Some(vec!["/api/*".to_string(), "/admin/*".to_string()]),
            regex: None,
            values_set: None,
            wildcard_patterns: None,
            compiled_regex: None,
        };

        let mut log = PluginLog::new("test");
        let session1 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/api/users".to_string()))]);
        assert!(condition.evaluate(&session1, &mut log).await);

        let session2 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/admin/settings".to_string()))]);
        assert!(condition.evaluate(&session2, &mut log).await);

        let session3 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/public/index.html".to_string()))]);
        assert!(!condition.evaluate(&session3, &mut log).await);
    }

    #[tokio::test]
    async fn test_exclude_path() {
        let condition = ExcludeCondition {
            key: KeyGet::Path,
            values: Some(vec![
                "/health".to_string(),
                "/ready".to_string(),
                "/metrics".to_string(),
            ]),
            regex: None,
            values_set: None,
            wildcard_patterns: None,
            compiled_regex: None,
        };

        let mut log = PluginLog::new("test");
        let session1 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/api/users".to_string()))]);
        assert!(condition.evaluate(&session1, &mut log).await); // Not excluded

        let session2 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/health".to_string()))]);
        assert!(!condition.evaluate(&session2, &mut log).await); // Excluded

        let session3 = create_mock_session_with_key_get(vec![(KeyGet::Path, Some("/ready".to_string()))]);
        assert!(!condition.evaluate(&session3, &mut log).await); // Excluded
    }

    #[test]
    fn test_pattern_matching() {
        // Exact match
        assert!(matches_pattern("hello", "hello"));
        assert!(!matches_pattern("hello", "world"));

        // Prefix match
        assert!(matches_pattern("/api/users", "/api/*"));
        assert!(!matches_pattern("/public/index", "/api/*"));

        // Suffix match
        assert!(matches_pattern("image.png", "*.png"));
        assert!(!matches_pattern("image.jpg", "*.png"));

        // Contains
        assert!(matches_pattern("/api/v1/users", "*v1*"));
        assert!(!matches_pattern("/api/v2/users", "*v1*"));

        // Wildcard all
        assert!(matches_pattern("anything", "*"));
    }
}
