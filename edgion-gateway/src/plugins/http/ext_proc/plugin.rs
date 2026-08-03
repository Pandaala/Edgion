//! ExtProc plugin — external processing over a bidirectional gRPC stream.
//!
//! Covers the request-header phase (`RequestFilter`) and response-header
//! phase (the async `UpstreamResponse` hook). The processor may mutate headers
//! or return an `ImmediateResponse` to short-circuit. The request phase also
//! supports a fully buffered body and body replacement. Response-body modes
//! remain rejected until the response buffering seam exists.
//!
//! A single plugin type implements both traits; the runtime constructs a
//! separate instance per stage from the same config.

use async_trait::async_trait;
use bytes::Bytes;

use super::client::ExtProcClient;
use super::mutation;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter, UpstreamResponse};
use edgion_common::ext_proc::proto::{
    body_mutation, common_response::ResponseStatus, processing_request, processing_response, CommonResponse,
    HeaderMutation, HttpBody, HttpHeaders, ImmediateResponse, ProcessingRequest,
};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::{ExtProcConfig, ProcessingMode};

const PLUGIN_NAME: &str = "ExtProc";

pub(super) fn exchange_failure_reason(status: &tonic::Status) -> &'static str {
    if status.code() == tonic::Code::FailedPrecondition {
        "bad verdict"
    } else {
        "unreachable"
    }
}

pub struct ExtProc {
    name: String,
    client: ExtProcClient,
    mode: ProcessingMode,
    failure_mode_allow: bool,
}

impl ExtProc {
    fn build(config: &ExtProcConfig) -> Option<Self> {
        match ExtProcClient::new(config) {
            Ok(client) => Some(ExtProc {
                name: PLUGIN_NAME.to_string(),
                client,
                mode: config.processing_mode.clone(),
                failure_mode_allow: config.failure_mode_allow,
            }),
            Err(e) => {
                // Validation already gated the config, so a build failure here is
                // unexpected; fail closed by declining to construct the plugin.
                tracing::error!(error = %e, "ext_proc client build failed; plugin disabled (fail-closed)");
                None
            }
        }
    }

    /// Construct the request-stage filter, or `None` to fail closed.
    pub fn create_request(config: &ExtProcConfig) -> Option<Box<dyn RequestFilter>> {
        Self::build(config).map(|p| Box::new(p) as Box<dyn RequestFilter>)
    }

    /// Construct the response-stage filter, or `None` to fail closed.
    pub fn create_response(config: &ExtProcConfig) -> Option<Box<dyn UpstreamResponse>> {
        Self::build(config).map(|p| Box::new(p) as Box<dyn UpstreamResponse>)
    }

    /// Decide a phase result when the processor could not be consulted or gave an
    /// unusable answer, honoring `failure_mode_allow`: fail-open continues the
    /// chain (`GoodNext`), fail-close short-circuits with a 500. Used for BOTH
    /// transport/gRPC errors AND malformed/wrong-phase verdicts — a buggy
    /// processor must not silently bypass a configured fail-close.
    ///
    /// `detail` carries an optional sub-tag (e.g. the gRPC status code's static
    /// description for transport failures) so the access-log PluginLog entry
    /// preserves the failure category without resorting to data-plane tracing.
    ///
    /// This is also the single choke point for the dye suite's dependency-
    /// failure recording: `record_outcome` is called here (before returning
    /// `PluginRunningResult`, so it is always ordered before the runtime's
    /// own response write) with `reason` kebab-cased as the cause.
    fn fail_mode(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        reason: &str,
        detail: Option<&str>,
    ) -> PluginRunningResult {
        match detail {
            Some(d) if self.failure_mode_allow => {
                log.push_fmt("ext_proc {}:{}:fail-open", la![reason, d]);
            }
            Some(d) => {
                log.push_fmt("ext_proc {}:{}:fail-close", la![reason, d]);
            }
            None if self.failure_mode_allow => {
                log.push_fmt("ext_proc {}:fail-open", la![reason]);
            }
            None => {
                log.push_fmt("ext_proc {}:fail-close", la![reason]);
            }
        }
        let cause = reason.replace(' ', "-");
        if self.failure_mode_allow {
            session.record_outcome(DyeOutcome::FailOpen, &cause);
            PluginRunningResult::GoodNext
        } else {
            session.record_outcome(DyeOutcome::FailClose, &cause);
            PluginRunningResult::ErrResponse {
                status: 500,
                body: Some("ext_proc unavailable".to_string()),
            }
        }
    }

