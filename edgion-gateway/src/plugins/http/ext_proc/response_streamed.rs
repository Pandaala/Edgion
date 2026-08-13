//! Bounded lockstep response-body processing for attached ExtProc sessions.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use pingora_proxy::ResponseBodySink;

use super::attached::AttachedExtProc;
use super::plugin::exchange_failure_reason;
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::plugins::runtime::traits::StreamingResponseProcessor;
use crate::plugins::runtime::{DyeOutcome, PluginSession};
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
    done: bool,
    dye_plan: Option<Arc<CompiledDyePlan>>,
}

impl ExtProcResponseProcessor {
    pub(super) fn new(
        owner: Arc<AttachedExtProc>,
        failure_mode_allow: bool,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            owner,
            failure_mode_allow,
            done: false,
            dye_plan,
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
        session.install_dye_plan(self.dye_plan.clone());
        if !self.failure_mode_allow {
            if !session.try_claim_response_terminal(crate::ai::terminal::ResponseTerminalCause::ProcessorFailure) {
                self.owner.terminal("superseded", "terminal-lost");
                return;
            }
            self.owner.terminal("fail-close", reason);
            session.record_outcome(DyeOutcome::FailClose, reason);
            *chunk = None;
            session.mark_response_terminal_emission(crate::ai::terminal::TerminalEmission::NoEvent);
            sink.terminate();
        } else {
            self.owner.terminal("fail-open", reason);
            session.record_outcome(DyeOutcome::FailOpen, reason);
        }
    }
}

#[async_trait]
impl StreamingResponseProcessor for ExtProcResponseProcessor {
    fn name(&self) -> &str {
        "ExtProc"
    }

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

    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        if self.done {
            return;
        }
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
        let response = match self.owner.exchange(request, end_of_stream).await {
            Ok(response) => response,
            Err(status) => {
                let _reason = exchange_failure_reason(&status);
                self.fail(chunk, sink, session, "dependency");
                return;
            }
        };
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
            session.install_dye_plan(self.dye_plan.clone());
            session.record_outcome(DyeOutcome::Success, "response-eos");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_common::ext_proc::proto::{
        BodyMutation, BodyResponse, CommonResponse, HeaderMutation, ImmediateResponse,
    };
    use std::sync::Mutex;

    struct Scripted {
        results: Mutex<Vec<Result<ProcessingResponse, tonic::Status>>>,
        terminal: Mutex<Option<(&'static str, &'static str)>>,
    }

    #[async_trait]
    impl ResponseExchange for Scripted {
        async fn exchange(
            &self,
            _request: ProcessingRequest,
            _last: bool,
        ) -> Result<ProcessingResponse, tonic::Status> {
            self.results.lock().unwrap().remove(0)
        }

        fn terminal(&self, outcome: &'static str, reason: &'static str) {
            *self.terminal.lock().unwrap() = Some((outcome, reason));
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
            terminal: Mutex::new(None),
        });
        (
            ExtProcResponseProcessor {
                owner: exchange.clone(),
                failure_mode_allow: fail_open,
                done: false,
                dye_plan: None,
            },
            exchange,
        )
    }

    fn failure_session(outcome: DyeOutcome) -> MockPluginSession {
        let mut session = mock_session();
        session.expect_install_dye_plan().times(1).return_const(());
        session
            .expect_record_outcome()
            .withf(move |actual, _| std::mem::discriminant(actual) == std::mem::discriminant(&outcome))
            .times(1)
            .return_const(());
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
        let mut session = failure_session(DyeOutcome::FailClose);
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
        let mut session = failure_session(DyeOutcome::FailClose);
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
        let mut session = failure_session(DyeOutcome::FailClose);
        bad_headers
            .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert!(sink.is_terminated());
        assert!(chunk.is_none());

        let immediate = ProcessingResponse {
            response: Some(processing_response::Response::ImmediateResponse(
                ImmediateResponse::default(),
            )),
        };
        let (mut fail_open, terminal) = processor(Ok(immediate), true);
        let mut original = Some(Bytes::from_static(b"original"));
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session(DyeOutcome::FailOpen);
        fail_open
            .on_chunk(&mut original, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert_eq!(original.as_deref(), Some(&b"original"[..]));
        assert!(!sink.is_terminated());
        assert_eq!(
            *terminal.terminal.lock().unwrap(),
            Some(("fail-open", "post-commit-immediate"))
        );
    }

    #[tokio::test]
    async fn missing_and_wrong_response_body_verdicts_fail_closed() {
        use edgion_common::ext_proc::proto::HeadersResponse;

        let verdicts = [
            ProcessingResponse {
                response: Some(processing_response::Response::ResponseBody(BodyResponse {
                    response: None,
                })),
            },
            ProcessingResponse {
                response: Some(processing_response::Response::ResponseHeaders(
                    HeadersResponse::default(),
                )),
            },
        ];
        for verdict in verdicts {
            let (mut processor, _) = processor(Ok(verdict), false);
            let mut chunk = Some(Bytes::from_static(b"unreleased"));
            let mut sink = ResponseBodySink::new();
            let mut session = failure_session(DyeOutcome::FailClose);
            processor
                .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
                .await;
            assert!(sink.is_terminated());
            assert!(chunk.is_none());
        }
    }

    #[tokio::test]
    async fn disconnect_fail_open_preserves_window_and_eos_records_success() {
        let (mut disconnected, _) = processor(Err(tonic::Status::unavailable("private detail")), true);
        let mut chunk = Some(Bytes::from_static(b"original"));
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session(DyeOutcome::FailOpen);
        disconnected
            .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert_eq!(chunk.as_deref(), Some(&b"original"[..]));

        let (mut eos, _) = processor(Ok(verdict(None, false)), false);
        let mut empty = None;
        let mut sink = ResponseBodySink::new();
        let mut session = failure_session(DyeOutcome::Success);
        eos.on_chunk(&mut empty, true, ResponseStreamKind::None, &mut sink, &mut session)
            .await;
        assert!(!sink.is_terminated());
    }
}
