//! Condition evaluator for plugin conditional execution
//!
//! This module provides the logic to evaluate conditions at runtime
//! to determine whether a plugin should be executed or skipped.

use super::{
    Condition, ConditionSource, ExcludeCondition, IncludeCondition, KeyExistCondition,
    KeyMatchCondition, PluginConditions, ProbabilityCondition, TimeRangeCondition,
};
use chrono::Utc;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Context trait for condition evaluation
///
/// This trait abstracts the request context so conditions can be evaluated
/// without depending on the concrete session type.
pub trait ConditionContext {
    /// Get a header value by name
    fn get_header(&self, name: &str) -> Option<String>;

    /// Get a query parameter value by name
    fn get_query_param(&self, name: &str) -> Option<String>;

    /// Get a cookie value by name
    fn get_cookie(&self, name: &str) -> Option<String>;

    /// Get the request path
    fn get_path(&self) -> &str;

    /// Get the client IP address (real IP after extraction)
    fn get_client_ip(&self) -> &str;

    /// Get the HTTP method
    fn get_method(&self) -> &str;

    /// Get a context variable by key
    fn get_ctx_var(&self, key: &str) -> Option<String>;
}

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
    pub fn evaluate<C: ConditionContext>(&self, ctx: &C) -> EvaluationResult {
        self.evaluate_detail(ctx).result
    }

    /// Evaluate conditions and return matched condition for logging
    pub fn evaluate_detail<C: ConditionContext>(&self, ctx: &C) -> ConditionEvalResult<'_> {
        // Check skip conditions (OR logic) - any match causes skip
        if let Some(skip_conditions) = &self.skip {
            for condition in skip_conditions {
                if condition.evaluate(ctx) {
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
                if !condition.evaluate(ctx) {
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
    pub fn evaluate<C: ConditionContext>(&self, ctx: &C) -> bool {
        match self {
            Condition::KeyExist(c) => c.evaluate(ctx),
            Condition::KeyMatch(c) => c.evaluate(ctx),
            Condition::TimeRange(c) => c.evaluate(),
            Condition::Probability(c) => c.evaluate(ctx),
            Condition::Include(c) => c.evaluate(ctx),
            Condition::Exclude(c) => c.evaluate(ctx),
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

    /// Get brief condition detail: "source:key" or key info
    pub fn cond_detail(&self) -> String {
        match self {
            Condition::KeyExist(c) => format!("{}:{}", c.source.as_str(), c.key),
            Condition::KeyMatch(c) => format!("{}:{}", c.source.as_str(), c.key),
            Condition::TimeRange(_) => "time".to_string(),
            Condition::Probability(c) => format!("{:.0}%", c.ratio * 100.0),
            Condition::Include(c) => c.source.as_str().to_string(),
            Condition::Exclude(c) => c.source.as_str().to_string(),
        }
    }
}

impl KeyExistCondition {
    /// Evaluate key existence condition
    pub fn evaluate<C: ConditionContext>(&self, ctx: &C) -> bool {
        get_source_value(ctx, &self.source, &self.key).is_some()
    }
}

impl KeyMatchCondition {
    /// Evaluate key match condition
    pub fn evaluate<C: ConditionContext>(&self, ctx: &C) -> bool {
        let value = match get_source_value(ctx, &self.source, &self.key) {
            Some(v) => v,
            None => return false,
        };

        // Check exact match first
        if let Some(expected) = &self.value {
            if &value == expected {
                return true;
            }
        }

        // Check regex match
        if let Some(compiled) = &self.compiled_regex {
            if compiled.is_match(&value) {
                return true;
            }
        }

        // If regex is specified but not compiled, try to compile and match
        if let Some(pattern) = &self.regex {
            if self.compiled_regex.is_none() {
                if let Ok(re) = regex::Regex::new(pattern) {
                    return re.is_match(&value);
                }
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
    pub fn evaluate<C: ConditionContext>(&self, ctx: &C) -> bool {
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
            let source = self.key_source.as_ref().unwrap_or(&ConditionSource::Header);
            if let Some(key_value) = get_source_value(ctx, source, key) {
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
}

impl IncludeCondition {
    /// Evaluate include condition
    /// Returns true if value matches ANY item in the list
    pub fn evaluate<C: ConditionContext>(&self, ctx: &C) -> bool {
        let value = get_source_value_direct(ctx, &self.source);

        for pattern in &self.values {
            if matches_pattern(&value, pattern) {
                return true;
            }
        }

        false
    }
}

impl ExcludeCondition {
    /// Evaluate exclude condition
    /// Returns true if value does NOT match ANY item in the list
    pub fn evaluate<C: ConditionContext>(&self, ctx: &C) -> bool {
        let value = get_source_value_direct(ctx, &self.source);

        for pattern in &self.values {
            if matches_pattern(&value, pattern) {
                return false;
            }
        }

        true
    }
}

/// Get value from source based on key
fn get_source_value<C: ConditionContext>(ctx: &C, source: &ConditionSource, key: &str) -> Option<String> {
    match source {
        ConditionSource::Header => ctx.get_header(key),
        ConditionSource::Query => ctx.get_query_param(key),
        ConditionSource::Cookie => ctx.get_cookie(key),
        ConditionSource::Path => Some(ctx.get_path().to_string()),
        ConditionSource::ClientIp => Some(ctx.get_client_ip().to_string()),
        ConditionSource::Method => Some(ctx.get_method().to_string()),
        ConditionSource::Ctx => ctx.get_ctx_var(key),
    }
}

/// Get value directly from source (for Include/Exclude conditions)
fn get_source_value_direct<C: ConditionContext>(ctx: &C, source: &ConditionSource) -> String {
    match source {
        ConditionSource::Header => String::new(), // Headers need a key, return empty
        ConditionSource::Query => String::new(),  // Query needs a key, return empty
        ConditionSource::Cookie => String::new(), // Cookie needs a key, return empty
        ConditionSource::Path => ctx.get_path().to_string(),
        ConditionSource::ClientIp => ctx.get_client_ip().to_string(),
        ConditionSource::Method => ctx.get_method().to_string(),
        ConditionSource::Ctx => String::new(), // Ctx needs a key, return empty
    }
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

    /// Mock context for testing
    struct MockContext {
        headers: std::collections::HashMap<String, String>,
        query_params: std::collections::HashMap<String, String>,
        cookies: std::collections::HashMap<String, String>,
        path: String,
        client_ip: String,
        method: String,
        ctx_vars: std::collections::HashMap<String, String>,
    }

    impl MockContext {
        fn new() -> Self {
            Self {
                headers: std::collections::HashMap::new(),
                query_params: std::collections::HashMap::new(),
                cookies: std::collections::HashMap::new(),
                path: "/".to_string(),
                client_ip: "127.0.0.1".to_string(),
                method: "GET".to_string(),
                ctx_vars: std::collections::HashMap::new(),
            }
        }
    }

    impl ConditionContext for MockContext {
        fn get_header(&self, name: &str) -> Option<String> {
            self.headers.get(name).cloned()
        }

        fn get_query_param(&self, name: &str) -> Option<String> {
            self.query_params.get(name).cloned()
        }

        fn get_cookie(&self, name: &str) -> Option<String> {
            self.cookies.get(name).cloned()
        }

        fn get_path(&self) -> &str {
            &self.path
        }

        fn get_client_ip(&self) -> &str {
            &self.client_ip
        }

        fn get_method(&self) -> &str {
            &self.method
        }

        fn get_ctx_var(&self, key: &str) -> Option<String> {
            self.ctx_vars.get(key).cloned()
        }
    }

    #[test]
    fn test_empty_conditions_run() {
        let conditions = PluginConditions::default();
        let ctx = MockContext::new();
        assert_eq!(conditions.evaluate(&ctx), EvaluationResult::Run);
    }

    #[test]
    fn test_skip_condition_matched() {
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                source: ConditionSource::Header,
                key: "X-Internal".to_string(),
            })]),
            run: None,
        };

        let mut ctx = MockContext::new();
        ctx.headers.insert("X-Internal".to_string(), "true".to_string());

        assert_eq!(conditions.evaluate(&ctx), EvaluationResult::Skip);
    }

    #[test]
    fn test_skip_condition_not_matched() {
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                source: ConditionSource::Header,
                key: "X-Internal".to_string(),
            })]),
            run: None,
        };

        let ctx = MockContext::new();
        assert_eq!(conditions.evaluate(&ctx), EvaluationResult::Run);
    }

    #[test]
    fn test_run_condition_all_satisfied() {
        let conditions = PluginConditions {
            skip: None,
            run: Some(vec![
                Condition::KeyExist(KeyExistCondition {
                    source: ConditionSource::Header,
                    key: "Authorization".to_string(),
                }),
                Condition::Include(IncludeCondition {
                    source: ConditionSource::Method,
                    values: vec!["GET".to_string(), "POST".to_string()],
                }),
            ]),
        };

        let mut ctx = MockContext::new();
        ctx.headers
            .insert("Authorization".to_string(), "Bearer token".to_string());
        ctx.method = "GET".to_string();

        assert_eq!(conditions.evaluate(&ctx), EvaluationResult::Run);
    }

    #[test]
    fn test_run_condition_not_all_satisfied() {
        let conditions = PluginConditions {
            skip: None,
            run: Some(vec![
                Condition::KeyExist(KeyExistCondition {
                    source: ConditionSource::Header,
                    key: "Authorization".to_string(),
                }),
                Condition::Include(IncludeCondition {
                    source: ConditionSource::Method,
                    values: vec!["POST".to_string()],
                }),
            ]),
        };

        let mut ctx = MockContext::new();
        ctx.headers
            .insert("Authorization".to_string(), "Bearer token".to_string());
        ctx.method = "GET".to_string(); // Not POST

        assert_eq!(conditions.evaluate(&ctx), EvaluationResult::Skip);
    }

    #[test]
    fn test_key_match_exact() {
        let condition = KeyMatchCondition {
            source: ConditionSource::Header,
            key: "X-Environment".to_string(),
            value: Some("production".to_string()),
            regex: None,
            compiled_regex: None,
        };

        let mut ctx = MockContext::new();
        ctx.headers
            .insert("X-Environment".to_string(), "production".to_string());

        assert!(condition.evaluate(&ctx));

        ctx.headers
            .insert("X-Environment".to_string(), "staging".to_string());
        assert!(!condition.evaluate(&ctx));
    }

    #[test]
    fn test_key_match_regex() {
        let mut condition = KeyMatchCondition {
            source: ConditionSource::Header,
            key: "User-Agent".to_string(),
            value: None,
            regex: Some(r"^Mozilla.*".to_string()),
            compiled_regex: None,
        };
        condition.compile_regex().unwrap();

        let mut ctx = MockContext::new();
        ctx.headers
            .insert("User-Agent".to_string(), "Mozilla/5.0".to_string());

        assert!(condition.evaluate(&ctx));

        ctx.headers
            .insert("User-Agent".to_string(), "curl/7.64.1".to_string());
        assert!(!condition.evaluate(&ctx));
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
    fn test_probability_always() {
        let condition = ProbabilityCondition {
            ratio: 1.0,
            key: None,
            key_source: None,
        };

        let ctx = MockContext::new();
        assert!(condition.evaluate(&ctx));
    }

    #[test]
    fn test_probability_never() {
        let condition = ProbabilityCondition {
            ratio: 0.0,
            key: None,
            key_source: None,
        };

        let ctx = MockContext::new();
        assert!(!condition.evaluate(&ctx));
    }

    #[test]
    fn test_probability_deterministic() {
        let condition = ProbabilityCondition {
            ratio: 0.5,
            key: Some("X-User-ID".to_string()),
            key_source: Some(ConditionSource::Header),
        };

        let mut ctx = MockContext::new();
        ctx.headers.insert("X-User-ID".to_string(), "user123".to_string());

        // Same key should always produce same result
        let first_result = condition.evaluate(&ctx);
        for _ in 0..10 {
            assert_eq!(condition.evaluate(&ctx), first_result);
        }
    }

    #[test]
    fn test_include_path() {
        let condition = IncludeCondition {
            source: ConditionSource::Path,
            values: vec!["/api/*".to_string(), "/admin/*".to_string()],
        };

        let mut ctx = MockContext::new();

        ctx.path = "/api/users".to_string();
        assert!(condition.evaluate(&ctx));

        ctx.path = "/admin/settings".to_string();
        assert!(condition.evaluate(&ctx));

        ctx.path = "/public/index.html".to_string();
        assert!(!condition.evaluate(&ctx));
    }

    #[test]
    fn test_exclude_path() {
        let condition = ExcludeCondition {
            source: ConditionSource::Path,
            values: vec!["/health".to_string(), "/ready".to_string(), "/metrics".to_string()],
        };

        let mut ctx = MockContext::new();

        ctx.path = "/api/users".to_string();
        assert!(condition.evaluate(&ctx)); // Not excluded

        ctx.path = "/health".to_string();
        assert!(!condition.evaluate(&ctx)); // Excluded

        ctx.path = "/ready".to_string();
        assert!(!condition.evaluate(&ctx)); // Excluded
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