    async fn apply_request_common_response(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        mut common: Option<CommonResponse>,
        apply_body_mutation: bool,
    ) -> Result<(), PluginRunningResult> {
        let Some(common) = common.as_mut() else {
            return Ok(());
        };
        if apply_body_mutation {
            if let Some(mutation) = common.body_mutation.take() {
                let body = match mutation.mutation {
                    Some(body_mutation::Mutation::Body(body)) => Some(Bytes::from(body)),
                    Some(body_mutation::Mutation::ClearBody(true)) => Some(Bytes::new()),
                    Some(body_mutation::Mutation::ClearBody(false)) | None => None,
                };
                if let Some(body) = body {
                    session
                        .overwrite_request_body(body)
                        .await
                        .map_err(|_| self.fail_mode(session, log, "body mutation failed", None))?;
                }
            }
        }
        if let Some(headers) = common.header_mutation.as_ref() {
            let count = mutation::apply_to_request(session, headers, log);
            log.push_fmt("req hdr mut {}", la![count.to_string().as_str()]);
        }
        Ok(())
    }

    /// `requestBodyMode: STREAMED` request phase: open the request-scoped
    /// stream, run the header phase on it, then install the per-chunk body
    /// handler. Installing a handler makes the request non-replayable and
    /// switches upstream framing to the streamed disposition.
    async fn run_request_streamed(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Gate on the latched transport fact, NOT `declares_body()`: the
        // latter infers from Content-Length, which an H2 client can
        // contradict (declare CL:0, still send DATA frames) to smuggle body
        // bytes past inspection. `None` = the session type cannot supply the
        // fact (custom-connector, subrequest) — the per-chunk capability is
        // absent there and the fail mode applies.
        let headers_end_stream = match session.request_headers_end_stream() {
            Some(fact) => fact,
            None => return self.fail_mode(session, log, "streamed unsupported", None),
        };

        // Transport-guaranteed bodyless request: nothing to stream. Degenerate
        // to the header-only exchange so retry/framing semantics stay
        // untouched.
        if headers_end_stream {
            if !self.mode.request_header_mode.is_send() {
                return PluginRunningResult::GoodNext;
            }
            let request = ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                    headers: Some(mutation::collect_request_headers(session)),
                    end_of_stream: true,
                })),
            };
            let response = match self.client.exchange(request).await {
                Ok(response) => response,
                Err(status) => {
                    let reason = exchange_failure_reason(&status);
                    return self.fail_mode(session, log, reason, Some(status.code().description()));
                }
            };
            return match response.response {
                Some(processing_response::Response::RequestHeaders(hr)) => {
                    // Same rejection as the non-degenerate streamed branch:
                    // CONTINUE_AND_REPLACE has no buffered body to replace in
                    // streamed mode — never silently ignore it.
                    if hr.response.as_ref().is_some_and(|common| {
                        ResponseStatus::try_from(common.status).ok() == Some(ResponseStatus::ContinueAndReplace)
                    }) {
                        return self.fail_mode(session, log, "bad verdict", Some("replace unsupported in streamed"));
                    }
                    match self
                        .apply_request_common_response(session, log, hr.response, false)
                        .await
                    {
                        Ok(()) => PluginRunningResult::GoodNext,
                        Err(result) => result,
                    }
                }
                Some(processing_response::Response::ImmediateResponse(imm)) => immediate_reply(session, log, imm),
                _ => self.fail_mode(session, log, "bad verdict", None),
            };
        }

        // Lazy open: no network work yet — a connect/handshake failure
        // surfaces from the first exchange below and takes the fail mode.
        let mut stream = self.client.open_stream();

        if self.mode.request_header_mode.is_send() {
            let request = ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                    headers: Some(mutation::collect_request_headers(session)),
                    end_of_stream: false,
                })),
            };
            let response = match stream.exchange(request, false).await {
                Ok(response) => response,
                Err(status) => {
                    let reason = exchange_failure_reason(&status);
                    return self.fail_mode(session, log, reason, Some(status.code().description()));
                }
            };
            match response.response {
                Some(processing_response::Response::RequestHeaders(hr)) => {
                    // CONTINUE_AND_REPLACE cannot apply in streamed mode: there
                    // is no cached body to replace and the chunk stream has not
                    // started yet.
                    if hr.response.as_ref().is_some_and(|common| {
                        ResponseStatus::try_from(common.status).ok() == Some(ResponseStatus::ContinueAndReplace)
                    }) {
                        return self.fail_mode(session, log, "bad verdict", Some("replace unsupported in streamed"));
                    }
                    if let Err(result) = self
                        .apply_request_common_response(session, log, hr.response, false)
                        .await
                    {
                        return result;
                    }
                }
                Some(processing_response::Response::ImmediateResponse(imm)) => {
                    return immediate_reply(session, log, imm);
                }
                _ => return self.fail_mode(session, log, "bad verdict", None),
            }
        }

        let handler = super::streamed::ExtProcStreamedBodyHandler::new(
            Box::new(stream),
            self.failure_mode_allow,
            session.current_dye_plan(),
        );
        if !session.install_request_body_stream_handler(Box::new(handler)) {
            return self.fail_mode(session, log, "streamed unsupported", None);
        }
        log.push("ext_proc body streamed");
        PluginRunningResult::GoodNext
    }

    fn apply_response_headers_response(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        common: Option<CommonResponse>,
    ) -> PluginRunningResult {
        if common.as_ref().is_some_and(|response| {
            ResponseStatus::try_from(response.status).ok() == Some(ResponseStatus::ContinueAndReplace)
        }) {
            return self.fail_mode(
                session,
                log,
                "bad verdict",
                Some("response body replacement unsupported"),
            );
        }
        if let Some(mutation) = headers_mutation(common.as_ref()) {
            let count = mutation::apply_to_response(session, mutation, log);
            log.push_fmt("resp hdr mut {}", la![count.to_string().as_str()]);
        }
        PluginRunningResult::GoodNext
    }
}

