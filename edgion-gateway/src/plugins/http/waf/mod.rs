//! Native WAF adapter spanning the complete supported HTTP transaction.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{BodyRequirement, WafConfig, WafMode};
use edgion_waf::seclang::{
    BodyError, BodyProcessorSelection, PhaseOutcome, RequestBody, RequestBodyExecutionSummary, RequestHead,
    RequestHeader, ResponseHead, ResponseHeader, ResponseWindow, RuleEngineMode, RuntimeError, RuntimeLimits,
    Transaction,
};

use crate::ctx::{WafErrorCode, WafLogPhase, WafPhase, WafPhaseClaim};
use crate::plugins::runtime::body_gate::body_read_failure_result;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseFilter};

const NAME: &str = "Waf";

struct WafGenerationMetricLease {
    bytes: i64,
}

impl WafGenerationMetricLease {
    fn new(bytes: usize) -> Self {
        let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
        crate::observe::metrics::adjust_waf_generation_bytes(bytes);
        Self { bytes }
    }
}

impl Drop for WafGenerationMetricLease {
    fn drop(&mut self) {
        crate::observe::metrics::adjust_waf_generation_bytes(-self.bytes);
    }
}

#[inline]
fn elapsed_micros(started: Instant) -> u64 {
    u64::try_from(Instant::now().saturating_duration_since(started).as_micros()).unwrap_or(u64::MAX)
}

struct SharedWaf {
    generation: Arc<edgion_waf::seclang::CompiledGeneration>,
    owner: Arc<()>,
    engine_mode: RuleEngineMode,
    requires_request_body: bool,
    request_body_execution: RequestBodyExecutionSummary,
    priority: u16,
    _generation_metric: WafGenerationMetricLease,
}

/// One compiled logical WAF. Clones share the immutable generation and owner
/// token while implementing the three Gateway phase hooks independently.
#[derive(Clone)]
pub struct WafPlugin {
    shared: Arc<SharedWaf>,
}

impl WafPlugin {
    pub fn try_new(config: &WafConfig) -> Result<Self, String> {
        let started = Instant::now();
        let generation = match config.compile_generation() {
            Ok(generation) => {
                crate::observe::metrics::record_waf_compile("ok", elapsed_micros(started));
                generation
            }
            Err(error) => {
                crate::observe::metrics::record_waf_compile("error", elapsed_micros(started));
                return Err(error);
            }
        };
        let engine_mode = match config.mode {
            WafMode::On => RuleEngineMode::On,
            WafMode::DetectionOnly => RuleEngineMode::DetectionOnly,
        };
        let request_body_execution = generation.request_body_execution_summary();
        let requires_request_body = request_body_execution.requires_request_body();
        let generation_bytes = generation.retained_bytes();
        Ok(Self {
            shared: Arc::new(SharedWaf {
                generation: Arc::new(generation),
                owner: Arc::new(()),
                engine_mode,
                requires_request_body,
                request_body_execution,
                priority: config.priority,
                _generation_metric: WafGenerationMetricLease::new(generation_bytes),
            }),
        })
    }

    pub(crate) fn owner(&self) -> Arc<()> {
        Arc::clone(&self.shared.owner)
    }

    pub(crate) fn priority(&self) -> u16 {
        self.shared.priority
    }

    pub(crate) fn runtime_error_status(error: &RuntimeError) -> u16 {
        match error {
            RuntimeError::Body {
                kind: BodyError::ByteLimit { .. },
            } => 413,
            _ => 500,
        }
    }

