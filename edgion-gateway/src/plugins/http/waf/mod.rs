//! Native WAF adapter spanning the complete supported HTTP transaction.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{BodyRequirement, WafConfig, WafMode, WafRequestBodyInspection};
use edgion_waf::seclang::{
    BodyError, BodyProcessorSelection, JsonCheckerError, JsonInspectionLimits, JsonStreamError, JsonStreamLimit,
    PhaseOutcome, RequestBody, RequestBodyPrefixCompletion, RequestHead, RequestHeader, ResponseHead, ResponseHeader,
    ResponseWindow, RuleCheckPolicy, RuleEngineMode, RuntimeError, RuntimeLimits, Transaction, XmlInspectionLimits,
    XmlStreamError, XmlStreamLimit,
};

use crate::ctx::{WafErrorCode, WafLogPhase, WafPhase, WafPhaseClaim};
use crate::plugins::runtime::body_gate::body_read_failure_result;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::session::BodyReadError;
use crate::plugins::runtime::traits::{
    PluginSession, PluginSessionHeaderExt, RequestFilter, UpstreamResponse, UpstreamResponseFilter,
};

mod json_capture;

/// The transport validates authority; WAF exposes its H2 Host-equivalent bytes
/// without changing live headers or losing explicit Host values.
fn request_header_views<'input>(
    headers: &'input http::HeaderMap,
    protocol: &str,
    uri: Option<&'input http::Uri>,
) -> Vec<RequestHeader<'input>> {
    let authority = if protocol == "HTTP/2.0" && !headers.contains_key(http::header::HOST) {
        uri.and_then(|uri| uri.authority())
            .map(|authority| authority.as_str().as_bytes())
    } else {
        None
    };
    let mut views = Vec::with_capacity(headers.len() + usize::from(authority.is_some()));
    views.extend(
        headers
            .iter()
            .map(|(name, value)| RequestHeader::new(name.as_str().as_bytes(), value.as_bytes())),
    );
    if let Some(authority) = authority {
        views.push(RequestHeader::new(b"host", authority));
    }
    views
}

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
    request_body_inspection: WafRequestBodyInspection,
    request_body_prefix_bytes: usize,
    json_limits: JsonInspectionLimits,
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
        let requires_request_body = generation.requires_complete_request_body();
        let generation_bytes = generation.retained_bytes();
        Ok(Self {
            shared: Arc::new(SharedWaf {
                generation: Arc::new(generation),
                owner: Arc::new(()),
                engine_mode,
                requires_request_body,
                request_body_inspection: config.request_body.inspection,
                request_body_prefix_bytes: config.request_body.prefix_size_bytes()?,
                json_limits: config
                    .request_body
                    .json
                    .as_ref()
                    .map(|json| json.inspection_limits())
                    .unwrap_or_default(),
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
            }
            | RuntimeError::JsonChecker {
                kind:
                    JsonCheckerError::Lexical(JsonStreamError::Limit {
                        kind: JsonStreamLimit::SourceBytes,
                        ..
                    }),
            }
            | RuntimeError::XmlStream {
                kind:
                    XmlStreamError::Limit {
                        kind: XmlStreamLimit::SourceBytes,
                        ..
                    },
            } => 413,
            _ => 500,
        }
    }

    pub(crate) fn runtime_error_code(error: &RuntimeError) -> WafErrorCode {
        match error {
            RuntimeError::JsonChecker { kind } => match kind {
                JsonCheckerError::Lexical(JsonStreamError::Limit {
                    kind: JsonStreamLimit::SourceBytes,
                    ..
                }) => WafErrorCode::BodyLimit,
                JsonCheckerError::Limit { .. } | JsonCheckerError::Lexical(JsonStreamError::Limit { .. }) => {
                    WafErrorCode::RuntimeLimit
                }
                JsonCheckerError::Allocation | JsonCheckerError::Lexical(JsonStreamError::Allocation) => {
                    WafErrorCode::Allocation
                }
                JsonCheckerError::Arithmetic | JsonCheckerError::Lexical(JsonStreamError::Arithmetic) => {
                    WafErrorCode::Arithmetic
                }
                JsonCheckerError::State
                | JsonCheckerError::AlreadyComplete
                | JsonCheckerError::Lexical(JsonStreamError::Abandoned | JsonStreamError::AlreadyComplete) => {
                    WafErrorCode::PhaseState
                }
                _ => WafErrorCode::BodyProcessing,
            },
            RuntimeError::XmlStream { kind } => match kind {
                XmlStreamError::Limit {
                    kind: XmlStreamLimit::SourceBytes,
                    ..
                } => WafErrorCode::BodyLimit,
                XmlStreamError::Limit { .. } => WafErrorCode::RuntimeLimit,
                XmlStreamError::Allocation => WafErrorCode::Allocation,
                XmlStreamError::Arithmetic => WafErrorCode::Arithmetic,
                XmlStreamError::Abandoned | XmlStreamError::AlreadyComplete => WafErrorCode::PhaseState,
                XmlStreamError::SinkRejected => WafErrorCode::RuntimeInvariant,
                _ => WafErrorCode::BodyProcessing,
            },
            RuntimeError::XmlResultLimit { .. } | RuntimeError::JsonResultLimit { .. } => WafErrorCode::RuntimeLimit,
            RuntimeError::XmlUnsupported { .. } | RuntimeError::JsonUnsupported { .. } => WafErrorCode::Unsupported,
            RuntimeError::XmlInspectionState | RuntimeError::JsonInspectionState => WafErrorCode::RuntimeInvariant,
            RuntimeError::InputByteLimit { .. }
            | RuntimeError::DetectorInputLimit { .. }
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
            | RuntimeError::CtlEntryLimit { .. }
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
            | RuntimeError::RequestBodyStreamCtlUnavailable
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

    fn body_read_failure(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        error: &BodyReadError,
    ) -> PluginRunningResult {
        let result = body_read_failure_result(session, &BodyRequirement::fail_close(), log, error);
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
        result
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
        // Borrow straight from the live header map: `RequestHeader` is a
        // `(&[u8], &[u8])` view, so materializing owned bytes first only to
        // point at them was 1 + 2N throwaway allocations per request. The
        // borrow ends when `RequestHead` is consumed by the constructor below
        // (`Transaction` stores an owned `RequestState`), before any `&mut
        // session` use.
        let protocol = session.get_http_protocol();
        let request_uri = (protocol == "HTTP/2.0").then(|| session.request_uri());
        let headers = request_header_views(session.request_header_map(), &protocol, request_uri);
        let uri = session.get_uri();
        let raw_uri = session.get_request_uri();
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
        let runtime_limits = RuntimeLimits::default();
        let mut transaction = match Transaction::new_shared_with_policy(
            Arc::clone(&self.shared.generation),
            request,
            runtime_limits,
            edgion_waf::seclang::EvaluationMode::Accelerated,
            self.shared.engine_mode,
            RuleCheckPolicy {
                expensive_checks: session.ctx().egc.waf_expensive_checks,
            },
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

        let processor = if self.shared.request_body_inspection == WafRequestBodyInspection::Full {
            match transaction.preflight_request_body(BodyProcessorSelection::Auto) {
                Ok(processor) => processor,
                Err(error) => return self.runtime_failure(session, WafLogPhase::RequestBody, &error),
            }
        } else {
            BodyProcessorSelection::Auto
        };

        let prefix_mode = self.shared.request_body_inspection == WafRequestBodyInspection::Prefix
            && self.shared.requires_request_body;
        let mut prefix_complete = false;
        let structured_published = self.shared.request_body_inspection == WafRequestBodyInspection::Full
            && matches!(processor, BodyProcessorSelection::Json | BodyProcessorSelection::Xml);
        if structured_published {
            let cap = session.ctx().request_body_config.max_body_size_bytes;
            let inspection = if processor == BodyProcessorSelection::Xml {
                json_capture::inspect_xml(
                    session,
                    &mut transaction,
                    XmlInspectionLimits::default(),
                    runtime_limits,
                    cap,
                    self.shared.requires_request_body,
                )
                .await
            } else {
                json_capture::inspect(
                    session,
                    &mut transaction,
                    self.shared.json_limits,
                    runtime_limits,
                    cap,
                    self.shared.requires_request_body,
                )
                .await
            };
            match inspection {
                Ok(()) => {}
                Err(json_capture::JsonCaptureError::Body(error)) => {
                    return self.body_read_failure(session, log, &error)
                }
                Err(json_capture::JsonCaptureError::Runtime(error)) => {
                    return self.runtime_failure(session, WafLogPhase::RequestBody, &error)
                }
            }
        }
        let mut body = if self.shared.requires_request_body && !structured_published {
            let result = if prefix_mode {
                session
                    .read_request_body_prefix(self.shared.request_body_prefix_bytes)
                    .await
                    .map(|(body, complete)| {
                        prefix_complete = complete;
                        body
                    })
            } else {
                let cap = session.ctx().request_body_config.max_body_size_bytes;
                let captured = match session.ensure_request_body(Some(cap)).await {
                    Ok(len) => len,
                    Err(error) => return self.body_read_failure(session, log, &error),
                };
                let known_bodyless = session.ctx().request_info.body_declared == Some(false);
                let available = match session.request_body_available_len(known_bodyless) {
                    Ok(len) if len == captured => len,
                    Ok(_) => return self.body_read_failure(session, log, &BodyReadError::Incomplete),
                    Err(error) => return self.body_read_failure(session, log, &error),
                };
                if available > cap {
                    return self.body_read_failure(session, log, &BodyReadError::BufferLimitExceeded { limit: cap });
                }
                let Ok(actual) = usize::try_from(available) else {
                    return self.runtime_failure(session, WafLogPhase::RequestBody, &RuntimeError::Arithmetic);
                };
                if actual > runtime_limits.body.max_body_bytes {
                    return self.runtime_failure(
                        session,
                        WafLogPhase::RequestBody,
                        &RuntimeError::Body {
                            kind: BodyError::ByteLimit {
                                actual,
                                limit: runtime_limits.body.max_body_bytes,
                            },
                        },
                    );
                }
                if processor == BodyProcessorSelection::Json
                    && actual > self.shared.json_limits.lexical.max_source_bytes
                {
                    return self.runtime_failure(
                        session,
                        WafLogPhase::RequestBody,
                        &RuntimeError::JsonChecker {
                            kind: JsonCheckerError::Lexical(JsonStreamError::Limit {
                                kind: JsonStreamLimit::SourceBytes,
                                limit: self.shared.json_limits.lexical.max_source_bytes,
                            }),
                        },
                    );
                }
                match session.read_request_body(Some(cap)).await {
                    Ok(body) if body.len() == actual => Ok(body),
                    Ok(_) => Err(BodyReadError::Incomplete),
                    Err(error) => Err(error),
                }
            };
            match result {
                Ok(body) => body,
                Err(error) => return self.body_read_failure(session, log, &error),
            }
        } else {
            bytes::Bytes::new()
        };
        if self.shared.request_body_inspection == WafRequestBodyInspection::Full && !structured_published {
            crate::observe::metrics::record_waf_body_bytes_inspected("request", body.len());
            if let Err(error) = transaction.publish_request_body_with_json_limits(
                RequestBody::new(std::mem::take(&mut body), processor),
                self.shared.json_limits,
            ) {
                return self.runtime_failure(session, WafLogPhase::RequestBody, &error);
            }
        }
        let started = Instant::now();
        let execution = if self.shared.request_body_inspection == WafRequestBodyInspection::Prefix {
            let completion = if !self.shared.requires_request_body {
                RequestBodyPrefixCompletion::NotRequired
            } else if prefix_complete {
                RequestBodyPrefixCompletion::EndOfStream
            } else {
                RequestBodyPrefixCompletion::WindowFilled
            };
            crate::observe::metrics::record_waf_body_bytes_inspected("request", body.len());
            transaction.execute_request_body_prefix(&body, self.shared.request_body_prefix_bytes, completion)
        } else {
            transaction.execute_request_body()
        };
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
        // Borrow the live response map until `publish_response_head` builds
        // the transaction-owned response state. The transaction outlives this
        // callback, but the temporary per-header byte copies do not need to.
        let headers: Vec<_> = session
            .iter_response_headers()
            .map(|(name, value)| ResponseHeader::new(name.as_bytes(), value))
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
#[path = "../../../../tests/unit/plugins/http/waf/mod_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/waf/mod_test/fixture.rs"]
pub(crate) mod fixture;
