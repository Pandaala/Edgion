//! Streamed request-body handler for ExtProc `requestBodyMode: STREAMED`.
//!
//! One handler instance is installed per request by the request-stage plugin
//! run and driven per transport body event by `request_body_filter_action`.
//! Each chunk is exchanged with the external processor over the request-scoped
//! stream opened at the header phase; the processor's body mutation replaces
//! the current chunk in place (never accumulated — the only body cache in the
//! gateway remains `BodyStorage`).

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;

use super::client::ExtProcStream;
use super::plugin::{exchange_failure_reason, sanitize_status};
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{ChunkVerdict, DyeOutcome, PluginLog, RequestBodyStreamHandler};
use edgion_common::ext_proc::proto::{
    body_mutation, common_response::ResponseStatus, processing_request, processing_response, HttpBody,
    ProcessingRequest, ProcessingResponse,
};

/// Per-message transport used by the handler. Abstracted so unit tests can
/// script verdicts without a live gRPC server; the production impl is the
/// request-scoped [`ExtProcStream`].
#[async_trait]
pub trait BodyExchange: Send {
    async fn exchange_body(&mut self, body: HttpBody, last: bool) -> Result<ProcessingResponse, tonic::Status>;
}

#[async_trait]
impl BodyExchange for ExtProcStream {
    async fn exchange_body(&mut self, body: HttpBody, last: bool) -> Result<ProcessingResponse, tonic::Status> {
        let request = ProcessingRequest {
            request: Some(processing_request::Request::RequestBody(body)),
        };
        self.exchange(request, last).await
    }
}

pub struct ExtProcStreamedBodyHandler {
    /// `None` after fail-open deactivation: the stream (and its processor-side
    /// half) is released instead of being pinned for the rest of the request.
    exchange: Option<Box<dyn BodyExchange>>,
    failure_mode_allow: bool,
    /// Set when the handler stops exchanging (fail-open deactivation or a
    /// terminal verdict); later chunks pass through untouched.
    done: bool,
    log: Option<PluginLog>,
    /// Lines pushed since the last `take_log` — gates draining an empty log.
    log_lines: usize,
    dye_plan: Option<Arc<CompiledDyePlan>>,
    /// Failure outcome of the last chunk, drained by the hook for dye
    /// recording (mirrors the request-stage `fail_mode` recording).
    pending_outcome: Option<(DyeOutcome, String)>,
}

impl ExtProcStreamedBodyHandler {
    pub fn new(
        exchange: Box<dyn BodyExchange>,
        failure_mode_allow: bool,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            exchange: Some(exchange),
            failure_mode_allow,
            done: false,
            log: Some(PluginLog::new("ExtProc")),
            log_lines: 0,
            dye_plan,
            pending_outcome: None,
        }
    }

    fn push_log(&mut self, line: &str) {
        if let Some(log) = self.log.as_mut() {
            log.push(line);
            self.log_lines += 1;
        }
    }

    /// Apply the configured failure mode to a per-chunk failure: fail-open
    /// deactivates the handler for the rest of the request, fail-close
    /// terminates with a 500. `reason` follows the request-stage taxonomy
    /// ("unreachable" / "bad verdict") so dye causes and log grammar stay
    /// uniform across stages; `detail` carries the static sub-tag.
    fn fail(&mut self, reason: &str, detail: Option<&str>) -> ChunkVerdict {
        let cause = reason.replace(' ', "-");
        let mode = if self.failure_mode_allow {
            "fail-open"
        } else {
            "fail-close"
        };
        match detail {
            Some(detail) => self.push_log(&format!("ext_proc body {reason}:{detail}:{mode}")),
            None => self.push_log(&format!("ext_proc body {reason}:{mode}")),
        }
        if self.failure_mode_allow {
            self.done = true;
            // Release the stream now instead of pinning it (and its
            // processor-side half) for the rest of a possibly long upload.
            self.exchange = None;
            self.pending_outcome = Some((DyeOutcome::FailOpen, cause));
            ChunkVerdict::Continue
        } else {
            self.pending_outcome = Some((DyeOutcome::FailClose, cause));
            ChunkVerdict::Terminate {
                status: 500,
                body: Some("ext_proc unavailable".to_string()),
                headers: Vec::new(),
            }
        }
    }
}

#[async_trait]
impl RequestBodyStreamHandler for ExtProcStreamedBodyHandler {
    fn name(&self) -> &str {
        "ExtProc"
    }

