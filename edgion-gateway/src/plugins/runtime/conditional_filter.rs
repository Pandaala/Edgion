//! Conditional filter wrappers for condition-based plugin execution
//!
//! This module provides wrapper types that add condition evaluation to any filter.
//! Conditions are evaluated before the inner filter runs, allowing plugins to be
//! skipped based on runtime context (headers, path, time, probability, etc.)

use async_trait::async_trait;

use std::sync::Arc;
use std::time::Duration;

use super::dye_plan::CompiledDyePlan;
use super::log::PluginLog;
use super::traits::{
    PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use crate::plugins::runtime::conditions::{CompiledPluginConditions, EvaluationResult, PluginConditions};
use edgion_resources::filters::PluginRunningResult;

/// Wrapper that adds condition evaluation to a RequestFilter
///
/// Before running the inner filter, conditions are evaluated:
/// - If conditions is `None`, or the container has neither `skip` nor `run`,
///   the inner filter runs unconditionally. Explicit empty `allOf`/`anyOf`
///   sets are rejected by canonical validation before runtime publication.
/// - If conditions evaluate to Skip, the filter is skipped and the reason is
///   recorded in [`PluginLog::cond_skip`].
/// - If conditions evaluate to Run, the inner filter executes normally
pub struct ConditionalRequestFilter {
    inner: Box<dyn RequestFilter>,
    conditions: Option<CompiledPluginConditions>,
    alias: Option<String>,
    dye_plan: Option<Arc<CompiledDyePlan>>,
}

impl ConditionalRequestFilter {
    /// Create a new conditional request filter
    pub fn new(inner: Box<dyn RequestFilter>, conditions: Option<PluginConditions>) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias: None,
            dye_plan: None,
        }
    }

    /// Create a wrapper that also carries an operator-defined alias and an
    /// optional compiled dye plan.
    pub fn with_alias_and_dye(
        inner: Box<dyn RequestFilter>,
        conditions: Option<PluginConditions>,
        alias: Option<String>,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias,
            dye_plan,
        }
    }
}

#[async_trait]
impl RequestFilter for ConditionalRequestFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        self.dye_plan.as_ref()
    }

    fn suppress_automatic_log(&self) -> bool {
        self.inner.suppress_automatic_log()
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = match conditions.evaluate_detail_async(session, log).await {
                    Ok(eval) => eval,
                    Err(result) => return result,
                };
                if eval.result == EvaluationResult::Skip {
                    if let Some(cause) = eval.cause {
                        log.set_cond_skip(format!("{}:{cause}", eval.action));
                    }
                    return PluginRunningResult::Nothing;
                }
            }
        }
        self.inner.run_request(session, log).await
    }
}

/// Wrapper that adds condition evaluation to an UpstreamResponseFilter (sync)
pub struct ConditionalUpstreamResponseFilter {
    inner: Box<dyn UpstreamResponseFilter>,
    conditions: Option<CompiledPluginConditions>,
    alias: Option<String>,
    dye_plan: Option<Arc<CompiledDyePlan>>,
}

impl ConditionalUpstreamResponseFilter {
    /// Create a new conditional upstream response filter
    pub fn new(inner: Box<dyn UpstreamResponseFilter>, conditions: Option<PluginConditions>) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias: None,
            dye_plan: None,
        }
    }

    /// Create a wrapper that also carries an operator-defined alias and an
    /// optional compiled dye plan.
    pub fn with_alias_and_dye(
        inner: Box<dyn UpstreamResponseFilter>,
        conditions: Option<PluginConditions>,
        alias: Option<String>,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias,
            dye_plan,
        }
    }
}

impl UpstreamResponseFilter for ConditionalUpstreamResponseFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        self.dye_plan.as_ref()
    }

    fn suppress_automatic_log(&self) -> bool {
        self.inner.suppress_automatic_log()
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = conditions.evaluate_detail(session, log);
                if eval.result == EvaluationResult::Skip {
                    if let Some(cause) = eval.cause {
                        log.set_cond_skip(format!("{}:{cause}", eval.action));
                    }
                    return PluginRunningResult::Nothing;
                }
            }
        }
        self.inner.run_upstream_response_filter(session, log)
    }
}

