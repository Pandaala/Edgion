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
use pingora_proxy::RequestBodyEvent;

use super::attached::AttachedExtProc;
use super::plugin::{exchange_failure_reason, has_unsupported_common_fields, immediate_action, ImmediateAction};
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::{ChunkVerdict, DyeOutcome, PluginLog, RequestBodyStreamHandler};
use edgion_common::ext_proc::proto::{
    body_mutation, common_response::ResponseStatus, processing_request, processing_response, HttpBody,
    ProcessingRequest, ProcessingResponse,
};

/// Per-message transport used by the handler. Abstracted so unit tests can
/// script verdicts without a live gRPC server; the production impl is the
/// request-scoped attached owner.
#[async_trait]
pub trait BodyExchange: Send {
    async fn exchange_body(&mut self, body: HttpBody, last: bool) -> Result<ProcessingResponse, tonic::Status>;

    fn cancel(&mut self) {}
    fn terminal(&mut self, _outcome: &'static str, _reason: &'static str) {}
}

#[async_trait]
impl BodyExchange for Arc<AttachedExtProc> {
    async fn exchange_body(&mut self, body: HttpBody, last: bool) -> Result<ProcessingResponse, tonic::Status> {
        let request = ProcessingRequest {
            request: Some(processing_request::Request::RequestBody(body)),
        };
        AttachedExtProc::exchange(self, request, last).await
    }

    fn cancel(&mut self) {
        AttachedExtProc::close(self);
    }

    fn terminal(&mut self, outcome: &'static str, reason: &'static str) {
        AttachedExtProc::mark_terminal(self, outcome, reason);
    }
}

pub struct ExtProcStreamedBodyHandler {
    /// `None` after fail-open deactivation: the stream (and its processor-side
    /// half) is released instead of being pinned for the rest of the request.
    exchange: Option<Box<dyn BodyExchange>>,
    failure_mode_allow: bool,
    disable_immediate_response: bool,
    /// Set when the handler stops exchanging (fail-open deactivation or a
    /// terminal verdict); later chunks pass through untouched.
    done: bool,
    dye_plan: Option<Arc<CompiledDyePlan>>,
    /// Failure outcome of the last chunk, drained by the hook for dye
    /// recording (mirrors the request-stage `fail_mode` recording).
    pending_outcome: Option<(DyeOutcome, String)>,
    terminal_at_request_eos: bool,
    log: PluginLog,
    has_log: bool,
}

impl ExtProcStreamedBodyHandler {
    pub fn new(
        exchange: Box<dyn BodyExchange>,
        failure_mode_allow: bool,
        disable_immediate_response: bool,
        dye_plan: Option<Arc<CompiledDyePlan>>,
        terminal_at_request_eos: bool,
    ) -> Self {
        Self {
            exchange: Some(exchange),
            failure_mode_allow,
            disable_immediate_response,
            done: false,
            dye_plan,
            pending_outcome: None,
            terminal_at_request_eos,
            log: PluginLog::new("ExtProc"),
            has_log: false,
        }
    }

    /// Apply the configured failure mode to a per-chunk failure: fail-open
    /// deactivates the handler for the rest of the request, fail-close
    /// terminates with a 500. `reason` follows the request-stage taxonomy
    /// ("unreachable" / "bad verdict") so dye causes and log grammar stay
    /// uniform across stages; `detail` carries the static sub-tag.
    fn fail(&mut self, reason: &str, detail: Option<&str>) -> ChunkVerdict {
        let cause = reason.replace(' ', "-");
        let _ = detail;
        let terminal_reason = if reason == "bad verdict" {
            "bad-verdict"
        } else {
            "dependency"
        };
        if self.failure_mode_allow {
            self.done = true;
            // Release the stream now instead of pinning it (and its
            // processor-side half) for the rest of a possibly long upload.
            if let Some(exchange) = self.exchange.as_mut() {
                exchange.terminal("fail-open", terminal_reason);
            }
            self.exchange = None;
            self.pending_outcome = Some((DyeOutcome::FailOpen, cause));
            ChunkVerdict::Continue
        } else {
            if let Some(exchange) = self.exchange.as_mut() {
                exchange.terminal("fail-close", terminal_reason);
            }
            self.pending_outcome = Some((DyeOutcome::FailClose, cause));
            ChunkVerdict::Terminate {
                status: 500,
                grpc_status: None,
                body: None,
                header_ops: Vec::new(),
            }
        }
    }
}