    async fn on_chunk(&mut self, chunk: &mut Option<Bytes>, end_of_stream: bool) -> ChunkVerdict {
        if self.done {
            return ChunkVerdict::Continue;
        }
        let Some(exchange) = self.exchange.as_mut() else {
            return ChunkVerdict::Continue;
        };
        let body = HttpBody {
            body: chunk.as_ref().map(|b| b.to_vec()).unwrap_or_default(),
            end_of_stream,
        };
        let response = match exchange.exchange_body(body, end_of_stream).await {
            Ok(response) => response,
            Err(status) => {
                let reason = exchange_failure_reason(&status);
                return self.fail(reason, Some(status.code().description()));
            }
        };

        match response.response {
            Some(processing_response::Response::RequestBody(br)) => {
                // CONTINUE_AND_REPLACE mid-stream ("replace and stop sending
                // chunks") is not implemented; never silently downgrade it to
                // a plain CONTINUE — reject per the failure policy.
                if br.response.as_ref().is_some_and(|common| {
                    ResponseStatus::try_from(common.status).ok() == Some(ResponseStatus::ContinueAndReplace)
                }) {
                    return self.fail("bad verdict", Some("replace unsupported in streamed"));
                }
                // Header mutations in a body verdict cannot apply: the
                // upstream request headers are already committed by the time
                // body chunks flow.
                if br.response.as_ref().is_some_and(|c| c.header_mutation.is_some()) {
                    self.push_log("ext_proc body hdr mut ignored");
                }
                if let Some(mutation) = br.response.and_then(|c| c.body_mutation) {
                    match mutation.mutation {
                        // `Bytes::from(Vec<u8>)` takes ownership — no copy.
                        Some(body_mutation::Mutation::Body(bytes)) => {
                            *chunk = Some(Bytes::from(bytes));
                        }
                        Some(body_mutation::Mutation::ClearBody(true)) => {
                            *chunk = Some(Bytes::new());
                        }
                        Some(body_mutation::Mutation::ClearBody(false)) | None => {}
                    }
                }
                if end_of_stream {
                    self.push_log("ext_proc body streamed done");
                }
                ChunkVerdict::Continue
            }
            Some(processing_response::Response::ImmediateResponse(imm)) => {
                let status = imm.status.as_ref().map(|s| sanitize_status(s.code)).unwrap_or(500);
                self.done = true;
                if let Some(log) = self.log.as_mut() {
                    log.push_fmt("ext_proc immediate {}", la![status.to_string().as_str()]);
                    self.log_lines += 1;
                }
                // Carry the immediate response's header mutations into the
                // local reply, matching the request-stage `immediate_reply`
                // semantics. Protected/pseudo headers are rejected here (with
                // the same log lines as the mutation module) so the hook can
                // apply the pairs verbatim.
                let mut headers = Vec::new();
                if let Some(mutation) = imm.headers.as_ref() {
                    for option in &mutation.set_headers {
                        let Some(header) = option.header.as_ref() else { continue };
                        if let Some(log) = self.log.as_mut() {
                            if super::mutation::reject_protected(&header.key, log) {
                                self.log_lines += 1;
                                continue;
                            }
                        }
                        let value = if header.raw_value.is_empty() {
                            header.value.clone()
                        } else {
                            String::from_utf8_lossy(&header.raw_value).into_owned()
                        };
                        headers.push((header.key.clone(), value));
                    }
                }
                let body = (!imm.body.is_empty()).then(|| String::from_utf8_lossy(&imm.body).into_owned());
                ChunkVerdict::Terminate { status, body, headers }
            }
            _ => self.fail("bad verdict", None),
        }
    }

    fn take_log(&mut self) -> Option<PluginLog> {
        if self.log_lines == 0 {
            return None;
        }
        self.log_lines = 0;
        self.log.take()
    }

    fn dye_plan(&self) -> Option<Arc<CompiledDyePlan>> {
        self.dye_plan.clone()
    }

