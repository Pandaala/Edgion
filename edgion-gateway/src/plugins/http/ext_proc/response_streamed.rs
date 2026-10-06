//! Bounded lockstep response-body processing for attached ExtProc sessions.
//! Response headers are committed here; terminal diagnostics never enqueue dye headers.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use pingora_proxy::ResponseBodySink;

use super::attached::AttachedExtProc;
use super::plugin::{exchange_failure_reason, has_unsupported_common_fields};
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::{StreamingResponseEvent, StreamingResponseProcessor};
use crate::plugins::runtime::PluginSession;
use edgion_common::ext_proc::proto::{
    body_mutation, common_response::ResponseStatus, processing_request, processing_response, HttpBody,
    ProcessingRequest, ProcessingResponse,
};

/// WP1 consumes bounded Pingora transport chunks. A future canonical raw-byte
/// hold/release adapter from `crate::sse` must feed this processor before any
/// semantic SSE-window claim; this type never treats a transport chunk as an
/// SSE event and never parses or reserializes SSE.
pub(super) const RESPONSE_WINDOW_CAP: usize = 1024 * 1024;

#[async_trait]
trait ResponseExchange: Send + Sync {
    async fn exchange(&self, request: ProcessingRequest, last: bool) -> Result<ProcessingResponse, tonic::Status>;
    fn terminal(&self, outcome: &'static str, reason: &'static str);
    fn release_inflight(&self) {}
}

#[async_trait]
impl ResponseExchange for AttachedExtProc {
    async fn exchange(&self, request: ProcessingRequest, last: bool) -> Result<ProcessingResponse, tonic::Status> {
        AttachedExtProc::exchange(self, request, last).await
    }

    fn terminal(&self, outcome: &'static str, reason: &'static str) {
        self.mark_terminal(outcome, reason);
    }

    fn release_inflight(&self) {
        AttachedExtProc::release_inflight(self);
    }
}

fn within_cap(parts: &[usize], cap: usize) -> bool {
    parts
        .iter()
        .try_fold(0usize, |total, part| total.checked_add(*part))
        .is_some_and(|total| total <= cap)
}

pub(super) struct ExtProcResponseProcessor {
    owner: Arc<dyn ResponseExchange>,
    failure_mode_allow: bool,
    disable_immediate_response: bool,
    response_trailers_send: bool,
    done: bool,
}

impl ExtProcResponseProcessor {
    pub(super) fn new(
        owner: Arc<AttachedExtProc>,
        failure_mode_allow: bool,
        disable_immediate_response: bool,
        response_trailers_send: bool,
    ) -> Self {
        Self {
            owner,
            failure_mode_allow,
            disable_immediate_response,
            response_trailers_send,
            done: false,
        }
    }

    fn fail(
        &mut self,
        chunk: &mut Option<Bytes>,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
        reason: &'static str,
    ) {
        self.done = true;
        if !self.failure_mode_allow {
            if !session.try_claim_response_terminal(crate::response::terminal::ResponseTerminalCause::ProcessorFailure)
            {
                self.owner.terminal("superseded", "terminal-lost");
                return;
            }
            self.owner.terminal("fail-close", reason);
            *chunk = None;
            session.mark_response_terminal_emission(crate::response::terminal::TerminalEmission::NoEvent);
            sink.terminate();
        } else {
            self.owner.terminal("fail-open", reason);
        }
    }
}

#[async_trait]
impl StreamingResponseProcessor for ExtProcResponseProcessor {
    fn changes_body_length(&self) -> bool {
        true
    }

    fn on_terminal_preempted(&mut self, winner: crate::response::terminal::ResponseTerminalCause) {
        self.done = true;
        self.owner.terminal("superseded", winner.as_str());
    }

    fn release_inflight(&mut self) {
        self.done = true;
        self.owner.release_inflight();
    }

    async fn on_event(
        &mut self,
        chunk: &mut Option<Bytes>,
        event: StreamingResponseEvent,
        _stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        if self.done {
            return;
        }
        let (end_of_stream, last) = match event {
            StreamingResponseEvent::Data { end_of_stream } => (end_of_stream, end_of_stream),
            StreamingResponseEvent::TerminalWithoutTrailers => (true, true),
            StreamingResponseEvent::TerminalBeforeTrailers if self.response_trailers_send => {
                // Completion belongs to the following real trailer exchange.
                // In particular, do not fabricate an empty body verdict here.
                self.done = true;
                return;
            }
            StreamingResponseEvent::TerminalBeforeTrailers => (true, true),
        };
        let input_len = chunk.as_ref().map_or(0, Bytes::len);
        if !within_cap(&[input_len], RESPONSE_WINDOW_CAP) {
            self.fail(chunk, sink, session, "input-cap");
            return;
        }
        let request = ProcessingRequest {
            request: Some(processing_request::Request::ResponseBody(HttpBody {
                body: chunk.as_ref().map(|bytes| bytes.to_vec()).unwrap_or_default(),
                end_of_stream,
            })),
        };
        let response = match self.owner.exchange(request, last).await {
            Ok(response) => response,
            Err(status) => {
                let _reason = exchange_failure_reason(&status);
                self.fail(chunk, sink, session, "dependency");
                return;
            }
        };
        if matches!(
            response.response,
            Some(processing_response::Response::ImmediateResponse(_))
        ) && self.disable_immediate_response
        {
            self.done = true;
            self.owner.terminal("suppressed", "immediate-disabled");
            return;
        }
        let Some(processing_response::Response::ResponseBody(response)) = response.response else {
            let reason = if matches!(
                response.response,
                Some(processing_response::Response::ImmediateResponse(_))
            ) {
                "post-commit-immediate"
            } else {
                "wrong-verdict"
            };
            self.fail(chunk, sink, session, reason);
            return;
        };
        let Some(common) = response.response else {
            self.fail(chunk, sink, session, "missing-verdict");
            return;
        };
        if has_unsupported_common_fields(&common) {
            self.fail(chunk, sink, session, "invalid-verdict");
            return;
        }
        if common.header_mutation.is_some()
            || ResponseStatus::try_from(common.status).ok() != Some(ResponseStatus::Continue)
        {
            self.fail(chunk, sink, session, "invalid-verdict");
            return;
        }
        if let Some(mutation) = common.body_mutation.and_then(|mutation| mutation.mutation) {
            match mutation {
                body_mutation::Mutation::Body(body) => {
                    if !within_cap(&[body.len()], RESPONSE_WINDOW_CAP) {
                        self.fail(chunk, sink, session, "output-cap");
                        return;
                    }
                    *chunk = Some(Bytes::from(body));
                }
                body_mutation::Mutation::ClearBody(true) => *chunk = None,
                body_mutation::Mutation::ClearBody(false) => {}
            }
        }
        if end_of_stream {
            self.done = true;
        }
    }

    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        self.on_event(
            chunk,
            StreamingResponseEvent::Data { end_of_stream },
            stream_kind,
            sink,
            session,
        )
        .await;
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/ext_proc/response_streamed_test.rs"]
mod tests;