/// Wrapper that adds condition evaluation to an UpstreamResponseBodyFilter (sync)
///
/// Before running the inner filter, conditions are evaluated:
/// - If conditions is `None`, or the container has neither `skip` nor `run`,
///   the inner filter runs unconditionally. Explicit empty `allOf`/`anyOf`
///   sets are rejected by canonical validation before runtime publication.
/// - If conditions evaluate to Skip, the filter is skipped and returns `None`
///   without recording a per-chunk log.
/// - If conditions evaluate to Run, the inner filter executes normally
pub struct ConditionalUpstreamResponseBodyFilter {
    inner: Box<dyn UpstreamResponseBodyFilter>,
    conditions: Option<CompiledPluginConditions>,
    alias: Option<String>,
    dye_plan: Option<Arc<CompiledDyePlan>>,
}

impl ConditionalUpstreamResponseBodyFilter {
    /// Create a new conditional upstream response body filter
    pub fn new(inner: Box<dyn UpstreamResponseBodyFilter>, conditions: Option<PluginConditions>) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias: None,
            dye_plan: None,
        }
    }

    /// Create a wrapper that also carries an operator-defined alias and an
    /// optional compiled dye plan.
    pub fn with_alias_and_dye(
        inner: Box<dyn UpstreamResponseBodyFilter>,
        conditions: Option<PluginConditions>,
        alias: Option<String>,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias,
            dye_plan,
        }
    }
}

impl UpstreamResponseBodyFilter for ConditionalUpstreamResponseBodyFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        self.dye_plan.as_ref()
    }

    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> Option<Duration> {
        // Body stage runs per chunk and does not surface PluginLog entries, so we
        // use the log-less evaluate variant. Skip-cause is intentionally not
        // recorded; emitting it per chunk would explode log volume.
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() && conditions.evaluate(session) == EvaluationResult::Skip {
                return None;
            }
        }

        self.inner
            .run_upstream_response_body_filter(body, end_of_stream, session)
    }
}

/// Wrapper that adds condition evaluation to an UpstreamResponse (async)
pub struct ConditionalUpstreamResponse {
    inner: Box<dyn UpstreamResponse>,
    conditions: Option<CompiledPluginConditions>,
    alias: Option<String>,
    dye_plan: Option<Arc<CompiledDyePlan>>,
}

impl ConditionalUpstreamResponse {
    /// Create a new conditional upstream response handler
    pub fn new(inner: Box<dyn UpstreamResponse>, conditions: Option<PluginConditions>) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias: None,
            dye_plan: None,
        }
    }

    /// Create a wrapper that also carries an operator-defined alias and an
    /// optional compiled dye plan.
    pub fn with_alias_and_dye(
        inner: Box<dyn UpstreamResponse>,
        conditions: Option<PluginConditions>,
        alias: Option<String>,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias,
            dye_plan,
        }
    }
}

