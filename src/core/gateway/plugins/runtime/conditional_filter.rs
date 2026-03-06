//! Conditional filter wrappers for condition-based plugin execution
//!
//! This module provides wrapper types that add condition evaluation to any filter.
//! Conditions are evaluated before the inner filter runs, allowing plugins to be
//! skipped based on runtime context (headers, path, time, probability, etc.)

use async_trait::async_trait;

use std::time::Duration;

use super::log::PluginLog;
use super::traits::{
    PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use crate::core::gateway::plugins::runtime::conditions::{EvaluationResult, PluginConditions};
use crate::types::filters::PluginRunningResult;

// ==================== PluginSession Integration ====================
//
// NOTE: PluginConditions.evaluate() now accepts &dyn PluginSession directly.
// PluginSession also has key_get() and key_set() methods for unified value access.

// ==================== ConditionalRequestFilter ====================

/// Wrapper that adds condition evaluation to a RequestFilter
///
/// Before running the inner filter, conditions are evaluated:
/// - If conditions is None or empty, the inner filter runs unconditionally
/// - If conditions evaluate to Skip, the filter is skipped and `skipped_by_condition` is set
/// - If conditions evaluate to Run, the inner filter executes normally
pub struct ConditionalRequestFilter {
    inner: Box<dyn RequestFilter>,
    conditions: Option<PluginConditions>,
}

impl ConditionalRequestFilter {
    /// Create a new conditional request filter
    pub fn new(inner: Box<dyn RequestFilter>, conditions: Option<PluginConditions>) -> Self {
        Self { inner, conditions }
    }

    /// Create a wrapper without conditions (always runs)
    #[allow(dead_code)]
    pub fn without_conditions(inner: Box<dyn RequestFilter>) -> Self {
        Self::new(inner, None)
    }
}

#[async_trait]
impl RequestFilter for ConditionalRequestFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Check conditions before running
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = conditions.evaluate_detail(session).await;
                if eval.result == EvaluationResult::Skip {
                    if let Some(cond) = eval.matched {
                        log.set_cond_skip(format!("{}:{},{}", eval.action, cond.cond_type(), cond.cond_detail()));
                    }
                    return PluginRunningResult::Nothing;
                }
            }
        }

        // Conditions satisfied or none defined, run the inner filter
        self.inner.run_request(session, log).await
    }
}

// ==================== ConditionalUpstreamResponseFilter ====================

/// Wrapper that adds condition evaluation to an UpstreamResponseFilter (sync)
pub struct ConditionalUpstreamResponseFilter {
    inner: Box<dyn UpstreamResponseFilter>,
    conditions: Option<PluginConditions>,
}

impl ConditionalUpstreamResponseFilter {
    /// Create a new conditional upstream response filter
    pub fn new(inner: Box<dyn UpstreamResponseFilter>, conditions: Option<PluginConditions>) -> Self {
        Self { inner, conditions }
    }

    /// Create a wrapper without conditions (always runs)
    #[allow(dead_code)]
    pub fn without_conditions(inner: Box<dyn UpstreamResponseFilter>) -> Self {
        Self::new(inner, None)
    }
}

impl UpstreamResponseFilter for ConditionalUpstreamResponseFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Check conditions before running (sync version for sync filter context)
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = conditions.evaluate_detail_sync(session);
                if eval.result == EvaluationResult::Skip {
                    if let Some(cond) = eval.matched {
                        log.set_cond_skip(format!("{}:{},{}", eval.action, cond.cond_type(), cond.cond_detail()));
                    }
                    return PluginRunningResult::Nothing;
                }
            }
        }

        // Conditions satisfied or none defined, run the inner filter
        self.inner.run_upstream_response_filter(session, log)
    }
}

// ==================== ConditionalUpstreamResponseBodyFilter ====================

/// Wrapper that adds condition evaluation to an UpstreamResponseBodyFilter (sync)
///
/// Before running the inner filter, conditions are evaluated:
/// - If conditions is None or empty, the inner filter runs unconditionally
/// - If conditions evaluate to Skip, the filter is skipped and returns None
/// - If conditions evaluate to Run, the inner filter executes normally
pub struct ConditionalUpstreamResponseBodyFilter {
    inner: Box<dyn UpstreamResponseBodyFilter>,
    conditions: Option<PluginConditions>,
}

impl ConditionalUpstreamResponseBodyFilter {
    /// Create a new conditional upstream response body filter
    pub fn new(inner: Box<dyn UpstreamResponseBodyFilter>, conditions: Option<PluginConditions>) -> Self {
        Self { inner, conditions }
    }

    /// Create a wrapper without conditions (always runs)
    #[allow(dead_code)]
    pub fn without_conditions(inner: Box<dyn UpstreamResponseBodyFilter>) -> Self {
        Self::new(inner, None)
    }
}