    pub(crate) fn runtime_error_code(error: &RuntimeError) -> WafErrorCode {
        match error {
            RuntimeError::InputByteLimit { .. }
            | RuntimeError::HeaderLimit { .. }
            | RuntimeError::HeaderByteLimit { .. }
            | RuntimeError::ValueByteLimit { .. }
            | RuntimeError::QueryArgumentLimit { .. }
            | RuntimeError::CookieLimit { .. } => WafErrorCode::InputLimit,
            RuntimeError::Body {
                kind: BodyError::ByteLimit { .. },
            } => WafErrorCode::BodyLimit,
            RuntimeError::Body { .. } => WafErrorCode::BodyProcessing,
            RuntimeError::CollectionEntryLimit { .. }
            | RuntimeError::RetainedByteLimit { .. }
            | RuntimeError::MatcherScratchByteLimit { .. }
            | RuntimeError::SelectedValueLimit { .. }
            | RuntimeError::OperatorEvaluationLimit { .. }
            | RuntimeError::OperatorWorkLimit { .. }
            | RuntimeError::TxVariableLimit { .. }
            | RuntimeError::TxByteLimit { .. }
            | RuntimeError::SetVarActionLimit { .. }
            | RuntimeError::CaptureEntryLimit { .. }
            | RuntimeError::CaptureByteLimit { .. } => WafErrorCode::RuntimeLimit,
            RuntimeError::ResponseHeaderLimit { .. }
            | RuntimeError::ResponseHeaderByteLimit { .. }
            | RuntimeError::InvalidResponseStatus { .. }
            | RuntimeError::ResponseWindowByteLimit { .. } => WafErrorCode::ResponseLimit,
            RuntimeError::RequestBodyAlreadyPublished
            | RuntimeError::RequestBodyNotPublished
            | RuntimeError::ResponseAlreadyPublished
            | RuntimeError::ResponseNotPublished
            | RuntimeError::PhaseOrder { .. } => WafErrorCode::PhaseState,
            RuntimeError::XmlSelectorUnavailable
            | RuntimeError::RuntimeActionUnavailable { .. }
            | RuntimeError::RuntimeMacroUnavailable { .. } => WafErrorCode::Unsupported,
            RuntimeError::GenerationInvariant
            | RuntimeError::InvalidHeaderName { .. }
            | RuntimeError::InvalidHeaderValue { .. }
            | RuntimeError::InvalidResponseHeaderName { .. }
            | RuntimeError::InvalidResponseHeaderValue { .. } => WafErrorCode::RuntimeInvariant,
            RuntimeError::Matcher { .. } => WafErrorCode::Matcher,
            RuntimeError::Transformation { .. } => WafErrorCode::Transformation,
            RuntimeError::Allocation => WafErrorCode::Allocation,
            RuntimeError::IntegerConversion | RuntimeError::Arithmetic => WafErrorCode::Arithmetic,
        }
    }

    fn runtime_failure(
        &self,
        session: &mut dyn PluginSession,
        phase: WafLogPhase,
        error: &RuntimeError,
    ) -> PluginRunningResult {
        let status = Self::runtime_error_status(error);
        session.record_waf_terminal_error(&self.shared.owner, phase, Self::runtime_error_code(error), status);
        PluginRunningResult::ErrResponse { status, body: None }
    }

    fn record_outcome(
        &self,
        session: &mut dyn PluginSession,
        phase: WafLogPhase,
        execution_us: u64,
        outcome: &PhaseOutcome,
    ) -> Option<(u64, u16)> {
        session.record_waf_phase_outcome(&self.shared.owner, phase, execution_us, outcome);
        outcome
            .intervention()
            .map(|intervention| (intervention.rule_id(), intervention.status()))
    }

    fn intervention_result(intervention: Option<(u64, u16)>) -> PluginRunningResult {
        match intervention {
            Some((_, status)) => PluginRunningResult::ErrResponse { status, body: None },
            None => PluginRunningResult::GoodNext,
        }
    }

    fn restore_transaction(
        &self,
        session: &mut dyn PluginSession,
        transaction: Transaction<'static>,
        phase: WafLogPhase,
    ) -> PluginRunningResult {
        if session.install_waf_transaction(Arc::clone(&self.shared.owner), transaction) {
            PluginRunningResult::GoodNext
        } else {
            session.record_waf_terminal_error(&self.shared.owner, phase, WafErrorCode::TransactionOwnerConflict, 500);
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            }
        }
    }

    fn claim_phase(
        &self,
        session: &mut dyn PluginSession,
        phase: WafPhase,
        log_phase: WafLogPhase,
    ) -> Option<PluginRunningResult> {
        match session.claim_waf_phase(&self.shared.owner, phase) {
            WafPhaseClaim::Execute => None,
            WafPhaseClaim::Skip => Some(PluginRunningResult::GoodNext),
            WafPhaseClaim::Pending => {
                session.record_waf_terminal_error(&self.shared.owner, log_phase, WafErrorCode::WinnerNotSelected, 500);
                Some(PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                })
            }
        }
    }
}

