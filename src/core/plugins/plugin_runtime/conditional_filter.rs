//! Conditional filter wrappers for condition-based plugin execution
//!
//! This module provides wrapper types that add condition evaluation to any filter.
//! Conditions are evaluated before the inner filter runs, allowing plugins to be
//! skipped based on runtime context (headers, path, time, probability, etc.)

use async_trait::async_trait;

use super::log::PluginLog;
use super::traits::{PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseFilter};
use crate::core::plugins::plugins_cond::{EvaluationResult, PluginConditions};
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
                let eval = conditions.evaluate_detail(session);
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
        // Check conditions before running
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = conditions.evaluate_detail(session);
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
                let eval = conditions.evaluate_detail(session);
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
    use crate::core::plugins::plugins_cond::{Condition, IncludeCondition, KeyExistCondition};
    use crate::types::common::KeyGet;
    use std::collections::HashMap;
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

    // ==================== Mock Plugin Session ====================

    struct MockPluginSession {
        headers: HashMap<String, String>,
        query_params: HashMap<String, String>,
        cookies: HashMap<String, String>,
        path: String,
        method: String,
        client_ip: String,
        ctx_vars: HashMap<String, String>,
    }

    impl MockPluginSession {
        fn new() -> Self {
            Self {
                headers: HashMap::new(),
                query_params: HashMap::new(),
                cookies: HashMap::new(),
                path: "/".to_string(),
                method: "GET".to_string(),
                client_ip: "127.0.0.1".to_string(),
                ctx_vars: HashMap::new(),
            }
        }

        fn with_header(mut self, name: &str, value: &str) -> Self {
            self.headers.insert(name.to_string(), value.to_string());
            self
        }

        fn with_path(mut self, path: &str) -> Self {
            self.path = path.to_string();
            self
        }

        fn with_method(mut self, method: &str) -> Self {
            self.method = method.to_string();
            self
        }
    }

    // Implement PluginSession for MockPluginSession
    // Only implement the methods needed for condition evaluation
    #[async_trait]
    impl PluginSession for MockPluginSession {
        fn header_value(&self, name: &str) -> Option<String> {
            self.headers.get(name).cloned()
        }

        fn request_headers(&self) -> Vec<(String, String)> {
            self.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        }

        fn method(&self) -> String {
            self.method.clone()
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

        fn get_query(&self) -> Option<String> {
            None // Not used in condition evaluation tests
        }

        fn get_method(&self) -> &str {
            &self.method
        }

        fn get_ctx_var(&self, key: &str) -> Option<String> {
            self.ctx_vars.get(key).cloned()
        }

        fn set_ctx_var(&mut self, key: &str, value: &str) -> super::super::traits::PluginSessionResult<()> {
            self.ctx_vars.insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn remove_ctx_var(&mut self, key: &str) -> super::super::traits::PluginSessionResult<()> {
            self.ctx_vars.remove(key);
            Ok(())
        }

        fn get_path_param(&mut self, _name: &str) -> Option<String> {
            None // Not used in condition evaluation tests
        }

        // Stub implementations for other methods (not used in tests)
        async fn write_response_header(
            &mut self,
            _resp: Box<pingora_http::ResponseHeader>,
            _end_of_stream: bool,
        ) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn write_response_header_boxed<'a>(
            &'a mut self,
            _resp: Box<pingora_http::ResponseHeader>,
            _end_of_stream: bool,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = super::super::traits::PluginSessionResult<()>> + Send + 'a>,
        > {
            Box::pin(async { Ok(()) })
        }

        fn set_response_header(&mut self, _name: &str, _value: &str) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn append_response_header(
            &mut self,
            _name: &str,
            _value: &str,
        ) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn remove_response_header(&mut self, _name: &str) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn get_response_header(&self, _name: &str) -> Option<String> {
            None
        }

        fn set_response_status(&mut self, _status: u16) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn set_request_header(&mut self, _name: &str, _value: &str) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn append_request_header(
            &mut self,
            _name: &str,
            _value: &str,
        ) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn remove_request_header(&mut self, _name: &str) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn set_upstream_uri(&mut self, _uri: &str) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn set_upstream_host(&mut self, _host: &str) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn set_upstream_method(&mut self, _method: &str) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        async fn write_response_body(
            &mut self,
            _body: Option<bytes::Bytes>,
            _end_of_stream: bool,
        ) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        async fn shutdown(&mut self) {}

        fn client_addr(&self) -> &str {
            &self.client_ip
        }

        fn remote_addr(&self) -> &str {
            &self.client_ip
        }

        fn set_remote_addr(&mut self, _addr: &str) -> super::super::traits::PluginSessionResult<()> {
            Ok(())
        }

        fn ctx(&self) -> &crate::types::EdgionHttpContext {
            unimplemented!("Not needed for condition tests")
        }

        fn push_plugin_ref(&mut self, _key: String) {}
        fn pop_plugin_ref(&mut self) {}
        fn plugin_ref_depth(&self) -> usize {
            0
        }
        fn has_plugin_ref(&self, _key: &str) -> bool {
            false
        }
        fn push_edgion_plugins_log(&mut self, _log: crate::core::plugins::plugin_runtime::log::EdgionPluginsLog) {}
        fn start_edgion_plugins_log(
            &mut self,
            _name: String,
        ) -> crate::core::plugins::plugin_runtime::log::EdgionPluginsLogToken {
            crate::core::plugins::plugin_runtime::log::EdgionPluginsLogToken::new(0, 0)
        }
        fn push_to_edgion_plugins_log(
            &mut self,
            _token: &crate::core::plugins::plugin_runtime::log::EdgionPluginsLogToken,
            _log: crate::core::plugins::plugin_runtime::log::PluginLog,
        ) {
        }

        fn key_get(&self, key: &crate::types::common::KeyGet) -> Option<String> {
            use crate::types::common::KeyGet;
            match key {
                KeyGet::ClientIp => {
                    if self.client_ip.is_empty() {
                        None
                    } else {
                        Some(self.client_ip.clone())
                    }
                }
                KeyGet::Header { name } => self.headers.get(name).cloned(),
                KeyGet::Cookie { name } => self.cookies.get(name).cloned(),
                KeyGet::Query { name } => self.query_params.get(name).cloned(),
                KeyGet::Path => Some(self.path.clone()),
                KeyGet::Method => Some(self.method.clone()),
                KeyGet::Ctx { name } => self.ctx_vars.get(name).cloned(),
                KeyGet::ClientIpAndPath => {
                    if self.client_ip.is_empty() {
                        None
                    } else {
                        Some(format!("{}:{}", self.client_ip, self.path))
                    }
                }
            }
        }

        fn key_set(
            &mut self,
            _key: &crate::types::common::KeySet,
            _value: Option<String>,
        ) -> super::super::traits::PluginSessionResult<()> {
            Ok(()) // Stub for tests
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

        let mut session = MockPluginSession::new().with_method("POST");
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
        let mut session = MockPluginSession::new().with_method("GET");
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
        let mut session = MockPluginSession::new().with_header("X-Internal", "true");
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
        let mut log = PluginLog::new("test");

        let result = filter.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::Nothing);
        assert!(!log.is_cond_skipped()); // Should run
    }

    #[test]
    fn test_plugin_session_key_get() {
        use crate::types::KeyGet;

        let session = MockPluginSession::new()
            .with_header("X-Test", "value")
            .with_path("/api/users")
            .with_method("POST");

        // PluginSession has key_get() method
        assert_eq!(
            session.key_get(&KeyGet::Header {
                name: "X-Test".to_string()
            }),
            Some("value".to_string())
        );
        assert_eq!(
            session.key_get(&KeyGet::Header {
                name: "X-Missing".to_string()
            }),
            None
        );
        assert_eq!(session.key_get(&KeyGet::Path), Some("/api/users".to_string()));
        assert_eq!(session.key_get(&KeyGet::Method), Some("POST".to_string()));
        assert_eq!(session.key_get(&KeyGet::ClientIp), Some("127.0.0.1".to_string()));
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
        let mut session = MockPluginSession::new().with_header("X-Internal", "true");
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
        let mut session = MockPluginSession::new().with_method("GET");
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
            Condition::Probability(crate::core::plugins::plugins_cond::ProbabilityCondition { ratio: 0.1, key: None });
        assert_eq!(c3.cond_type(), "prob");
        assert_eq!(c3.cond_detail(), "10%");
    }
}