impl UpstreamResponseBodyFilter for ConditionalUpstreamResponseBodyFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Option<Duration> {
        // Check conditions before running (sync version for sync filter context)
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = conditions.evaluate_detail_sync(session);
                if eval.result == EvaluationResult::Skip {
                    if let Some(cond) = eval.matched {
                        log.set_cond_skip(format!("{}:{},{}", eval.action, cond.cond_type(), cond.cond_detail()));
                    }
                    return None;
                }
            }
        }

        // Conditions satisfied or none defined, run the inner filter
        self.inner
            .run_upstream_response_body_filter(body, end_of_stream, session, log)
    }
}

// ==================== ConditionalUpstreamResponse ====================

/// Wrapper that adds condition evaluation to an UpstreamResponse (async)
pub struct ConditionalUpstreamResponse {
    inner: Box<dyn UpstreamResponse>,
    conditions: Option<PluginConditions>,
}

impl ConditionalUpstreamResponse {
    /// Create a new conditional upstream response handler
    pub fn new(inner: Box<dyn UpstreamResponse>, conditions: Option<PluginConditions>) -> Self {
        Self { inner, conditions }
    }

    /// Create a wrapper without conditions (always runs)
    #[allow(dead_code)]
    pub fn without_conditions(inner: Box<dyn UpstreamResponse>) -> Self {
        Self::new(inner, None)
    }
}