#[async_trait]
impl RequestFilter for WafPlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn suppress_automatic_log(&self) -> bool {
        true
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if let Some(result) = self.claim_phase(session, WafPhase::Request, WafLogPhase::RequestHeaders) {
            return result;
        }
        let raw_headers = session.request_headers_bytes();
        let headers: Vec<_> = raw_headers
            .iter()
            .map(|(name, value)| RequestHeader::new(name, value))
            .collect();
        let uri = session.get_uri();
        let raw_uri = session.get_request_uri();
        let protocol = session.get_http_protocol();
        let request_id = session.get_request_id();
        let method = session.get_method();
        let remote_addr = session.remote_ip();
        let request_line = format!("{method} {raw_uri} {protocol}");
        let request = RequestHead::new(
            request_line.as_bytes(),
            method.as_bytes(),
            uri.as_bytes(),
            raw_uri.as_bytes(),
            protocol.as_bytes(),
            remote_addr.as_bytes(),
            request_id.as_bytes(),
            &headers,
        );
        let mut transaction = match Transaction::new_shared_with_modes(
            Arc::clone(&self.shared.generation),
            request,
            RuntimeLimits::default(),
            edgion_waf::seclang::EvaluationMode::Accelerated,
            self.shared.engine_mode,
        ) {
            Ok(transaction) => transaction,
            Err(error) => return self.runtime_failure(session, WafLogPhase::RequestHeaders, &error),
        };

        let started = Instant::now();
        let execution = transaction.execute_request_headers();
        let request_headers_us = elapsed_micros(started);
        crate::observe::metrics::record_waf_execution_micros("request_headers", request_headers_us);
        let phase_one = match execution {
            Ok(outcome) => self.record_outcome(session, WafLogPhase::RequestHeaders, request_headers_us, &outcome),
            Err(error) => return self.runtime_failure(session, WafLogPhase::RequestHeaders, &error),
        };
        if phase_one.is_some() {
            return Self::intervention_result(phase_one);
        }

        let raw_stream_only = self.shared.requires_request_body
            && self.shared.request_body_execution.streaming_raw_rule_units() != 0
            && self.shared.request_body_execution.streaming_structured_rule_units() == 0
            && self.shared.request_body_execution.bounded_snapshot_rule_units() == 0
            && self.shared.request_body_execution.unsupported_rule_units() == 0;
        if raw_stream_only {
            if let Err(error) = transaction.start_request_body_stream() {
                return self.runtime_failure(session, WafLogPhase::RequestBody, &error);
            }
            let restored = self.restore_transaction(session, transaction, WafLogPhase::RequestBody);
            if !matches!(restored, PluginRunningResult::GoodNext) {
                return restored;
            }
            let body_limit = session.ctx().request_body_config.max_body_size_bytes;
            if !session.enable_waf_request_body_streaming(&self.shared.owner, body_limit) {
                session.record_waf_terminal_error(
                    &self.shared.owner,
                    WafLogPhase::RequestBody,
                    WafErrorCode::BodyStreamInstallFailed,
                    500,
                );
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                };
            }
            return PluginRunningResult::GoodNext;
        }

        let body = if self.shared.requires_request_body {
            let cap = session.ctx().request_body_config.max_body_size_bytes;
            match session.read_request_body(Some(cap)).await {
                Ok(body) => body,
                Err(error) => {
                    let result = body_read_failure_result(session, &BodyRequirement::fail_close(), log, &error);
                    let status = match &result {
                        PluginRunningResult::ErrResponse { status, .. } => *status,
                        _ => 500,
                    };
                    session.record_waf_terminal_error(
                        &self.shared.owner,
                        WafLogPhase::RequestBody,
                        WafErrorCode::RequestBodyReadFailed,
                        status,
                    );
                    return result;
                }
            }
        } else {
            bytes::Bytes::new()
        };
        if let Err(error) = transaction.publish_request_body(RequestBody::new(&body, BodyProcessorSelection::Auto)) {
            return self.runtime_failure(session, WafLogPhase::RequestBody, &error);
        }
        let started = Instant::now();
        let execution = transaction.execute_request_body();
        let request_body_us = elapsed_micros(started);
        crate::observe::metrics::record_waf_execution_micros("request_body", request_body_us);
        let phase_two = match execution {
            Ok(outcome) => self.record_outcome(session, WafLogPhase::RequestBody, request_body_us, &outcome),
            Err(error) => return self.runtime_failure(session, WafLogPhase::RequestBody, &error),
        };
        if phase_two.is_some() {
            return Self::intervention_result(phase_two);
        }

        self.restore_transaction(session, transaction, WafLogPhase::RequestBody)
    }
}

