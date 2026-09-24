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
#[path = "../../../../tests/unit/plugins/http/ext_proc/streamed_test.rs"]
mod tests;