#[async_trait]
impl RequestBodyStreamHandler for ExtProcStreamedBodyHandler {
    async fn on_chunk(&mut self, chunk: &mut Option<Bytes>, event: RequestBodyEvent) -> ChunkVerdict {
        if self.done {
            return ChunkVerdict::Continue;
        }
        if event == RequestBodyEvent::Abandoned {
            self.done = true;
            if let Some(exchange) = self.exchange.as_mut() {
                exchange.terminal("cancelled", "request-abandoned");
                exchange.cancel();
            }
            self.exchange = None;
            return ChunkVerdict::Continue;
        }
        let Some(exchange) = self.exchange.as_mut() else {
            return ChunkVerdict::Continue;
        };
        let end_of_stream = event.is_complete();
        let body = HttpBody {
            body: chunk.as_ref().map(|b| b.to_vec()).unwrap_or_default(),
            end_of_stream,
        };
        let response = match exchange
            .exchange_body(body, end_of_stream && self.terminal_at_request_eos)
            .await
        {
            Ok(response) => response,
            Err(status) => {
                let reason = exchange_failure_reason(&status);
                return self.fail(reason, Some(status.code().description()));
            }
        };

        match response.response {
            Some(processing_response::Response::RequestBody(br)) => {
                let Some(common) = br.response else {
                    return self.fail("bad verdict", Some("missing common response"));
                };
                if has_unsupported_common_fields(&common) {
                    return self.fail("bad verdict", Some("unsupported common response fields"));
                }
                let status = ResponseStatus::try_from(common.status).ok();
                // CONTINUE_AND_REPLACE mid-stream ("replace and stop sending
                // chunks") is not implemented; never silently downgrade it to
                // a plain CONTINUE — reject per the failure policy.
                if status == Some(ResponseStatus::ContinueAndReplace) {
                    return self.fail("bad verdict", Some("replace unsupported in streamed"));
                }
                if status != Some(ResponseStatus::Continue) {
                    return self.fail("bad verdict", Some("invalid response status"));
                }
                // Header mutations in a body verdict cannot apply: the
                // upstream request headers are already committed by the time
                // body chunks flow.
                if common.header_mutation.is_some() {
                    return self.fail("bad verdict", Some("header mutation after commit"));
                }
                if common
                    .body_mutation
                    .as_ref()
                    .is_some_and(|mutation| mutation.mutation.is_none())
                {
                    return self.fail("bad verdict", Some("malformed body mutation"));
                }
                if let Some(mutation) = common.body_mutation {
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
                ChunkVerdict::Continue
            }
            Some(processing_response::Response::ImmediateResponse(imm)) => {
                let action = match immediate_action(imm, self.disable_immediate_response) {
                    Ok(action) => action,
                    Err(detail) => return self.fail("bad verdict", Some(detail)),
                };
                if matches!(action, ImmediateAction::Suppress) {
                    self.done = true;
                    exchange.terminal("suppressed", "immediate-disabled");
                    return ChunkVerdict::Continue;
                }
                let ImmediateAction::Reply(imm) = action else {
                    unreachable!("suppression returned above")
                };
                self.done = true;
                exchange.terminal("immediate-response", "request-immediate");
                // Carry the immediate response's header mutations into the
                // local reply, matching the request-stage `immediate_reply`
                // semantics. Protected/pseudo headers are rejected here (with
                // the same log lines as the mutation module) so the hook can
                // apply the pairs verbatim.
                let header_ops = imm.headers.as_ref().map_or_else(Vec::new, |mutation| {
                    self.has_log |= mutation
                        .set_headers
                        .iter()
                        .filter_map(|option| option.header.as_ref())
                        .any(|header| super::mutation::is_protected_header(&header.key))
                        || mutation
                            .remove_headers
                            .iter()
                            .any(|name| super::mutation::is_protected_header(name));
                    super::mutation::local_reply_ops(mutation, &mut self.log)
                });
                ChunkVerdict::Terminate {
                    status: imm.status,
                    grpc_status: imm.grpc_status,
                    body: imm.body,
                    header_ops,
                }
            }
            _ => self.fail("bad verdict", None),
        }
    }

    fn take_log(&mut self) -> Option<PluginLog> {
        self.has_log.then(|| {
            self.has_log = false;
            std::mem::replace(&mut self.log, PluginLog::new("ExtProc"))
        })
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
            mode_override: None,
        }
    }

