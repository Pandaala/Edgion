use super::*;
use crate::plugins::runtime::conditions::{Condition, KeyExistCondition, KeyMatchCondition};
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_condition_input_mocks;
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
    setup_condition_input_mocks(&mut session, &[(KeyGet::Method, method)]);
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
    setup_condition_input_mocks(&mut session, &[(KeyGet::Method, "POST")]);
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
    setup_condition_input_mocks(&mut session, &[(KeyGet::Method, "GET")]);
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
    setup_condition_input_mocks(
        &mut session,
        &[(
            KeyGet::Header {
                name: "X-Internal".to_string(),
            },
            "true",
        )],
    );

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
    setup_condition_input_mocks(&mut session, &[]);

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
    setup_condition_input_mocks(
        &mut session,
        &[(
            KeyGet::Header {
                name: "X-Internal".to_string(),
            },
            "true",
        )],
    );
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
    setup_condition_input_mocks(&mut session, &[(KeyGet::Method, "GET")]);
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
    let with_alias =
        ConditionalRequestFilter::with_alias_and_dye(Box::new(inner), None, Some("auth-internal".to_string()), None);
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

    let suite: DyeSuite = yaml_serde::from_str("request:\n  - name: X-T\n").unwrap();
    let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
    let (inner, _) = CountingFilter::new("Canary");
    let filter = ConditionalRequestFilter::with_alias_and_dye(Box::new(inner), None, None, Some(plan.clone()));
    assert!(filter.dye_plan().is_some());
    assert_eq!(filter.dye_plan().unwrap().type_prefix, "canary");
}

#[tokio::test]
async fn test_conditional_filter_empty_conditions_forward_lazily_to_inner() {
    let (inner, run_count) = CountingFilter::new("test-filter");
    // A present container with neither `skip` nor `run` takes the direct-forward path.
    let filter = ConditionalRequestFilter::new(Box::new(inner), Some(PluginConditions::default()));

    // No mock expectations: any condition-input read would panic.
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("test");

    let fut = filter.run_request(&mut session, &mut log);
    assert_eq!(
        run_count.load(Ordering::SeqCst),
        0,
        "inner must not run before first poll"
    );
    let result = fut.await;

    assert_eq!(result, PluginRunningResult::Nothing);
    assert!(!log.is_cond_skipped());
    assert_eq!(run_count.load(Ordering::SeqCst), 1);
}