impl UpstreamResponseFilter for WafPlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn suppress_automatic_log(&self) -> bool {
        true
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        _log: &mut PluginLog,
    ) -> PluginRunningResult {
        if let Some(result) = self.claim_phase(session, WafPhase::ResponseHeaders, WafLogPhase::ResponseHeaders) {
            return result;
        }
        let Some(mut transaction) = session.take_waf_transaction(&self.shared.owner) else {
            session.record_waf_terminal_error(
                &self.shared.owner,
                WafLogPhase::ResponseHeaders,
                WafErrorCode::TransactionMissing,
                500,
            );
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        };
        let raw_headers = session.response_headers_bytes();
        let headers: Vec<_> = raw_headers
            .iter()
            .map(|(name, value)| ResponseHeader::new(name, value))
            .collect();
        let status = session.get_response_status().unwrap_or(500);
        if let Err(error) = transaction.publish_response_head(ResponseHead::new(status, &headers)) {
            return self.runtime_failure(session, WafLogPhase::ResponseHeaders, &error);
        }
        let started = Instant::now();
        let execution = transaction.execute_response_headers();
        let response_headers_us = elapsed_micros(started);
        crate::observe::metrics::record_waf_execution_micros("response_headers", response_headers_us);
        let intervention = match execution {
            Ok(outcome) => self.record_outcome(session, WafLogPhase::ResponseHeaders, response_headers_us, &outcome),
            Err(error) => return self.runtime_failure(session, WafLogPhase::ResponseHeaders, &error),
        };
        if intervention.is_some() {
            return Self::intervention_result(intervention);
        }
        self.restore_transaction(session, transaction, WafLogPhase::ResponseHeaders)
    }
}