#[async_trait]
impl UpstreamResponse for ConditionalUpstreamResponse {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Check conditions before running
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = conditions.evaluate_detail(session).await;
                if eval.result == EvaluationResult::Skip {
                    if let Some(cond) = eval.matched {
                        log.set_cond_skip(format!("{}:{},{}", eval.action, cond.cond_type(), cond.cond_detail()));
                    }
                    return PluginRunningResult::Nothing;
                }
            }
        }

        // Conditions satisfied or none defined, run the inner filter
        self.inner.run_upstream_response(session, log).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;
    use crate::core::gateway::plugins::runtime::conditions::{Condition, IncludeCondition, KeyExistCondition};
    use crate::types::common::KeyGet;
    use mockall::predicate::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ==================== Mock Request Filter ====================

    struct MockRequestFilter {
        name: String,
        run_count: AtomicUsize,
    }

    impl MockRequestFilter {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                run_count: AtomicUsize::new(0),
            }
        }

        #[allow(dead_code)]
        fn run_count(&self) -> usize {
            self.run_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RequestFilter for MockRequestFilter {
        fn name(&self) -> &str {
            &self.name
        }

        async fn run_request(&self, _session: &mut dyn PluginSession, _log: &mut PluginLog) -> PluginRunningResult {
            self.run_count.fetch_add(1, Ordering::SeqCst);
            PluginRunningResult::Nothing
        }
    }

    // ==================== Tests ====================

    #[test]
    fn test_conditional_filter_no_conditions() {
        // When conditions is None, should_evaluate returns false
        let conditions = PluginConditions::default();
        assert!(!conditions.should_evaluate());
    }

    #[test]
    fn test_conditional_filter_with_skip_condition() {
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Skip".to_string(),
                },
            })]),
            run: None,
        };
        assert!(conditions.should_evaluate());
    }

    #[tokio::test]
    async fn test_conditional_filter_runs_when_no_conditions() {
        let inner = MockRequestFilter::new("test-filter");
        let filter = ConditionalRequestFilter::new(Box::new(inner), None);

        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("test");

        let result = filter.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::Nothing);
        assert!(!log.is_cond_skipped());
    }

    #[tokio::test]
    async fn test_conditional_filter_runs_when_conditions_satisfied() {
        let inner = MockRequestFilter::new("test-filter");

        // Run condition: method must be POST
        let conditions = PluginConditions {
            skip: None,
            run: Some(vec![Condition::Include(IncludeCondition {
                key: KeyGet::Method,
                values: Some(vec!["POST".to_string()]),
                regex: None,
                values_set: None,
                compiled_regex: None,
            })]),
        };

        let filter = ConditionalRequestFilter::new(Box::new(inner), Some(conditions));

        let mut session = MockPluginSession::new();
        session
            .expect_key_get()
            .with(eq(KeyGet::Method))
            .return_const(Some("POST".to_string()));
        let mut log = PluginLog::new("test");

        let result = filter.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::Nothing);
        assert!(!log.is_cond_skipped());
    }

    #[tokio::test]
    async fn test_conditional_filter_skips_when_run_condition_not_met() {
        let inner = MockRequestFilter::new("test-filter");

        // Run condition: method must be POST
        let conditions = PluginConditions {
            skip: None,
            run: Some(vec![Condition::Include(IncludeCondition {
                key: KeyGet::Method,
                values: Some(vec!["POST".to_string()]),
                regex: None,
                values_set: None,
                compiled_regex: None,
            })]),
        };

        let filter = ConditionalRequestFilter::new(Box::new(inner), Some(conditions));

        // But request is GET
        let mut session = MockPluginSession::new();
        session
            .expect_key_get()
            .with(eq(KeyGet::Method))
            .return_const(Some("GET".to_string()));
        let mut log = PluginLog::new("test");

        let result = filter.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::Nothing);
        assert!(log.is_cond_skipped()); // Should be skipped
    }

    #[tokio::test]
    async fn test_conditional_filter_skips_when_skip_condition_met() {
        let inner = MockRequestFilter::new("test-filter");

        // Skip if header X-Internal exists
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
            })]),
            run: None,
        };

        let filter = ConditionalRequestFilter::new(Box::new(inner), Some(conditions));

        // Request has X-Internal header
        let mut session = MockPluginSession::new();
        session
            .expect_key_get()
            .with(eq(KeyGet::Header {
                name: "X-Internal".to_string(),
            }))
            .return_const(Some("true".to_string()));

        let mut log = PluginLog::new("test");

        let result = filter.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::Nothing);
        assert!(log.is_cond_skipped()); // Should be skipped
    }

    #[tokio::test]
    async fn test_conditional_filter_runs_when_skip_condition_not_met() {
        let inner = MockRequestFilter::new("test-filter");

        // Skip if header X-Internal exists
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
            })]),
            run: None,
        };

        let filter = ConditionalRequestFilter::new(Box::new(inner), Some(conditions));

        // Request does NOT have X-Internal header
        let mut session = MockPluginSession::new();
        session
            .expect_key_get()
            .with(eq(KeyGet::Header {
                name: "X-Internal".to_string(),
            }))
            .return_const(None);

        let mut log = PluginLog::new("test");

        let result = filter.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::Nothing);
        assert!(!log.is_cond_skipped()); // Should run
    }

    #[tokio::test]
    async fn test_condition_log_recorded_on_skip() {
        let inner = MockRequestFilter::new("test-filter");

        // Skip if header X-Internal exists
        let conditions = PluginConditions {
            skip: Some(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
            })]),
            run: None,
        };

        let filter = ConditionalRequestFilter::new(Box::new(inner), Some(conditions));

        // Request has X-Internal header - should skip
        let mut session = MockPluginSession::new();
        session
            .expect_key_get()
            .with(eq(KeyGet::Header {
                name: "X-Internal".to_string(),
            }))
            .return_const(Some("true".to_string()));
        let mut log = PluginLog::new("test");

        let _ = filter.run_request(&mut session, &mut log).await;

        assert!(log.is_cond_skipped());
        // Verify cond_skip contains condition info
        assert_eq!(log.cond_skip.as_deref(), Some("skip:keyExist,hdr:X-Internal"));
    }

    #[tokio::test]
    async fn test_condition_log_recorded_on_run_not_met() {
        let inner = MockRequestFilter::new("test-filter");

        // Run condition: method must be POST
        let conditions = PluginConditions {
            skip: None,
            run: Some(vec![Condition::Include(IncludeCondition {
                key: KeyGet::Method,
                values: Some(vec!["POST".to_string()]),
                regex: None,
                values_set: None,
                compiled_regex: None,
            })]),
        };

        let filter = ConditionalRequestFilter::new(Box::new(inner), Some(conditions));

        // Request is GET - run condition not met
        let mut session = MockPluginSession::new();
        session
            .expect_key_get()
            .with(eq(KeyGet::Method))
            .return_const(Some("GET".to_string()));
        let mut log = PluginLog::new("test");

        let _ = filter.run_request(&mut session, &mut log).await;

        assert!(log.is_cond_skipped());
        // Verify cond_skip contains condition info (prefixed with ! for run condition not met)
        assert_eq!(log.cond_skip.as_deref(), Some("!run:include,method"));
    }

    #[test]
    fn test_condition_type_and_detail() {
        // Test keyExist
        let c1 = Condition::KeyExist(KeyExistCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
        });
        assert_eq!(c1.cond_type(), "keyExist");
        assert_eq!(c1.cond_detail(), "hdr:X-Test");

        // Test include
        let c2 = Condition::Include(IncludeCondition {
            key: KeyGet::Method,
            values: Some(vec!["GET".to_string(), "POST".to_string()]),
            regex: None,
            values_set: None,
            compiled_regex: None,
        });
        assert_eq!(c2.cond_type(), "include");
        assert_eq!(c2.cond_detail(), "method");

        // Test probability
        let c3 =
            Condition::Probability(crate::core::gateway::plugins::runtime::conditions::ProbabilityCondition { ratio: 0.1, key: None });
        assert_eq!(c3.cond_type(), "prob");
        assert_eq!(c3.cond_detail(), "10%");
    }
}