/// Clamp a processor-supplied status code into a valid final-response range.
/// 1xx is not a valid final reply, so out-of-range codes map to 500.
pub(super) fn sanitize_status(code: i32) -> u16 {
    if (200..=599).contains(&code) {
        code as u16
    } else {
        500
    }
}

/// Map a processor `ImmediateResponse` onto a short-circuit `ErrResponse`.
///
/// The runtime's `ErrResponse` handling is gRPC-route aware (emitting
/// grpc-status trailers when appropriate) in both stages, and the body is
/// carried as the reply body.
///
/// Header mutations (minus protected headers) are applied via the session. In
/// the **request** stage these are queued and drained into the local reply, so
/// they reach the client. In the **response** stage the runtime rebuilds a fresh
/// local reply, so a response-phase `ImmediateResponse`'s custom headers are NOT
/// preserved (only status, plus body on non-gRPC routes — gRPC routes terminate
/// status-only). Known limitations for the response-stage short-circuit.
fn immediate_reply(
    session: &mut dyn PluginSession,
    log: &mut PluginLog,
    imm: ImmediateResponse,
) -> PluginRunningResult {
    let status = imm.status.map(|s| sanitize_status(s.code)).unwrap_or(500);
    if let Some(headers) = imm.headers.as_ref() {
        mutation::apply_to_response(session, headers, log);
    }
    let body = (!imm.body.is_empty()).then(|| String::from_utf8_lossy(&imm.body).into_owned());
    log.push_fmt("ext_proc immediate {}", la![status.to_string().as_str()]);
    PluginRunningResult::ErrResponse { status, body }
}