    fn take_pending_outcome(&mut self) -> Option<(DyeOutcome, String)> {
        self.pending_outcome.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_common::ext_proc::proto::{BodyMutation, BodyResponse, CommonResponse, HttpStatus, ImmediateResponse};

    /// Scripted exchange: pops pre-programmed results in order. Panics when
    /// the handler exchanges more often than scripted — which is exactly what
    /// the deactivation tests pin.
    struct ScriptedExchange {
        script: Vec<Result<ProcessingResponse, tonic::Status>>,
    }

    impl ScriptedExchange {
        fn new(script: Vec<Result<ProcessingResponse, tonic::Status>>) -> Self {
            Self { script }
        }
    }

    #[async_trait]
    impl BodyExchange for ScriptedExchange {
        async fn exchange_body(&mut self, _body: HttpBody, _last: bool) -> Result<ProcessingResponse, tonic::Status> {
            assert!(!self.script.is_empty(), "handler exchanged more often than scripted");
            self.script.remove(0)
        }
    }

    fn body_verdict(mutation: Option<body_mutation::Mutation>) -> ProcessingResponse {
        ProcessingResponse {
            response: Some(processing_response::Response::RequestBody(BodyResponse {
                response: Some(CommonResponse {
                    body_mutation: mutation.map(|mutation| BodyMutation {
                        mutation: Some(mutation),
                    }),
                    ..Default::default()
                }),
            })),
        }
    }

    fn handler_with(
        script: Vec<Result<ProcessingResponse, tonic::Status>>,
        failure_mode_allow: bool,
    ) -> ExtProcStreamedBodyHandler {
        ExtProcStreamedBodyHandler::new(Box::new(ScriptedExchange::new(script)), failure_mode_allow, None)
    }

    #[tokio::test]
    async fn body_mutation_replaces_current_chunk() {
        let mut handler = handler_with(
            vec![Ok(body_verdict(Some(body_mutation::Mutation::Body(
                b"mutated".to_vec(),
            ))))],
            false,
        );
        let mut chunk = Some(Bytes::from_static(b"original"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, false).await,
            ChunkVerdict::Continue
        ));
        assert_eq!(chunk.as_deref(), Some(&b"mutated"[..]));
    }

