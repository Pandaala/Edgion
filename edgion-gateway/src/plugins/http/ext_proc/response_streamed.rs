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
            if !session.try_claim_response_terminal(crate::ai::terminal::ResponseTerminalCause::ProcessorFailure) {
                self.owner.terminal("superseded", "terminal-lost");
                return;
            }
            self.owner.terminal("fail-close", reason);
            *chunk = None;
            session.mark_response_terminal_emission(crate::ai::terminal::TerminalEmission::NoEvent);
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

    fn on_terminal_preempted(&mut self, winner: crate::ai::terminal::ResponseTerminalCause) {
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_common::ext_proc::proto::{
        BodyMutation, BodyResponse, CommonResponse, HeaderMutation, ImmediateResponse,
    };
    use parking_lot::Mutex;

    struct Scripted {
        results: Mutex<Vec<Result<ProcessingResponse, tonic::Status>>>,
        requests: Mutex<Vec<(ProcessingRequest, bool)>>,
        terminal: Mutex<Option<(&'static str, &'static str)>>,
    }

    #[async_trait]
    impl ResponseExchange for Scripted {
        async fn exchange(&self, request: ProcessingRequest, last: bool) -> Result<ProcessingResponse, tonic::Status> {
            self.requests.lock().push((request, last));
            self.results.lock().remove(0)
        }

        fn terminal(&self, outcome: &'static str, reason: &'static str) {
            *self.terminal.lock() = Some((outcome, reason));
        }
    }

    fn verdict(mutation: Option<body_mutation::Mutation>, headers: bool) -> ProcessingResponse {
        ProcessingResponse {
            response: Some(processing_response::Response::ResponseBody(BodyResponse {
                response: Some(CommonResponse {
                    status: ResponseStatus::Continue as i32,
                    header_mutation: headers.then(HeaderMutation::default),
                    body_mutation: mutation.map(|mutation| BodyMutation {
                        mutation: Some(mutation),
                    }),
                    ..Default::default()
                }),
            })),
            mode_override: None,
        }
    }

    fn mock_session() -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_try_claim_response_terminal().return_const(true);
        session.expect_mark_response_terminal_emission().return_const(());
        session
    }

    fn processor(
        result: Result<ProcessingResponse, tonic::Status>,
        fail_open: bool,
    ) -> (ExtProcResponseProcessor, Arc<Scripted>) {
        let exchange = Arc::new(Scripted {
            results: Mutex::new(vec![result]),
            requests: Mutex::new(Vec::new()),
            terminal: Mutex::new(None),
        });
        (
            ExtProcResponseProcessor {
                owner: exchange.clone(),
                failure_mode_allow: fail_open,
                disable_immediate_response: false,
                response_trailers_send: false,
                done: false,
            },
            exchange,
        )
    }

    #[tokio::test]
    async fn typed_pre_trailer_boundary_obeys_exact_terminal_table() {
        let (mut send_trailers, owner) = processor(Ok(verdict(None, false)), false);
        send_trailers.response_trailers_send = true;
        let mut chunk = None;
        let mut sink = ResponseBodySink::new();
        let mut session = mock_session();
        send_trailers
            .on_event(
                &mut chunk,
                StreamingResponseEvent::TerminalBeforeTrailers,
                ResponseStreamKind::None,
                &mut sink,
                &mut session,
            )
            .await;
        assert!(owner.requests.lock().is_empty());

        let (mut skip_trailers, owner) = processor(Ok(verdict(None, false)), false);
        let mut session = mock_session();
        session.expect_install_dye_plan().times(0).return_const(());
        session.expect_record_outcome().times(0).return_const(());
        skip_trailers
            .on_event(
                &mut chunk,
                StreamingResponseEvent::TerminalBeforeTrailers,
                ResponseStreamKind::None,
                &mut sink,
                &mut session,
            )
            .await;
        let requests = owner.requests.lock();
        assert!(requests[0].1);
        let Some(processing_request::Request::ResponseBody(body)) = requests[0].0.request.as_ref() else {
            panic!("expected terminal response body")
        };
        assert!(body.body.is_empty());
        assert!(body.end_of_stream);
    }

    #[tokio::test]
    async fn trailer_free_terminal_sends_zero_body_last() {
        let (mut processor, owner) = processor(Ok(verdict(None, false)), false);
        let mut chunk = None;
        let mut sink = ResponseBodySink::new();
        let mut session = mock_session();
        session.expect_install_dye_plan().times(0).return_const(());
        session.expect_record_outcome().times(0).return_const(());
        processor
            .on_event(
                &mut chunk,
                StreamingResponseEvent::TerminalWithoutTrailers,
                ResponseStreamKind::None,
                &mut sink,
                &mut session,
            )
            .await;
        let requests = owner.requests.lock();
        assert!(requests[0].1);
        let Some(processing_request::Request::ResponseBody(body)) = requests[0].0.request.as_ref() else {
            panic!("expected terminal response body")
        };
        assert!(body.body.is_empty());
        assert!(body.end_of_stream);
    }

    fn failure_session() -> MockPluginSession {
        let mut session = mock_session();
        // Headers are committed: no response dye or retained dye plan is allowed.
        session.expect_install_dye_plan().times(0);
        session.expect_record_outcome().times(0);
        session
    }

    #[test]
    fn inclusive_window_cap_uses_checked_arithmetic() {
        assert!(within_cap(&[RESPONSE_WINDOW_CAP], RESPONSE_WINDOW_CAP));
        assert!(!within_cap(&[RESPONSE_WINDOW_CAP, 1], RESPONSE_WINDOW_CAP));
        assert!(!within_cap(&[usize::MAX, 1], usize::MAX));
    }

    #[tokio::test]
    async fn pass_replace_and_clear_preserve_lockstep_window_contract() {
        for (mutation, expected) in [
            (None, Some(&b"original"[..])),
            (
                Some(body_mutation::Mutation::Body(b"replacement".to_vec())),
                Some(&b"replacement"[..]),
            ),
            (Some(body_mutation::Mutation::ClearBody(true)), None),
            (Some(body_mutation::Mutation::ClearBody(false)), Some(&b"original"[..])),
        ] {
            let (mut processor, _) = processor(Ok(verdict(mutation, false)), false);
            let mut chunk = Some(Bytes::from_static(b"original"));
            let mut sink = ResponseBodySink::new();
            let mut session = mock_session();
            processor
                .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
                .await;
            assert_eq!(chunk.as_deref(), expected);
            assert!(!sink.is_terminated());
        }
    }

    #[tokio::test]
    async fn response_body_phase_ignores_mode_override_even_when_enum_is_unknown() {
        let mut response = verdict(None, false);
        response.mode_override = Some(edgion_common::ext_proc::proto::ProcessingMode {
            response_body_mode: 99,
            ..Default::default()
        });
        let (mut processor, _) = processor(Ok(response), false);
        let mut chunk = Some(Bytes::from_static(b"original"));
        let mut sink = ResponseBodySink::new();
        let mut session = mock_session();
        processor
            .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert_eq!(chunk.as_deref(), Some(&b"original"[..]));
        assert!(!sink.is_terminated());
    }

    #[tokio::test]
    async fn exact_cap_succeeds_and_cap_plus_one_fails_closed() {
        let (mut exact, _) = processor(Ok(verdict(None, false)), false);
        let mut exact_chunk = Some(Bytes::from(vec![b'x'; RESPONSE_WINDOW_CAP]));
        let mut sink = ResponseBodySink::new();
        let mut session = mock_session();
        exact
            .on_chunk(
                &mut exact_chunk,
                false,
                ResponseStreamKind::None,
                &mut sink,
                &mut session,
            )
            .await;
        assert!(!sink.is_terminated());

        let (mut oversized, _) = processor(Ok(verdict(None, false)), false);
        let mut chunk = Some(Bytes::from(vec![b'x'; RESPONSE_WINDOW_CAP + 1]));
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session();
        oversized
            .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert!(sink.is_terminated());
        assert!(chunk.is_none());

        let (mut exact_replacement, _) = processor(
            Ok(verdict(
                Some(body_mutation::Mutation::Body(vec![b'y'; RESPONSE_WINDOW_CAP])),
                false,
            )),
            false,
        );
        let mut input = Some(Bytes::from_static(b"x"));
        let mut sink = ResponseBodySink::new();
        let mut session = mock_session();
        exact_replacement
            .on_chunk(&mut input, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert_eq!(input.as_ref().map(Bytes::len), Some(RESPONSE_WINDOW_CAP));

        let (mut oversized_replacement, _) = processor(
            Ok(verdict(
                Some(body_mutation::Mutation::Body(vec![b'y'; RESPONSE_WINDOW_CAP + 1])),
                false,
            )),
            false,
        );
        let mut input = Some(Bytes::from_static(b"x"));
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session();
        oversized_replacement
            .on_chunk(&mut input, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert!(sink.is_terminated());
        assert!(input.is_none());
    }

    #[tokio::test]
    async fn header_mutation_and_immediate_follow_post_commit_policy() {
        let (mut bad_headers, _) = processor(Ok(verdict(None, true)), false);
        let mut chunk = Some(Bytes::from_static(b"secret"));
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session();
        bad_headers
            .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert!(sink.is_terminated());
        assert!(chunk.is_none());

        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(
                ImmediateResponse::default(),
            )),
            mode_override: None,
        };
        let (mut fail_open, terminal) = processor(Ok(immediate), true);
        let mut original = Some(Bytes::from_static(b"original"));
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session();
        fail_open
            .on_chunk(&mut original, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert_eq!(original.as_deref(), Some(&b"original"[..]));
        assert!(!sink.is_terminated());
        assert_eq!(*terminal.terminal.lock(), Some(("fail-open", "post-commit-immediate")));
    }

    #[tokio::test]
    async fn disabled_post_commit_immediate_preserves_body_without_failure_policy() {
        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(
                ImmediateResponse::default(),
            )),
            mode_override: None,
        };
        let (mut processor, terminal) = processor(Ok(immediate), false);
        processor.disable_immediate_response = true;
        let mut original = Some(Bytes::from_static(b"original"));
        let mut sink = ResponseBodySink::new();
        let mut session = mock_session();
        processor
            .on_chunk(&mut original, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert_eq!(original.as_deref(), Some(&b"original"[..]));
        assert!(!sink.is_terminated());
        assert_eq!(*terminal.terminal.lock(), Some(("suppressed", "immediate-disabled")));
    }

    #[tokio::test]
    async fn missing_and_wrong_response_body_verdicts_fail_closed() {
        use edgion_common::ext_proc::proto::HeadersResponse;

        let verdicts = [
            ProcessingResponse {
                response: Some(processing_response::Response::ResponseBody(BodyResponse {
                    response: None,
                })),
                mode_override: None,
            },
            ProcessingResponse {
                response: Some(processing_response::Response::ResponseHeaders(
                    HeadersResponse::default(),
                )),
                mode_override: None,
            },
        ];
        for verdict in verdicts {
            let (mut processor, _) = processor(Ok(verdict), false);
            let mut chunk = Some(Bytes::from_static(b"unreleased"));
            let mut sink = ResponseBodySink::new();
            let mut session = failure_session();
            processor
                .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
                .await;
            assert!(sink.is_terminated());
            assert!(chunk.is_none());
        }
    }

    #[tokio::test]
    async fn response_body_rejects_unimplemented_common_fields_before_mutation() {
        let response = ProcessingResponse {
            response: Some(processing_response::Response::ResponseBody(BodyResponse {
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
        let (mut processor, _) = processor(Ok(response), false);
        let mut chunk = Some(Bytes::from_static(b"original"));
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session();
        processor
            .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert!(sink.is_terminated());
        assert!(chunk.is_none());
    }

    #[tokio::test]
    async fn disconnect_fail_open_and_eos_never_attempt_response_header_dye() {
        let (mut disconnected, _) = processor(Err(tonic::Status::unavailable("private detail")), true);
        let mut chunk = Some(Bytes::from_static(b"original"));
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session();
        disconnected
            .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert_eq!(chunk.as_deref(), Some(&b"original"[..]));

        let (mut eos, owner) = processor(Ok(verdict(None, false)), false);
        let mut empty = None;
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session();
        eos.on_chunk(&mut empty, true, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert!(!sink.is_terminated());
        assert!(owner.requests.lock()[0].1);
    }
}