/// Extract the `CommonResponse.header_mutation` from a headers verdict, if any.
fn headers_mutation(common: Option<&edgion_common::ext_proc::proto::CommonResponse>) -> Option<&HeaderMutation> {
    common.and_then(|c| c.header_mutation.as_ref())
}

#[async_trait]
impl RequestFilter for ExtProc {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if self.mode.request_body_mode.is_streamed() {
            return self.run_request_streamed(session, log).await;
        }
        let send_headers = self.mode.request_header_mode.is_send();
        let send_body = self.mode.request_body_mode.is_buffered();
        if !send_headers && !send_body {
            return PluginRunningResult::GoodNext;
        }

        let body = if send_body {
            match session.read_request_body(None).await {
                Ok(body) => Some(body),
                Err(_) => return self.fail_mode(session, log, "body unavailable", None),
            }
        } else {
            None
        };
        let mut requests = Vec::with_capacity(2);
        if send_headers {
            requests.push(ProcessingRequest {
                request: Some(processing_request::Request::RequestHeaders(HttpHeaders {
                    headers: Some(mutation::collect_request_headers(session)),
                    end_of_stream: !send_body,
                })),
            });
        }
        if let Some(body) = body {
            requests.push(ProcessingRequest {
                request: Some(processing_request::Request::RequestBody(HttpBody {
                    body: body.to_vec(),
                    end_of_stream: true,
                })),
            });
        }

        let responses = match self.client.exchange_all(requests).await {
            Ok(responses) => responses,
            Err(status) => {
                let reason = exchange_failure_reason(&status);
                return self.fail_mode(session, log, reason, Some(status.code().description()));
            }
        };

        for response in responses {
            match response.response {
                Some(processing_response::Response::RequestHeaders(hr)) => {
                    let apply_body_mutation = hr.response.as_ref().is_some_and(|common| {
                        ResponseStatus::try_from(common.status).ok() == Some(ResponseStatus::ContinueAndReplace)
                    });
                    if let Err(result) = self
                        .apply_request_common_response(session, log, hr.response, apply_body_mutation)
                        .await
                    {
                        return result;
                    }
                }
                Some(processing_response::Response::RequestBody(br)) => {
                    if let Err(result) = self
                        .apply_request_common_response(session, log, br.response, true)
                        .await
                    {
                        return result;
                    }
                }
                Some(processing_response::Response::ImmediateResponse(imm)) => {
                    return immediate_reply(session, log, imm);
                }
                _ => return self.fail_mode(session, log, "bad verdict", None),
            }
        }
        PluginRunningResult::GoodNext
    }
}

#[async_trait]
impl UpstreamResponse for ExtProc {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if !self.mode.response_header_mode.is_send() {
            return PluginRunningResult::GoodNext;
        }

        let request = ProcessingRequest {
            request: Some(processing_request::Request::ResponseHeaders(HttpHeaders {
                headers: Some(mutation::collect_response_headers(session)),
                end_of_stream: true,
            })),
        };

        let resp = match self.client.exchange(request).await {
            Ok(resp) => resp,
            Err(status) => {
                return self.fail_mode(
                    session,
                    log,
                    exchange_failure_reason(&status),
                    Some(status.code().description()),
                );
            }
        };

        match resp.response {
            Some(processing_response::Response::ResponseHeaders(hr)) => {
                self.apply_response_headers_response(session, log, hr.response)
            }
            Some(processing_response::Response::ImmediateResponse(imm)) => immediate_reply(session, log, imm),
            _ => self.fail_mode(session, log, "bad verdict", None),
        }
    }
}