    fn handler_with(
        script: Vec<Result<ProcessingResponse, tonic::Status>>,
        failure_mode_allow: bool,
    ) -> ExtProcStreamedBodyHandler {
        ExtProcStreamedBodyHandler::new(
            Box::new(ScriptedExchange::new(script)),
            failure_mode_allow,
            false,
            None,
            true,
        )
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
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Continue
        ));
        assert_eq!(chunk.as_deref(), Some(&b"mutated"[..]));
    }

    #[tokio::test]
    async fn request_body_phase_ignores_mode_override_even_when_enum_is_unknown() {
        let mut verdict = body_verdict(None);
        verdict.mode_override = Some(edgion_common::ext_proc::proto::ProcessingMode {
            request_body_mode: 99,
            ..Default::default()
        });
        let mut handler = handler_with(vec![Ok(verdict)], false);
        let mut chunk = Some(Bytes::from_static(b"original"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Continue
        ));
        assert_eq!(chunk.as_deref(), Some(&b"original"[..]));
    }

    #[tokio::test]
    async fn clear_body_empties_current_chunk() {
        let mut handler = handler_with(
            vec![Ok(body_verdict(Some(body_mutation::Mutation::ClearBody(true))))],
            false,
        );
        let mut chunk = Some(Bytes::from_static(b"secret"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Continue
        ));
        assert_eq!(chunk.as_deref(), Some(&b""[..]));
    }

    #[tokio::test]
    async fn malformed_immediate_response_fails_before_using_body() {
        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(ImmediateResponse {
                status: Some(HttpStatus { code: 100 }),
                body: b"denied".to_vec(),
                ..Default::default()
            })),
            mode_override: None,
        };
        let mut handler = handler_with(vec![Ok(immediate)], false);
        let mut chunk = Some(Bytes::from_static(b"payload"));
        match handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await {
            ChunkVerdict::Terminate {
                status,
                body,
                header_ops,
                ..
            } => {
                assert_eq!(status, 500, "1xx is not a valid final reply");
                assert_eq!(body, None);
                assert!(header_ops.is_empty());
            }
            ChunkVerdict::Continue => panic!("immediate response must terminate"),
        }
        assert!(handler.take_log().is_none());
    }

    #[tokio::test]
    async fn disabled_immediate_response_deactivates_and_preserves_chunk() {
        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(
                ImmediateResponse::default(),
            )),
            mode_override: None,
        };
        let mut handler = ExtProcStreamedBodyHandler::new(
            Box::new(ScriptedExchange::new(vec![Ok(immediate)])),
            false,
            true,
            None,
            true,
        );
        let mut chunk = Some(Bytes::from_static(b"original"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Continue
        ));
        assert_eq!(chunk.as_deref(), Some(&b"original"[..]));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Continue
        ));
    }

    #[tokio::test]
    async fn valid_immediate_carries_exact_grpc_status() {
        use edgion_common::ext_proc::proto::GrpcStatus;

        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(ImmediateResponse {
                status: Some(HttpStatus { code: 403 }),
                grpc_status: Some(GrpcStatus { status: 0 }),
                body: b"exact".to_vec(),
                ..Default::default()
            })),
            mode_override: None,
        };
        let mut handler = handler_with(vec![Ok(immediate)], false);
        let mut chunk = Some(Bytes::from_static(b"payload"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Terminate {
                status: 403,
                grpc_status: Some(0),
                ..
            }
        ));
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
            mode_override: None,
        };
        let mut handler = handler_with(vec![Ok(immediate)], false);
        let mut chunk = Some(Bytes::from_static(b"payload"));
        match handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await {
            ChunkVerdict::Terminate { status, header_ops, .. } => {
                assert_eq!(status, 403);
                assert!(matches!(
                    header_ops.as_slice(),
                    [crate::PendingHeaderOp::Set { name, value }]
                        if name == "x-denied-by" && value == "policy"
                ));
            }
            ChunkVerdict::Continue => panic!("immediate response must terminate"),
        }
        assert!(handler
            .take_log()
            .is_some_and(|log| log.contains("protected header mutation rejected")));
    }

    #[tokio::test]
    async fn transport_error_fail_close_terminates_500() {
        let mut handler = handler_with(vec![Err(tonic::Status::unavailable("down"))], false);
        let mut chunk = Some(Bytes::from_static(b"x"));
        match handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await {
            ChunkVerdict::Terminate { status, body, .. } => {
                assert_eq!(status, 500);
                assert!(body.is_none());
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
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Continue
        ));
        assert_eq!(chunk.as_deref(), Some(&b"x"[..]), "fail-open must not drop the chunk");
        let outcome = handler.take_pending_outcome().expect("fail-open records a dye outcome");
        assert!(matches!(outcome.0, DyeOutcome::FailOpen));
        let mut next = Some(Bytes::from_static(b"y"));
        assert!(matches!(
            handler.on_chunk(&mut next, RequestBodyEvent::Complete).await,
            ChunkVerdict::Continue
        ));
        assert!(handler.take_pending_outcome().is_none(), "outcome drains once");
        assert!(handler.take_log().is_none());
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
            mode_override: None,
        };
        let mut handler = handler_with(vec![Ok(cr)], false);
        let mut chunk = Some(Bytes::from_static(b"x"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Terminate { status: 500, .. }
        ));
        assert_eq!(
            chunk.as_deref(),
            Some(&b"x"[..]),
            "the rejected CR mutation must not have been applied"
        );
    }

    #[tokio::test]
    async fn streamed_body_rejects_unimplemented_common_fields_before_mutation() {
        let response = ProcessingResponse {
            response: Some(processing_response::Response::RequestBody(BodyResponse {
                response: Some(CommonResponse {
                    clear_route_cache: true,
                    body_mutation: Some(BodyMutation {
                        mutation: Some(body_mutation::Mutation::Body(b"changed".to_vec())),
                    }),
                    ..Default::default()
                }),
            })),
            mode_override: None,
        };
        let mut handler = handler_with(vec![Ok(response)], false);
        let mut chunk = Some(Bytes::from_static(b"original"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Terminate { status: 500, .. }
        ));
        assert_eq!(chunk.as_deref(), Some(&b"original"[..]));
    }

    #[tokio::test]
    async fn wrong_phase_verdict_follows_failure_policy() {
        use edgion_common::ext_proc::proto::HeadersResponse;
        let wrong = ProcessingResponse {
            response: Some(processing_response::Response::ResponseHeaders(
                HeadersResponse::default(),
            )),
            mode_override: None,
        };
        let mut handler = handler_with(vec![Ok(wrong)], false);
        let mut chunk = Some(Bytes::from_static(b"x"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Terminate { status: 500, .. }
        ));
    }

    #[tokio::test]
    async fn missing_unknown_and_malformed_streamed_body_verdicts_fail_closed() {
        let verdicts = [
            ProcessingResponse {
                response: Some(processing_response::Response::RequestBody(BodyResponse {
                    response: None,
                })),
                mode_override: None,
            },
            ProcessingResponse {
                response: Some(processing_response::Response::RequestBody(BodyResponse {
                    response: Some(CommonResponse {
                        status: 99,
                        ..Default::default()
                    }),
                })),
                mode_override: None,
            },
            ProcessingResponse {
                response: Some(processing_response::Response::RequestBody(BodyResponse {
                    response: Some(CommonResponse {
                        body_mutation: Some(BodyMutation { mutation: None }),
                        ..Default::default()
                    }),
                })),
                mode_override: None,
            },
        ];
        for verdict in verdicts {
            let mut handler = handler_with(vec![Ok(verdict)], false);
            let mut chunk = Some(Bytes::from_static(b"original"));
            assert!(matches!(
                handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
                ChunkVerdict::Terminate { status: 500, .. }
            ));
            assert_eq!(chunk.as_deref(), Some(&b"original"[..]));
        }
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

        let mut handler = ExtProcStreamedBodyHandler::new(Box::new(AssertingExchange), false, false, None, true);
        let mut chunk = None;
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Complete).await,
            ChunkVerdict::Continue
        ));
        assert!(
            handler.take_log().is_none(),
            "attached owner owns the only aggregate log"
        );
    }

    #[tokio::test]
    async fn abandoned_event_cancels_without_sending_clean_eos() {
        let mut handler = handler_with(Vec::new(), false);
        let mut chunk = None;

        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Abandoned).await,
            ChunkVerdict::Continue
        ));
        assert!(handler.exchange.is_none(), "the processor stream must be dropped");
        assert!(handler.take_log().is_none(), "attached owner owns abandonment logging");

        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Complete).await,
            ChunkVerdict::Continue
        ));
    }

    #[tokio::test]
    async fn take_log_is_none_without_entries() {
        let mut handler = handler_with(vec![Ok(body_verdict(None))], false);
        let mut chunk = Some(Bytes::from_static(b"x"));
        assert!(matches!(
            handler.on_chunk(&mut chunk, RequestBodyEvent::Data).await,
            ChunkVerdict::Continue
        ));
        assert!(handler.take_log().is_none(), "mid-stream success logs nothing");
    }
}