#[async_trait]
impl UpstreamResponse for WafPlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn suppress_automatic_log(&self) -> bool {
        true
    }

    async fn run_upstream_response(
        &self,
        session: &mut dyn PluginSession,
        _log: &mut PluginLog,
    ) -> PluginRunningResult {
        if let Some(result) = self.claim_phase(session, WafPhase::ResponseComplete, WafLogPhase::ResponseBody) {
            return result;
        }
        let Some(mut transaction) = session.take_waf_transaction(&self.shared.owner) else {
            session.record_waf_terminal_error(
                &self.shared.owner,
                WafLogPhase::ResponseBody,
                WafErrorCode::TransactionMissing,
                500,
            );
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        };

        let started = Instant::now();
        let execution = transaction.execute_response_window(ResponseWindow::new(&[], true));
        let response_body_us = elapsed_micros(started);
        crate::observe::metrics::record_waf_execution_micros("response_body", response_body_us);
        let phase_four = match execution {
            Ok(outcome) => self.record_outcome(session, WafLogPhase::ResponseBody, response_body_us, &outcome),
            Err(error) => return self.runtime_failure(session, WafLogPhase::ResponseBody, &error),
        };
        let started = Instant::now();
        let execution = transaction.execute_logging();
        let logging_us = elapsed_micros(started);
        crate::observe::metrics::record_waf_execution_micros("logging", logging_us);
        let phase_five = match execution {
            Ok(outcome) => self.record_outcome(session, WafLogPhase::Logging, logging_us, &outcome),
            Err(error) => return self.runtime_failure(session, WafLogPhase::Logging, &error),
        };
        Self::intervention_result(phase_four.or(phase_five))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use edgion_resources::resources::edgion_plugins::WafMode;

    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::EdgionHttpContext;

    fn plugin(rules: &str, mode: WafMode) -> WafPlugin {
        WafPlugin::try_new(&WafConfig {
            rules: rules.to_string(),
            mode,
            priority: 0,
        })
        .expect("test WAF compiles")
    }

    #[test]
    fn dedicated_summary_suppresses_all_generic_stage_logs() {
        let plugin = plugin("SecAction \"id:1,phase:1,pass\"\n", WafMode::On);
        assert!(RequestFilter::suppress_automatic_log(&plugin));
        assert!(UpstreamResponseFilter::suppress_automatic_log(&plugin));
        assert!(UpstreamResponse::suppress_automatic_log(&plugin));
    }

    #[test]
    fn blank_rules_are_rejected_by_the_gateway_constructor() {
        let config = WafConfig {
            rules: " \n\t".to_string(),
            mode: WafMode::On,
            priority: 0,
        };

        assert_eq!(
            WafPlugin::try_new(&config).err().as_deref(),
            Some("rules must not be empty")
        );
    }

    fn expect_request_head(session: &mut MockPluginSession, uri: &str) {
        expect_request_head_with_headers(session, uri, Vec::new());
    }

    fn expect_request_head_with_headers(session: &mut MockPluginSession, uri: &str, headers: Vec<(Vec<u8>, Vec<u8>)>) {
        session
            .expect_claim_waf_phase()
            .returning(|_, _| WafPhaseClaim::Execute);
        session.expect_record_waf_phase_outcome().returning(|_, _, _, _| ());
        session.expect_record_waf_terminal_error().returning(|_, _, _, _| ());
        session.expect_request_headers_bytes().return_once(move || headers);
        session.expect_get_method().times(1).return_const("POST".to_string());
        session.expect_get_uri().return_const(uri.to_string());
        session.expect_get_request_uri().return_const(uri.to_string());
        session.expect_get_http_protocol().return_const("HTTP/1.1".to_string());
        session.expect_remote_ip().return_const("192.0.2.1".to_string());
        session.expect_get_request_id().return_const("request-1".to_string());
    }

    #[tokio::test]
    async fn phase_one_denial_happens_before_request_body_read() {
        let plugin = plugin(
            "SecRule REQUEST_URI \"@contains /blocked\" \"id:1,phase:1,deny,status:403\"\n",
            WafMode::On,
        );
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/blocked");
        let mut log = PluginLog::new(NAME);

        let result = RequestFilter::run_request(&plugin, &mut session, &mut log).await;

        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: None
            }
        );
        assert!(!log.contains("blocked rule"));
    }

    #[tokio::test]
    async fn header_only_generation_does_not_capture_an_unused_body() {
        let plugin = plugin(
            "SecRule REQUEST_URI \"@contains /allowed\" \"id:1,phase:1,pass\"\n",
            WafMode::On,
        );
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/allowed");
        session.expect_install_waf_transaction().return_once(|_, transaction| {
            drop(transaction);
            true
        });
        let mut log = PluginLog::new(NAME);

        assert_eq!(
            RequestFilter::run_request(&plugin, &mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn raw_phrase_generation_installs_streaming_without_body_storage() {
        let plugin = plugin(
            "SecRule REQUEST_BODY \"@pm blocked\" \"id:20,phase:2,deny,status:409\"\n",
            WafMode::On,
        );
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/stream");
        session.expect_install_waf_transaction().return_once(|_, transaction| {
            assert_eq!(transaction.next_phase(), Some(edgion_waf::seclang::Phase::RequestBody));
            drop(transaction);
            true
        });
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = 8192;
        session.expect_ctx().return_const(ctx);
        session
            .expect_enable_waf_request_body_streaming()
            .withf(|_, limit| *limit == 8192)
            .return_once(|_, _| true);
        let mut log = PluginLog::new(NAME);

        assert_eq!(
            RequestFilter::run_request(&plugin, &mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn duplicate_request_headers_are_preserved_for_phase_one() {
        let plugin = plugin(
            "SecRule REQUEST_HEADERS:X-Test \"@streq blocked\" \"id:3,phase:1,deny,status:409\"\n",
            WafMode::On,
        );
        let mut session = MockPluginSession::new();
        expect_request_head_with_headers(
            &mut session,
            "/headers",
            vec![
                (b"x-test".to_vec(), b"allowed".to_vec()),
                (b"x-test".to_vec(), b"blocked".to_vec()),
            ],
        );
        let mut log = PluginLog::new(NAME);

        assert_eq!(
            RequestFilter::run_request(&plugin, &mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 409,
                body: None
            }
        );
    }

    #[tokio::test]
    async fn malformed_structured_body_fails_closed_without_payload_logging() {
        let plugin = plugin("SecRule ARGS \"@contains value\" \"id:4,phase:2,pass\"\n", WafMode::On);
        let mut session = MockPluginSession::new();
        expect_request_head_with_headers(
            &mut session,
            "/json",
            vec![(b"content-type".to_vec(), b"application/json".to_vec())],
        );
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = 1024;
        session.expect_ctx().return_const(ctx);
        session
            .expect_read_request_body()
            .return_once(|_| Ok(Bytes::from_static(b"private-invalid-json")));
        let mut log = PluginLog::new(NAME);

        assert_eq!(
            RequestFilter::run_request(&plugin, &mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            }
        );
        assert!(!log.contains("private"));
    }

    #[tokio::test]
    async fn gateway_body_cap_failure_maps_to_413() {
        use crate::plugins::runtime::traits::session::BodyReadError;

        let plugin = plugin(
            "SecRule REQUEST_BODY \"@contains blocked\" \"id:5,phase:2,pass\"\n",
            WafMode::On,
        );
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/large");
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = 8;
        session.expect_ctx().return_const(ctx);
        session
            .expect_read_request_body()
            .return_once(|_| Err(BodyReadError::BufferLimitExceeded { limit: 8 }));
        session.expect_record_error().returning(|_| ());
        let mut log = PluginLog::new(NAME);

        assert_eq!(
            RequestFilter::run_request(&plugin, &mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 413,
                body: None
            }
        );
    }

    #[tokio::test]
    async fn detection_only_records_body_match_and_keeps_the_transaction() {
        let plugin = plugin(
            "SecRule REQUEST_BODY \"@contains blocked\" \"id:2,phase:2,deny,status:406\"\n",
            WafMode::DetectionOnly,
        );
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/submit");
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = 1024;
        session.expect_ctx().return_const(ctx);
        session
            .expect_read_request_body()
            .withf(|limit| *limit == Some(1024))
            .return_once(|_| Ok(Bytes::from_static(b"blocked")));
        session
            .expect_install_waf_transaction()
            .times(1)
            .return_once(|_, transaction| {
                drop(transaction);
                true
            });
        let mut log = PluginLog::new(NAME);

        let result = RequestFilter::run_request(&plugin, &mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(!log.contains("matched"));
        assert!(!log.contains("blocked rule"));
    }

    #[tokio::test]
    async fn one_transaction_advances_through_all_gateway_phase_hooks() {
        let plugin = plugin(
            concat!(
                "SecRule REQUEST_BODY \"@contains allowed\" \"id:10,phase:2,pass\"\n",
                "SecRule RESPONSE_STATUS \"@streq 201\" \"id:11,phase:3,pass\"\n",
                "SecAction \"id:12,phase:4,pass\"\n",
                "SecAction \"id:13,phase:5,pass\"\n"
            ),
            WafMode::On,
        );
        let slot = Arc::new(std::sync::Mutex::new(None));
        let mut session = MockPluginSession::new();
        expect_request_head(&mut session, "/submit");
        let mut ctx = EdgionHttpContext::new();
        ctx.request_body_config.max_body_size_bytes = 1024;
        session.expect_ctx().return_const(ctx);
        session
            .expect_read_request_body()
            .return_once(|_| Ok(Bytes::from_static(b"allowed")));
        let install_slot = Arc::clone(&slot);
        session
            .expect_install_waf_transaction()
            .times(2)
            .returning(move |_, transaction| {
                let mut guard = install_slot.lock().expect("transaction slot lock");
                if guard.is_some() {
                    return false;
                }
                *guard = Some(transaction);
                true
            });
        let take_slot = Arc::clone(&slot);
        session
            .expect_take_waf_transaction()
            .times(2)
            .returning(move |_| take_slot.lock().expect("transaction slot lock").take());
        session.expect_response_headers_bytes().return_once(|| Vec::new());
        session.expect_get_response_status().return_const(Some(201_u16));

        let mut request_log = PluginLog::new(NAME);
        assert_eq!(
            RequestFilter::run_request(&plugin, &mut session, &mut request_log).await,
            PluginRunningResult::GoodNext
        );
        let mut header_log = PluginLog::new(NAME);
        assert_eq!(
            UpstreamResponseFilter::run_upstream_response_filter(&plugin, &mut session, &mut header_log),
            PluginRunningResult::GoodNext
        );
        let mut response_log = PluginLog::new(NAME);
        assert_eq!(
            UpstreamResponse::run_upstream_response(&plugin, &mut session, &mut response_log).await,
            PluginRunningResult::GoodNext
        );
        assert!(slot.lock().expect("transaction slot lock").is_none());
    }
}