// ========== Tests ==========

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::plugin_configs::GrpcServiceConfig;

    fn streamed_config(failure_mode_allow: bool, header_send: bool) -> ExtProcConfig {
        use edgion_resources::plugin_configs::{BodySendMode, HeaderSendMode};
        let mut config = test_config(failure_mode_allow);
        config.processing_mode.request_body_mode = BodySendMode::Streamed;
        if !header_send {
            config.processing_mode.request_header_mode = HeaderSendMode::Skip;
        }
        config
    }

    /// STREAMED on a session that cannot supply the transport fact
    /// (custom-connector, subrequest, mocks) must fail per policy, before any
    /// network work.
    #[tokio::test]
    async fn streamed_without_transport_fact_fails_per_policy() {
        let ext_proc = ExtProc::build(&streamed_config(false, true)).expect("build must not dial");
        let mut session = MockPluginSession::new();
        session.expect_request_headers_end_stream().return_const(None);
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "streamed-unsupported")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.run_request(&mut session, &mut log).await;
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }

    /// SKIP headers + a body-bearing request: the handler installs without any
    /// network exchange, and the request-stage verdict is GoodNext.
    #[tokio::test]
    async fn streamed_skip_headers_installs_handler_without_network() {
        let ext_proc = ExtProc::build(&streamed_config(false, false)).expect("build must not dial");
        let mut session = MockPluginSession::new();
        session.expect_request_headers_end_stream().return_const(Some(false));
        session.expect_current_dye_plan().return_const(None);
        session
            .expect_install_request_body_stream_handler()
            .times(1)
            .return_const(true);
        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.run_request(&mut session, &mut log).await;
        assert!(matches!(result, PluginRunningResult::GoodNext));
    }

    /// The install itself can refuse (capability re-check); the plugin must
    /// then apply its failure policy rather than continue silently.
    #[tokio::test]
    async fn streamed_install_refusal_fails_per_policy() {
        let ext_proc = ExtProc::build(&streamed_config(true, false)).expect("build must not dial");
        let mut session = MockPluginSession::new();
        session.expect_request_headers_end_stream().return_const(Some(false));
        session.expect_current_dye_plan().return_const(None);
        session
            .expect_install_request_body_stream_handler()
            .times(1)
            .return_const(false);
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "streamed-unsupported")
            .times(1)
            .return_const(());
        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.run_request(&mut session, &mut log).await;
        assert!(matches!(result, PluginRunningResult::GoodNext));
    }

    /// A structurally valid config (routable authority, never actually
    /// dialed by these tests — `fail_mode` is exercised directly).
    fn test_config(failure_mode_allow: bool) -> ExtProcConfig {
        ExtProcConfig {
            grpc_service: GrpcServiceConfig {
                target: "127.0.0.1:1".to_string(),
                ..ExtProcConfig::default().grpc_service
            },
            failure_mode_allow,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn request_body_response_overwrites_buffered_body() {
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_overwrite_request_body()
            .withf(|body| body.as_ref() == b"processed")
            .returning(|_| Ok(()));
        let response = CommonResponse {
            body_mutation: Some(BodyMutation {
                mutation: Some(body_mutation::Mutation::Body(b"processed".to_vec())),
            }),
            ..Default::default()
        };
        let mut log = PluginLog::new("ExtProc");
        assert!(ext_proc
            .apply_request_common_response(&mut session, &mut log, Some(response), true)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn header_continue_ignores_header_phase_body_mutation() {
        use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session.expect_overwrite_request_body().never();
        let response = CommonResponse {
            status: ResponseStatus::Continue as i32,
            body_mutation: Some(BodyMutation {
                mutation: Some(body_mutation::Mutation::Body(b"must-not-apply".to_vec())),
            }),
            ..Default::default()
        };

        let mut log = PluginLog::new("ExtProc");
        assert!(ext_proc
            .apply_request_common_response(&mut session, &mut log, Some(response), false)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn request_body_response_applies_header_mutation() {
        use edgion_common::ext_proc::proto::{HeaderAppendAction, HeaderMutation, HeaderValue, HeaderValueOption};

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_set_request_header()
            .withf(|name, value| name == "x-body-verdict" && value == "applied")
            .times(1)
            .returning(|_, _| Ok(()));
        let response = CommonResponse {
            header_mutation: Some(HeaderMutation {
                set_headers: vec![HeaderValueOption {
                    header: Some(HeaderValue {
                        key: "x-body-verdict".to_string(),
                        value: "applied".to_string(),
                        raw_value: Vec::new(),
                    }),
                    append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
                    keep_empty_value: false,
                }],
                remove_headers: Vec::new(),
            }),
            ..Default::default()
        };

        let mut log = PluginLog::new("ExtProc");
        assert!(ext_proc
            .apply_request_common_response(&mut session, &mut log, Some(response), true)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn response_header_continue_and_replace_uses_failure_policy() {
        let ext_proc = ExtProc::build(&test_config(true)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "bad-verdict")
            .times(1)
            .return_const(());
        let response = CommonResponse {
            status: ResponseStatus::ContinueAndReplace as i32,
            ..Default::default()
        };

        let mut log = PluginLog::new("ExtProc");
        assert!(matches!(
            ext_proc.apply_response_headers_response(&mut session, &mut log, Some(response)),
            PluginRunningResult::GoodNext
        ));
    }

    #[tokio::test]
    async fn request_body_mutation_failure_honors_fail_open() {
        use crate::plugins::runtime::traits::session::BodyReadError;
        use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

        let ext_proc = ExtProc::build(&test_config(true)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_overwrite_request_body()
            .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "body-mutation-failed")
            .times(1)
            .return_const(());
        let response = CommonResponse {
            body_mutation: Some(BodyMutation {
                mutation: Some(body_mutation::Mutation::Body(b"oversized".to_vec())),
            }),
            ..Default::default()
        };

        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc
            .apply_request_common_response(&mut session, &mut log, Some(response), true)
            .await;
        assert!(matches!(result, Err(PluginRunningResult::GoodNext)));
    }

    #[tokio::test]
    async fn request_body_mutation_failure_honors_fail_close() {
        use crate::plugins::runtime::traits::session::BodyReadError;
        use edgion_common::ext_proc::proto::{body_mutation, BodyMutation, CommonResponse};

        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_overwrite_request_body()
            .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 4 }));
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "body-mutation-failed")
            .times(1)
            .return_const(());
        let response = CommonResponse {
            body_mutation: Some(BodyMutation {
                mutation: Some(body_mutation::Mutation::Body(b"oversized".to_vec())),
            }),
            ..Default::default()
        };

        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc
            .apply_request_common_response(&mut session, &mut log, Some(response), true)
            .await;
        assert!(matches!(
            result,
            Err(PluginRunningResult::ErrResponse { status: 500, .. })
        ));
    }

    /// `fail_mode` is the single choke point both `run_request` and
    /// `run_upstream_response` route through on a transport error or a
    /// malformed/wrong-phase verdict. Fail-open arm: records `FailOpen` with
    /// the kebab-cased `reason` as cause, then continues (`GoodNext`).
    #[tokio::test]
    async fn fail_mode_fail_open_records_outcome() {
        let ext_proc = ExtProc::build(&test_config(true)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "unreachable")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.fail_mode(&mut session, &mut log, "unreachable", Some("deadline exceeded"));
        assert!(matches!(result, PluginRunningResult::GoodNext));
    }

    /// Fail-close arm: records `FailClose`, then returns `ErrResponse` (a
    /// value the runtime writes AFTER this call returns — record_outcome is
    /// therefore always ordered before that write by construction, since it
    /// executes before `fail_mode`'s own `return`).
    #[tokio::test]
    async fn fail_mode_fail_close_records_outcome() {
        let ext_proc = ExtProc::build(&test_config(false)).expect("client build must not touch the network");
        let mut session = MockPluginSession::new();
        session
            .expect_record_outcome()
            .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "bad-verdict")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("ExtProc");
        let result = ext_proc.fail_mode(&mut session, &mut log, "bad verdict", None);
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }
}