#[async_trait]
impl UpstreamResponse for ConditionalUpstreamResponse {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        self.dye_plan.as_ref()
    }

    fn suppress_automatic_log(&self) -> bool {
        self.inner.suppress_automatic_log()
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = match conditions.evaluate_detail_async(session, log).await {
                    Ok(eval) => eval,
                    Err(result) => return result,
                };
                if eval.result == EvaluationResult::Skip {
                    if let Some(cause) = eval.cause {
                        log.set_cond_skip(format!("{}:{cause}", eval.action));
                    }
                    return PluginRunningResult::Nothing;
                }
            }
        }
        self.inner.run_upstream_response(session, log).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::conditions::{Condition, KeyExistCondition, KeyMatchCondition};
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::conditions::{ConditionSet, MatchCore};
    use mockall::predicate::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingFilter {
        name: String,
        run_count: Arc<AtomicUsize>,
    }

    impl CountingFilter {
        fn new(name: &str) -> (Self, Arc<AtomicUsize>) {
            let run_count = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    name: name.to_string(),
                    run_count: Arc::clone(&run_count),
                },
                run_count,
            )
        }
    }

    #[async_trait]
    impl RequestFilter for CountingFilter {
        fn name(&self) -> &str {
            &self.name
        }

        async fn run_request(&self, _session: &mut dyn PluginSession, _log: &mut PluginLog) -> PluginRunningResult {
            self.run_count.fetch_add(1, Ordering::SeqCst);
            PluginRunningResult::Nothing
        }
    }

    impl UpstreamResponseFilter for CountingFilter {
        fn name(&self) -> &str {
            &self.name
        }

        fn run_upstream_response_filter(
            &self,
            _session: &mut dyn PluginSession,
            _log: &mut PluginLog,
        ) -> PluginRunningResult {
            self.run_count.fetch_add(1, Ordering::SeqCst);
            PluginRunningResult::GoodNext
        }
    }

    impl UpstreamResponseBodyFilter for CountingFilter {
        fn name(&self) -> &str {
            &self.name
        }

        fn run_upstream_response_body_filter(
            &self,
            body: &Option<bytes::Bytes>,
            end_of_stream: bool,
            _session: &mut dyn PluginSession,
        ) -> Option<Duration> {
            assert_eq!(body.as_deref(), Some(b"chunk".as_slice()));
            assert!(end_of_stream);
            self.run_count.fetch_add(1, Ordering::SeqCst);
            Some(Duration::from_millis(17))
        }
    }

    #[async_trait]
    impl UpstreamResponse for CountingFilter {
        fn name(&self) -> &str {
            &self.name
        }

        async fn run_upstream_response(
            &self,
            _session: &mut dyn PluginSession,
            _log: &mut PluginLog,
        ) -> PluginRunningResult {
            self.run_count.fetch_add(1, Ordering::SeqCst);
            PluginRunningResult::GoodNext
        }
    }

    fn run_on_method(method: &str) -> PluginConditions {
        PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Method,
                core: MatchCore {
                    values: Some(vec![method.to_string()]),
                    ..Default::default()
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
        }
    }

    fn session_with_method(method: &str) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session
            .expect_key_get()
            .with(eq(KeyGet::Method))
            .return_const(Some(method.to_string()));
        session
    }

    #[test]
    fn test_conditional_filter_no_conditions() {
        // When conditions is None, should_evaluate returns false
        let conditions = PluginConditions::default();
        assert!(conditions.is_empty());
    }

    #[test]
    fn test_conditional_filter_with_skip_condition() {
        let conditions = PluginConditions {
            skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Skip".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
            run: None,
        };
        assert!(!conditions.is_empty());
    }

    #[tokio::test]
    async fn test_conditional_filter_runs_when_no_conditions() {
        let (inner, run_count) = CountingFilter::new("test-filter");
        let filter = ConditionalRequestFilter::new(Box::new(inner), None);

        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new("test");

        let result = filter.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::Nothing);
        assert!(!log.is_cond_skipped());
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_conditional_filter_runs_when_conditions_satisfied() {
        let (inner, run_count) = CountingFilter::new("test-filter");

        // Run condition: method must be POST
        let conditions = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Method,
                core: MatchCore {
                    values: Some(vec!["POST".to_string()]),
                    ..Default::default()
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
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
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_conditional_filter_skips_when_run_condition_not_met() {
        let (inner, run_count) = CountingFilter::new("test-filter");

        // Run condition: method must be POST
        let conditions = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Method,
                core: MatchCore {
                    values: Some(vec!["POST".to_string()]),
                    ..Default::default()
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
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
        assert_eq!(run_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_conditional_filter_skips_when_skip_condition_met() {
        let (inner, run_count) = CountingFilter::new("test-filter");

        // Skip if header X-Internal exists
        let conditions = PluginConditions {
            skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
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
        assert_eq!(run_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_conditional_filter_runs_when_skip_condition_not_met() {
        let (inner, run_count) = CountingFilter::new("test-filter");

        // Skip if header X-Internal exists
        let conditions = PluginConditions {
            skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
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
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_condition_log_recorded_on_skip() {
        let (inner, run_count) = CountingFilter::new("test-filter");

        // Skip if header X-Internal exists
        let conditions = PluginConditions {
            skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
                key: KeyGet::Header {
                    name: "X-Internal".to_string(),
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
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
        assert_eq!(log.cond_skip.as_deref(), Some("skip:skip.allOf"));
        assert_eq!(run_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_condition_log_recorded_on_run_not_met() {
        let (inner, run_count) = CountingFilter::new("test-filter");

        // Run condition: method must be POST
        let conditions = PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Method,
                core: MatchCore {
                    values: Some(vec!["POST".to_string()]),
                    ..Default::default()
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
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
        assert_eq!(log.cond_skip.as_deref(), Some("!run:run.allOf[0]:keyMatch:method"));
        assert_eq!(run_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn upstream_response_filter_runs_when_condition_matches() {
        let (inner, run_count) = CountingFilter::new("test-response-filter");
        let filter = ConditionalUpstreamResponseFilter::new(Box::new(inner), Some(run_on_method("POST")));
        let mut session = session_with_method("POST");
        let mut log = PluginLog::new("test");

        let result = filter.run_upstream_response_filter(&mut session, &mut log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
        assert!(!log.is_cond_skipped());
    }

    #[test]
    fn upstream_response_filter_skips_without_calling_inner() {
        let (inner, run_count) = CountingFilter::new("test-response-filter");
        let filter = ConditionalUpstreamResponseFilter::new(Box::new(inner), Some(run_on_method("POST")));
        let mut session = session_with_method("GET");
        let mut log = PluginLog::new("test");

        let result = filter.run_upstream_response_filter(&mut session, &mut log);

        assert_eq!(result, PluginRunningResult::Nothing);
        assert_eq!(run_count.load(Ordering::SeqCst), 0);
        assert!(log.is_cond_skipped());
    }

    #[test]
    fn upstream_response_body_filter_runs_and_returns_inner_duration() {
        let duration = Duration::from_millis(17);
        let (inner, run_count) = CountingFilter::new("test-response-body-filter");
        let filter = ConditionalUpstreamResponseBodyFilter::new(Box::new(inner), Some(run_on_method("POST")));
        let mut session = session_with_method("POST");
        let body = Some(bytes::Bytes::from_static(b"chunk"));

        let result = filter.run_upstream_response_body_filter(&body, true, &mut session);

        assert_eq!(result, Some(duration));
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn upstream_response_body_filter_skips_without_calling_inner() {
        let (inner, run_count) = CountingFilter::new("test-response-body-filter");
        let filter = ConditionalUpstreamResponseBodyFilter::new(Box::new(inner), Some(run_on_method("POST")));
        let mut session = session_with_method("GET");
        let body = Some(bytes::Bytes::from_static(b"chunk"));

        let result = filter.run_upstream_response_body_filter(&body, true, &mut session);

        assert_eq!(result, None);
        assert_eq!(run_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn upstream_response_runs_when_condition_matches() {
        let (inner, run_count) = CountingFilter::new("test-response");
        let filter = ConditionalUpstreamResponse::new(Box::new(inner), Some(run_on_method("POST")));
        let mut session = session_with_method("POST");
        let mut log = PluginLog::new("test");

        let result = filter.run_upstream_response(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(run_count.load(Ordering::SeqCst), 1);
        assert!(!log.is_cond_skipped());
    }

    #[tokio::test]
    async fn upstream_response_skips_without_calling_inner() {
        let (inner, run_count) = CountingFilter::new("test-response");
        let filter = ConditionalUpstreamResponse::new(Box::new(inner), Some(run_on_method("POST")));
        let mut session = session_with_method("GET");
        let mut log = PluginLog::new("test");

        let result = filter.run_upstream_response(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::Nothing);
        assert_eq!(run_count.load(Ordering::SeqCst), 0);
        assert!(log.is_cond_skipped());
    }

    #[test]
    fn test_condition_type_and_detail() {
        // Test keyExist
        let c1 = Condition::KeyExist(KeyExistCondition {
            key: KeyGet::Header {
                name: "X-Test".to_string(),
            },
            negate: false,
            descriptor: Box::from(""),
        });
        assert_eq!(c1.cond_type(), "keyExist");

        // Test keyMatch
        let c2 = Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Method,
            core: MatchCore {
                values: Some(vec!["GET".to_string(), "POST".to_string()]),
                ..Default::default()
            },
            negate: false,
            descriptor: Box::from(""),
        });
        assert_eq!(c2.cond_type(), "keyMatch");

        // Test probability
        let c3 = Condition::Probability(crate::plugins::runtime::conditions::ProbabilityCondition {
            ratio: 0.1,
            key: None,
            negate: false,
            descriptor: Box::from(""),
        });
        assert_eq!(c3.cond_type(), "probability");
    }

    #[test]
    fn conditional_request_filter_returns_alias_when_set() {
        let (inner, _) = CountingFilter::new("KeyAuth");
        let with_alias = ConditionalRequestFilter::with_alias_and_dye(
            Box::new(inner),
            None,
            Some("auth-internal".to_string()),
            None,
        );
        assert_eq!(with_alias.alias(), Some("auth-internal"));
        assert_eq!(with_alias.name(), "KeyAuth");
        assert!(with_alias.dye_plan().is_none());

        let (inner2, _) = CountingFilter::new("KeyAuth");
        let no_alias = ConditionalRequestFilter::new(Box::new(inner2), None);
        assert_eq!(no_alias.alias(), None);
    }

    #[test]
    fn conditional_request_filter_carries_dye_plan() {
        use crate::plugins::runtime::dye_plan::CompiledDyePlan;
        use edgion_resources::resources::edgion_plugins::DyeSuite;

        let suite: DyeSuite = serde_yaml::from_str("request:\n  - name: X-T\n").unwrap();
        let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
        let (inner, _) = CountingFilter::new("Canary");
        let filter = ConditionalRequestFilter::with_alias_and_dye(Box::new(inner), None, None, Some(plan.clone()));
        assert!(filter.dye_plan().is_some());
        assert_eq!(filter.dye_plan().unwrap().type_prefix, "canary");
    }
}