    #[tokio::test]
    async fn clear_body_empties_current_chunk() {
        let mut handler = handler_with(
            vec![Ok(body_verdict(Some(body_mutation::Mutation::ClearBody(true))))],
            false,
        );
        let mut chunk = Some(Bytes::from_static(b"secret"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, false).await,
            ChunkVerdict::Continue
        ));
        assert_eq!(chunk.as_deref(), Some(&b""[..]));
    }

    #[tokio::test]
    async fn immediate_response_terminates_with_sanitized_status() {
        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(ImmediateResponse {
                status: Some(HttpStatus { code: 100 }),
                body: b"denied".to_vec(),
                ..Default::default()
            })),
        };
        let mut handler = handler_with(vec![Ok(immediate)], false);
        let mut chunk = Some(Bytes::from_static(b"payload"));
        match handler.on_chunk(&mut chunk, false).await {
            ChunkVerdict::Terminate { status, body, headers } => {
                assert_eq!(status, 500, "1xx is not a valid final reply");
                assert_eq!(body.as_deref(), Some("denied"));
                assert!(headers.is_empty());
            }
            ChunkVerdict::Continue => panic!("immediate response must terminate"),
        }
        assert!(handler.take_log().is_some());
    }

    /// Immediate-response header mutations are carried into the terminate
    /// verdict, with protected/pseudo headers filtered out by the handler.
    #[tokio::test]
    async fn immediate_response_headers_are_carried_and_filtered() {
        use edgion_common::ext_proc::proto::{HeaderAppendAction, HeaderMutation, HeaderValue};

        let set = |key: &str, value: &str| edgion_common::ext_proc::proto::HeaderValueOption {
            header: Some(HeaderValue {
                key: key.to_string(),
                value: value.to_string(),
                raw_value: Vec::new(),
            }),
            append_action: HeaderAppendAction::OverwriteIfExistsOrAdd as i32,
            keep_empty_value: false,
        };
        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(ImmediateResponse {
                status: Some(HttpStatus { code: 403 }),
                headers: Some(HeaderMutation {
                    set_headers: vec![set("x-denied-by", "policy"), set("content-length", "999")],
                    remove_headers: Vec::new(),
                }),
                body: b"no".to_vec(),
                ..Default::default()
            })),
        };
        let mut handler = handler_with(vec![Ok(immediate)], false);
        let mut chunk = Some(Bytes::from_static(b"payload"));
        match handler.on_chunk(&mut chunk, false).await {
            ChunkVerdict::Terminate { status, headers, .. } => {
                assert_eq!(status, 403);
                assert_eq!(headers, vec![("x-denied-by".to_string(), "policy".to_string())]);
            }
            ChunkVerdict::Continue => panic!("immediate response must terminate"),
        }
    }

    #[tokio::test]
    async fn transport_error_fail_close_terminates_500() {
        let mut handler = handler_with(vec![Err(tonic::Status::unavailable("down"))], false);
        let mut chunk = Some(Bytes::from_static(b"x"));
        match handler.on_chunk(&mut chunk, false).await {
            ChunkVerdict::Terminate { status, body, .. } => {
                assert_eq!(status, 500);
                assert_eq!(body.as_deref(), Some("ext_proc unavailable"));
            }
            ChunkVerdict::Continue => panic!("fail-close must terminate"),
        }
    }

    /// Fail-open deactivates the handler: the scripted exchange holds exactly
    /// one error, so a second exchange attempt would panic the script.
    #[tokio::test]
    async fn transport_error_fail_open_deactivates() {
        let mut handler = handler_with(vec![Err(tonic::Status::unavailable("down"))], true);
        let mut chunk = Some(Bytes::from_static(b"x"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, false).await,
            ChunkVerdict::Continue
        ));
        assert_eq!(chunk.as_deref(), Some(&b"x"[..]), "fail-open must not drop the chunk");
        let outcome = handler.take_pending_outcome().expect("fail-open records a dye outcome");
        assert!(matches!(outcome.0, DyeOutcome::FailOpen));
        let mut next = Some(Bytes::from_static(b"y"));
        assert!(matches!(
            handler.on_chunk(&mut next, true).await,
            ChunkVerdict::Continue
        ));
        assert!(handler.take_pending_outcome().is_none(), "outcome drains once");
        assert!(handler.take_log().is_some());
    }

    /// CONTINUE_AND_REPLACE mid-stream is unimplemented ("replace and stop")
    /// and must be rejected per the failure policy, never silently downgraded
    /// to CONTINUE.
    #[tokio::test]
    async fn mid_body_continue_and_replace_is_bad_verdict() {
        let cr = ProcessingResponse {
            response: Some(processing_response::Response::RequestBody(BodyResponse {
                response: Some(CommonResponse {
                    status: ResponseStatus::ContinueAndReplace as i32,
                    body_mutation: Some(BodyMutation {
                        mutation: Some(body_mutation::Mutation::Body(b"replacement".to_vec())),
                    }),
                    ..Default::default()
                }),
            })),
        };
        let mut handler = handler_with(vec![Ok(cr)], false);
        let mut chunk = Some(Bytes::from_static(b"x"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, false).await,
            ChunkVerdict::Terminate { status: 500, .. }
        ));
        assert_eq!(
            chunk.as_deref(),
            Some(&b"x"[..]),
            "the rejected CR mutation must not have been applied"
        );
    }

    #[tokio::test]
    async fn wrong_phase_verdict_follows_failure_policy() {
        use edgion_common::ext_proc::proto::HeadersResponse;
        let wrong = ProcessingResponse {
            response: Some(processing_response::Response::ResponseHeaders(
                HeadersResponse::default(),
            )),
        };
        let mut handler = handler_with(vec![Ok(wrong)], false);
        let mut chunk = Some(Bytes::from_static(b"x"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, false).await,
            ChunkVerdict::Terminate { status: 500, .. }
        ));
    }

    /// A pure EOS event (None chunk) still reaches the processor as an empty
    /// end_of_stream body message, half-closing the outbound direction. The
    /// recording exchange asserts the on-wire shape from inside the script.
    #[tokio::test]
    async fn eos_event_sends_empty_terminal_body() {
        struct AssertingExchange;

        #[async_trait]
        impl BodyExchange for AssertingExchange {
            async fn exchange_body(&mut self, body: HttpBody, last: bool) -> Result<ProcessingResponse, tonic::Status> {
                assert!(body.body.is_empty());
                assert!(body.end_of_stream);
                assert!(last, "EOS must half-close the outbound direction");
                Ok(body_verdict(None))
            }
        }

        let mut handler = ExtProcStreamedBodyHandler::new(Box::new(AssertingExchange), false, None);
        let mut chunk = None;
        assert!(matches!(
            handler.on_chunk(&mut chunk, true).await,
            ChunkVerdict::Continue
        ));
        assert!(handler.take_log().is_some(), "streamed-done line expected at EOS");
    }

    #[tokio::test]
    async fn take_log_is_none_without_entries() {
        let mut handler = handler_with(vec![Ok(body_verdict(None))], false);
        let mut chunk = Some(Bytes::from_static(b"x"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, false).await,
            ChunkVerdict::Continue
        ));
        assert!(handler.take_log().is_none(), "mid-stream success logs nothing");
    }
}
