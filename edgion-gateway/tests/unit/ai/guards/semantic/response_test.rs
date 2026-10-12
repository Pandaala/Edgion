use super::*;
use crate::ai::protocols::openai::terminal::AiClientStreamDialect;
use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::response::terminal::{FinalResponseEligibility, ResponseProcessorSlot, ResponseTerminalState};
use crate::sse::window::{LogicalBucketKey, LogicalField, SemanticFragment, SemanticSource, WindowRecord};
use crate::sse::ByteRange;
use tokio::io::AsyncWriteExt;

async fn live_session() -> pingora_proxy::Session {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = pingora_proxy::Session::new_h1(stream);
    client
        .write_all(b"GET /guardrail-cancel HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    drop(client);
    session.read_request().await.unwrap();
    session
}

fn processor(fail_open: bool) -> GuardrailResponseProcessor {
    GuardrailResponseProcessor::new(
        Arc::new(ManagedOutboundClient::test_with_static_target("http://localhost:1")),
        Arc::new(Semaphore::new(1)),
        Arc::new(AttachedGuardrail::new()),
        1,
        0,
        4096,
        4096,
        fail_open,
        false,
    )
    .unwrap()
}

fn semantic_record(text: &str) -> WindowRecord {
    let raw = Bytes::from(format!(
        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{}}}}}]}}\n\n",
        serde_json::to_string(text).unwrap()
    ));
    let encoded = serde_json::to_vec(text).unwrap();
    let start = raw.windows(encoded.len()).position(|part| part == encoded).unwrap();
    WindowRecord::raw(vec![raw]).with_fragments(vec![SemanticFragment {
        bucket: LogicalBucketKey {
            choice_index: 0,
            field: LogicalField::Content,
        },
        source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(start, start + encoded.len())]),
    }])
}

fn mock_request_session() -> MockPluginSession {
    let mut session = MockPluginSession::new();
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    session.expect_get_request_method().return_const(Some("GET"));
    session.expect_get_request_path().return_const("/".to_string());
    session.expect_try_claim_response_terminal().return_const(true);
    session.expect_mark_response_terminal_emission().return_const(());
    ai.expect_ai_terminal_render_context().return_const(Some((
        crate::ai::protocols::openai::terminal::AiClientStreamDialect::OpenAiCompatibleSse,
        crate::response::terminal::FinalResponseEligibility {
            method_allows_body: true,
            status_allows_body: true,
            stream_kind: ResponseStreamKind::Sse,
        },
    )));
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
    session
}

#[derive(Default)]
struct PendingReleaseHeadSink {
    held: bool,
    remaining_work: u64,
    work_exhausted: bool,
    replaced: bool,
    failed: bool,
    terminated: bool,
}

impl HeldResponseHeadSink for PendingReleaseHeadSink {
    fn response_head_is_held(&self) -> bool {
        self.held
    }

    fn reserve_response_head_work(&mut self, units: u64) -> pingora_core::Result<()> {
        if units > self.remaining_work {
            self.work_exhausted = true;
            return pingora_core::Error::e_explain(
                pingora_core::ErrorType::InternalError,
                "test response-head work budget exhausted",
            );
        }
        self.remaining_work -= units;
        Ok(())
    }

    fn replace_response_head(
        &mut self,
        _replacement: pingora_proxy::ResponseHeadReplacement,
    ) -> pingora_core::Result<()> {
        assert!(self.held, "only a pending Hold decision may be upgraded");
        self.replaced = true;
        Ok(())
    }

    fn fail_response_head(&mut self, _error: Box<pingora_core::Error>) -> pingora_core::Result<()> {
        assert!(self.held, "only a pending Hold decision may be upgraded");
        self.failed = true;
        Ok(())
    }

    fn terminate(&mut self) {
        self.terminated = true;
    }
}

fn localhost_uri(uri: &str) -> String {
    uri.replace("127.0.0.1", "localhost")
}

#[test]
fn pass_replace_and_reject_resolve_one_generation_atomically() {
    let mut pass = processor(false);
    let record = semantic_record("original");
    let expected: Vec<u8> = record.raw_slices_cloned().into_iter().flatten().collect();
    pass.window.retain(record).unwrap();
    let (released, reject) = pass.apply_decision(ParsedDecision::Pass).unwrap();
    assert!(!reject);
    let WindowOutput::Release(slices) = released else {
        panic!("Pass must release original slices")
    };
    assert_eq!(slices.into_iter().flatten().collect::<Vec<_>>(), expected);

    let key = LogicalBucketKey {
        choice_index: 0,
        field: LogicalField::Content,
    };
    let mut replace = processor(false);
    replace.window.retain(semantic_record("original")).unwrap();
    let (rewritten, reject) = replace
        .apply_decision(ParsedDecision::Replace(vec![(key, "reviewed".to_string())]))
        .unwrap();
    assert!(!reject);
    let WindowOutput::Replace(rewritten) = rewritten else {
        panic!("Replace must emit rewritten SSE")
    };
    let rewritten = String::from_utf8(rewritten.to_vec()).unwrap();
    assert!(rewritten.contains("\"content\":\"reviewed\""));
    assert!(!rewritten.contains("original"));

    let mut reject = processor(true);
    reject.window.retain(semantic_record("blocked")).unwrap();
    let (discarded, policy_reject) = reject.apply_decision(ParsedDecision::Reject).unwrap();
    assert!(policy_reject, "policy Reject is independent of fail-open");
    assert_eq!(discarded, WindowOutput::Discard);
}

#[test]
fn borrowed_preview_matches_final_replacement_after_prior_context() {
    let key = LogicalBucketKey {
        choice_index: 0,
        field: LogicalField::Content,
    };
    let mut processor = processor(false);
    processor.window = RawWindow::with_policy(RawWindowPolicy {
        per_bucket_overlap_bytes: 16,
        global_overlap_bytes: 16,
        max_replacement_bytes: 4096,
    })
    .unwrap();
    processor.window.retain(semantic_record("prior")).unwrap();
    processor.apply_decision(ParsedDecision::Pass).unwrap();
    processor.window.start_next().unwrap();
    processor.window.retain(semantic_record("hidden")).unwrap();
    let replacements = vec![(key, "é\\\"".to_string())];
    let preview = processor
        .window
        .preview_semantic_replace(replacements.iter().map(|(key, text)| (*key, text.as_str())))
        .unwrap();
    assert_eq!(processor.window.snapshot_all().unwrap()[0].context, "prior");
    let (output, reject) = processor
        .apply_final_decision(ParsedDecision::Replace(replacements))
        .unwrap();
    let WindowOutput::Replace(bytes) = output else {
        panic!("expected semantic replacement")
    };
    assert!(!reject);
    assert_eq!(bytes.len(), preview);
    assert_eq!(bytes, semantic_record("é\\\"").into_raw_slices().concat());
    assert_eq!(processor.window.start_next(), Err(WindowError::Finished));
    assert_eq!(processor.summary.windows, 2);
}

#[test]
fn pending_release_upgrades_to_precommit_replace_or_fail() {
    let mut reject = processor(false);
    reject.first_head_window_pending = false;
    reject.owner.prepare_head_replacement(
        &pingora_http::ResponseHeader::build(200, None).unwrap(),
        &crate::request_trace_id::RequestTraceId::default(),
    );
    let mut reject_sink = PendingReleaseHeadSink {
        held: true,
        ..PendingReleaseHeadSink::default()
    };
    let mut reject_session = MockPluginSession::new();
    reject_session
        .expect_try_claim_response_terminal()
        .once()
        .return_const(true);
    reject_session
        .expect_mark_response_terminal_emission()
        .once()
        .return_const(());

    assert!(reject.stage_head_policy_reject(&mut reject_sink, &mut reject_session));
    assert!(reject_sink.replaced);
    assert!(!reject_sink.failed);
    assert!(!reject_sink.terminated);

    let mut failure = processor(false);
    failure.first_head_window_pending = false;
    let mut failure_sink = PendingReleaseHeadSink {
        held: true,
        ..PendingReleaseHeadSink::default()
    };
    let mut failure_session = MockPluginSession::new();
    failure_session
        .expect_try_claim_response_terminal()
        .once()
        .return_const(true);
    failure_session
        .expect_mark_response_terminal_emission()
        .once()
        .return_const(());

    assert!(failure.stage_head_failure(&mut failure_sink, &mut failure_session, "later-window"));
    assert!(failure_sink.failed);
    assert!(!failure_sink.replaced);
    assert!(!failure_sink.terminated);
}

#[tokio::test]
async fn pending_release_continues_to_charge_work_and_exhaustion_stages_fail() {
    let mut processor = processor(false);
    processor.first_head_window_pending = false;
    let mut sink = PendingReleaseHeadSink {
        held: true,
        remaining_work: 0,
        ..PendingReleaseHeadSink::default()
    };

    assert_eq!(
        processor.decide(&MockPluginSession::new(), &mut sink).await,
        Err("head-work-limit")
    );
    assert!(sink.work_exhausted);

    let mut session = MockPluginSession::new();
    session.expect_try_claim_response_terminal().once().return_const(true);
    session.expect_mark_response_terminal_emission().once().return_const(());
    assert!(processor.stage_head_failure(&mut sink, &mut session, "head-work-limit"));
    assert!(sink.failed);
    assert!(!sink.terminated);
}

#[test]
fn rollback_reservation_is_inclusive_and_checked() {
    let mut processor = processor(true);
    let remaining = VecDeque::from([WindowRecord::raw(vec![Bytes::from_static(b"tail")])]);
    assert!(processor.rollback_fits(MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES - 8, 4, &remaining));
    assert!(!processor.rollback_fits(MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES - 7, 4, &remaining));
    assert!(!processor.rollback_fits(usize::MAX, 1, &remaining));
    let output = GuardrailResponseProcessor::output_buffer().unwrap();
    assert_eq!(output.capacity(), MAX_GUARDRAIL_CALLBACK_ROLLBACK_BYTES);
}

#[test]
fn incomplete_finalization_preserves_counters_and_is_exactly_once() {
    let mut processor = processor(false);
    processor.summary.windows = 3;
    processor.summary.passed = 2;
    processor.finalize_incomplete("upstream-abort", "stream-error");
    processor.finalize_incomplete("cancelled", "downstream-cancel");
    let line = processor.owner.take_summary_line().unwrap();
    assert!(line.contains("windows=3"));
    assert!(line.contains("passed=2"));
    assert!(line.contains("outcome=upstream-abort"));
    assert!(processor.owner.take_summary_line().is_none());
}

#[tokio::test]
async fn non_sse_and_missing_eos_source_fail_open_byte_identically() {
    let original = Bytes::from_static(b"unclassified response bytes");
    let mut non_sse = processor(true);
    let mut chunk = Some(original.clone());
    let mut sink = ResponseBodySink::new();
    let mut session = MockPluginSession::new();
    non_sse
        .on_chunk(&mut chunk, false, ResponseStreamKind::None, &mut sink, &mut session)
        .await;
    assert_eq!(chunk, Some(original));
    assert!(!sink.is_terminated());

    let tail = Bytes::from_static(b"unterminated canonical tail");
    let mut missing_eos = processor(true);
    let mut chunk = Some(tail.clone());
    let mut sink = ResponseBodySink::new();
    let mut session = MockPluginSession::new();
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    ai.expect_take_canonical_records().once().return_once(|| None);
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
    missing_eos
        .on_chunk(&mut chunk, true, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;
    assert_eq!(chunk, Some(tail));
    assert!(!sink.is_terminated());
}

#[tokio::test]
async fn semantic_stats_failure_obeys_fail_policy_without_losing_raw_bytes() {
    let raw = Bytes::from_static(b"\xff");
    let invalid = WindowRecord::raw(vec![raw.clone()]).with_fragments(vec![SemanticFragment {
        bucket: LogicalBucketKey {
            choice_index: 0,
            field: LogicalField::Content,
        },
        source: SemanticSource::Utf8Ranges(vec![ByteRange::new(0, 1)]),
    }]);

    let mut open_batch = CanonicalRecordBatch::new();
    open_batch.push(invalid.clone()).unwrap();
    let mut open = processor(true);
    let mut chunk = None;
    let mut sink = ResponseBodySink::new();
    let mut session = MockPluginSession::new();
    open.process_batch(open_batch, false, &mut chunk, &mut sink, &mut session)
        .await;
    assert_eq!(chunk, Some(raw));
    assert!(!sink.is_terminated());

    let mut close_batch = CanonicalRecordBatch::new();
    close_batch.push(invalid).unwrap();
    let mut close = processor(false);
    let mut chunk = None;
    let mut sink = ResponseBodySink::new();
    let mut session = MockPluginSession::new();
    session.expect_try_claim_response_terminal().return_const(true);
    session.expect_mark_response_terminal_emission().return_const(());
    close
        .process_batch(close_batch, false, &mut chunk, &mut sink, &mut session)
        .await;
    assert!(chunk.is_none());
    assert!(sink.is_terminated());
}

#[tokio::test]
async fn eos_flushes_raw_tail_once_without_a_callout() {
    let first = Bytes::from_static(b": comment\n\n");
    let tail = Bytes::from_static(b"data: [DONE]\n\n");
    let mut batch = CanonicalRecordBatch::new();
    batch.push(WindowRecord::raw(vec![first.clone()])).unwrap();
    batch.push(WindowRecord::raw(vec![tail.clone()])).unwrap();
    let mut processor = processor(false);
    let mut chunk = None;
    let mut sink = ResponseBodySink::new();
    let mut session = MockPluginSession::new();
    processor
        .process_batch(batch, true, &mut chunk, &mut sink, &mut session)
        .await;
    let expected = [first.as_ref(), tail.as_ref()].concat();
    assert_eq!(chunk.as_deref(), Some(expected.as_slice()));
    assert!(!sink.is_terminated());
    let summary = processor.owner.take_summary_line().unwrap();
    assert!(summary.contains("windows=0"));
    assert!(summary.contains("outcome=pass"));
}

#[tokio::test]
async fn one_callback_can_make_multiple_serial_policy_calls() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "application/json")
                .set_body_json(serde_json::json!({"version":"v1","action":"pass"})),
        )
        .expect(2)
        .mount(&server)
        .await;
    let mut processor = processor(false);
    processor.client = Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
        &server.uri(),
    )));
    let first = semantic_record("one");
    let second = semantic_record("two");
    let expected: Vec<u8> = first
        .raw_slices_cloned()
        .into_iter()
        .chain(second.raw_slices_cloned())
        .flatten()
        .collect();
    let mut batch = CanonicalRecordBatch::new();
    batch.push(first).unwrap();
    batch.push(second).unwrap();
    let mut chunk = None;
    let mut sink = ResponseBodySink::new();
    let mut session = mock_request_session();
    processor
        .process_batch(batch, false, &mut chunk, &mut sink, &mut session)
        .await;
    assert_eq!(chunk.as_deref(), Some(expected.as_slice()));
    assert!(!sink.is_terminated());
    assert_eq!(processor.summary.windows, 2);
    assert_eq!(processor.summary.passed, 2);
}

#[tokio::test]
async fn later_reject_in_one_callback_preserves_the_already_accepted_window() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(move |_request: &wiremock::Request| {
            let action = if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                "pass"
            } else {
                "reject"
            };
            ResponseTemplate::new(200)
                .append_header("content-type", "application/json")
                .set_body_json(serde_json::json!({"version":"v1","action":action}))
        })
        .expect(2)
        .mount(&server)
        .await;

    let first = semantic_record("accepted");
    let first_raw: Vec<u8> = first.raw_slices_cloned().into_iter().flatten().collect();
    let second = semantic_record("blocked");
    let mut batch = CanonicalRecordBatch::new();
    batch.push(first).unwrap();
    batch.push(second).unwrap();

    let mut processor = processor(false);
    processor.client = Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
        &server.uri(),
    )));
    let mut chunk = None;
    let mut sink = ResponseBodySink::new();
    let mut session = mock_request_session();
    processor
        .process_batch(batch, false, &mut chunk, &mut sink, &mut session)
        .await;

    assert_eq!(chunk.as_deref(), Some(first_raw.as_slice()));
    assert!(sink.is_terminated());
    assert_eq!(sink.peek_extra().len(), 1);
    assert_eq!(processor.summary.passed, 1);
    assert_eq!(processor.summary.outcome, "reject");
    assert_eq!(processor.summary.output_bytes, first_raw.len() as u64);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn later_dependency_failure_preserves_accepted_replace_and_remaining_originals() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let responder_calls = Arc::clone(&calls);
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(move |_request: &wiremock::Request| {
            if responder_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200)
                    .append_header("content-type", "application/json")
                    .set_body_json(serde_json::json!({
                        "version":"v1",
                        "action":"replace",
                        "replacements":[{
                            "choiceIndex":0,
                            "field":{"type":"content"},
                            "current":"reviewed"
                        }]
                    }))
            } else {
                ResponseTemplate::new(200)
                    .append_header("content-type", "application/json")
                    .set_body_bytes(b"not-json".to_vec())
            }
        })
        .expect(2)
        .mount(&server)
        .await;

    let first = semantic_record("first");
    let second = semantic_record("second");
    let second_raw: Vec<u8> = second.raw_slices_cloned().into_iter().flatten().collect();
    let mut batch = CanonicalRecordBatch::new();
    batch.push(first).unwrap();
    batch.push(second).unwrap();
    let mut processor = processor(true);
    processor.client = Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
        &server.uri(),
    )));
    let mut chunk = None;
    let mut sink = ResponseBodySink::new();
    let mut session = mock_request_session();
    processor
        .process_batch(batch, false, &mut chunk, &mut sink, &mut session)
        .await;

    let output = chunk.expect("fail-open must preserve every owned byte");
    assert!(output.windows(b"reviewed".len()).any(|part| part == b"reviewed"));
    assert!(!output.windows(b"first".len()).any(|part| part == b"first"));
    assert!(output.ends_with(&second_raw));
    assert!(!sink.is_terminated());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let summary = processor.owner.take_summary_line().unwrap();
    assert!(summary.contains("outcome=fail-open"));
    assert!(summary.contains("replaced=1"));
}

#[tokio::test]
async fn policy_reject_discards_before_terminate_even_when_fail_open() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "application/json")
                .set_body_json(serde_json::json!({"version":"v1","action":"reject"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut processor = processor(true);
    processor.client = Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
        &server.uri(),
    )));
    let mut batch = CanonicalRecordBatch::new();
    batch.push(semantic_record("blocked")).unwrap();
    let mut chunk = None;
    let mut sink = ResponseBodySink::new();
    let mut session = mock_request_session();
    processor
        .process_batch(batch, false, &mut chunk, &mut sink, &mut session)
        .await;
    assert!(chunk.is_none());
    assert!(sink.is_terminated());
    assert_eq!(sink.peek_extra().len(), 1);
    assert!(sink.peek_extra()[0]
        .windows(b"content_policy_violation".len())
        .any(|part| part == b"content_policy_violation"));
    let summary = processor.owner.take_summary_line().unwrap();
    assert!(summary.contains("outcome=reject"));
    assert!(!summary.contains("outcome=fail-open"));
}

#[tokio::test]
async fn non_hold_policy_reject_claims_semantic_terminal_before_rendering() {
    let mut processor = processor(true);
    processor.window.retain(semantic_record("blocked")).unwrap();
    let (discarded, reject) = processor.apply_decision(ParsedDecision::Reject).unwrap();
    assert!(reject);
    assert_eq!(discarded, WindowOutput::Discard);

    let accepted = Bytes::from_static(b"already accepted bytes");
    let accepted_before = accepted.clone();
    let mut ctx = crate::EdgionHttpContext::default();
    let state = ctx.ensure_ai_state();
    state.client_stream_dialect = Some(AiClientStreamDialect::OpenAiCompatibleSse);
    state.final_response_eligibility = Some(FinalResponseEligibility {
        method_allows_body: true,
        status_allows_body: true,
        stream_kind: ResponseStreamKind::Sse,
    });
    let mut live = live_session().await;
    let mut sink = ResponseBodySink::new();
    {
        let mut session = PingoraSessionAdapter::new(&mut live, &mut ctx);
        session.set_response_processor_slot(ResponseProcessorSlot::Semantic);
        processor.emit_policy_reject(&mut sink, &mut session);
    }

    assert_eq!(accepted, accepted_before);
    assert_eq!(
        ctx.response_terminal_state,
        ResponseTerminalState::Claimed {
            winner: ResponseProcessorSlot::Semantic,
            cause: ResponseTerminalCause::GuardrailRejected,
            emission: TerminalEmission::Rendered,
        }
    );
    assert!(sink.is_terminated());
    assert_eq!(sink.peek_extra().len(), 1);
    assert!(sink.peek_extra()[0].len() <= 4 * 1024);
    let summary = processor.owner.take_summary_line().unwrap();
    assert!(summary.contains("outcome=reject"));
    assert!(summary.contains("reason=policy"));
    assert!(processor.owner.take_summary_line().is_none());
}

#[tokio::test]
async fn non_hold_policy_reject_preserves_an_existing_terminal_owner() {
    let mut guardrail = processor(false);
    guardrail.window.retain(semantic_record("blocked")).unwrap();
    let (discarded, reject) = guardrail.apply_decision(ParsedDecision::Reject).unwrap();
    assert!(reject);
    assert_eq!(discarded, WindowOutput::Discard);

    let accepted = Bytes::from_static(b"already accepted bytes");
    let accepted_before = accepted.clone();
    let mut ctx = crate::EdgionHttpContext::default();
    assert!(ctx
        .response_terminal_state
        .try_claim(ResponseProcessorSlot::Ai, ResponseTerminalCause::QuotaExceeded));
    ctx.response_terminal_state.mark_emission(TerminalEmission::Rendered);
    let expected_state = ctx.response_terminal_state;
    let mut live = live_session().await;
    let mut sink = ResponseBodySink::new();
    {
        let mut session = PingoraSessionAdapter::new(&mut live, &mut ctx);
        session.set_response_processor_slot(ResponseProcessorSlot::Semantic);
        guardrail.emit_policy_reject(&mut sink, &mut session);
    }

    assert_eq!(accepted, accepted_before);
    assert_eq!(ctx.response_terminal_state, expected_state);
    assert!(sink.peek_extra().is_empty());
    assert!(
        !sink.is_terminated(),
        "the Guardrail callback must not terminate a sink owned by another terminal; the enclosing driver settles it"
    );
    let summary = guardrail.owner.take_summary_line().unwrap();
    assert!(summary.contains("outcome=superseded"));
    assert!(summary.contains("reason=terminal-owner-conflict"));
    assert!(guardrail.owner.take_summary_line().is_none());

    let mut earlier = processor(false);
    earlier.publish_terminal("pass", "normal-eos");
    let mut earlier_ctx = crate::EdgionHttpContext::default();
    assert!(earlier_ctx
        .response_terminal_state
        .try_claim(ResponseProcessorSlot::Ai, ResponseTerminalCause::ProviderCompleted));
    let mut earlier_live = live_session().await;
    let mut earlier_sink = ResponseBodySink::new();
    {
        let mut session = PingoraSessionAdapter::new(&mut earlier_live, &mut earlier_ctx);
        session.set_response_processor_slot(ResponseProcessorSlot::Semantic);
        earlier.emit_policy_reject(&mut earlier_sink, &mut session);
    }
    let summary = earlier.owner.take_summary_line().unwrap();
    assert!(summary.contains("outcome=pass"));
    assert!(summary.contains("reason=normal-eos"));
    assert!(!summary.contains("superseded"));
    assert!(earlier.owner.take_summary_line().is_none());
}

#[tokio::test]
async fn permit_saturation_uses_the_same_absolute_call_deadline() {
    let mut client = ManagedOutboundClient::test_with_static_target("http://localhost:1");
    client.timeout = std::time::Duration::from_millis(10);
    let mut processor = GuardrailResponseProcessor::new(
        Arc::new(client),
        Arc::new(Semaphore::new(0)),
        Arc::new(AttachedGuardrail::new()),
        1,
        0,
        4096,
        4096,
        false,
        false,
    )
    .unwrap();
    processor.window.retain(semantic_record("wait")).unwrap();
    let mut sink = ResponseBodySink::new();
    assert_eq!(
        processor.decide(&MockPluginSession::new(), &mut sink).await,
        Err("timeout")
    );
}

#[tokio::test]
async fn cancelling_an_inflight_decision_releases_the_snapshot_permit() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(1))
                .append_header("content-type", "application/json")
                .set_body_json(serde_json::json!({"version":"v1","action":"pass"})),
        )
        .mount(&server)
        .await;
    let permits = Arc::new(Semaphore::new(1));
    let mut processor = GuardrailResponseProcessor::new(
        Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
            &server.uri(),
        ))),
        Arc::clone(&permits),
        Arc::new(AttachedGuardrail::new()),
        1,
        0,
        4096,
        4096,
        false,
        false,
    )
    .unwrap();
    processor.window.retain(semantic_record("cancel")).unwrap();
    let session = mock_request_session();
    let mut sink = ResponseBodySink::new();
    let mut decision = Box::pin(processor.decide(&session, &mut sink));
    tokio::select! {
        result = &mut decision => panic!("decision unexpectedly completed: {result:?}"),
        () = tokio::time::sleep(std::time::Duration::from_millis(25)) => {}
    }
    assert_eq!(permits.available_permits(), 0);
    drop(decision);
    assert_eq!(permits.available_permits(), 1);
}

#[tokio::test]
async fn cancelling_driver_await_releases_real_guardrail_window_permit_and_owner_once() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(1))
                .append_header("content-type", "application/json")
                .set_body_json(serde_json::json!({"version":"v1","action":"pass"})),
        )
        .mount(&server)
        .await;
    let permits = Arc::new(Semaphore::new(1));
    let owner = Arc::new(AttachedGuardrail::new());
    let processor = GuardrailResponseProcessor::new(
        Arc::new(ManagedOutboundClient::test_with_static_target(&localhost_uri(
            &server.uri(),
        ))),
        Arc::clone(&permits),
        Arc::clone(&owner),
        1,
        0,
        4096,
        4096,
        false,
        false,
    )
    .unwrap();
    let mut ctx = crate::EdgionHttpContext::default();
    ctx.response_stream_kind = ResponseStreamKind::Sse;
    let state = ctx.ensure_ai_state();
    state.semantic_guard_claimed = true;
    state.stream_processor = Some(Box::new(crate::ai::UsageExtractionProcessor::new(Arc::new(
        crate::ai::AiUsageShared::default(),
    ))));
    state.semantic_processor = Some(Box::new(processor));
    let mut session = live_session().await;
    // Mirror the private request-entry capture before driving a later response phase.
    ctx.request_info.method = Some(session.req_header().method.clone());
    let mut body = Some(Bytes::from_static(
        b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"cancel-driver\"}}]}\n\n",
    ));
    let mut sink = ResponseBodySink::new();
    ctx.freeze_response_processor_driver();
    let future =
        crate::routes::http::proxy_http::run_stream_processors(&mut session, &mut ctx, &mut body, false, &mut sink);

    assert!(tokio::time::timeout(std::time::Duration::from_millis(25), future)
        .await
        .is_err());
    assert_eq!(permits.available_permits(), 1);
    assert!(ctx.ai_state().unwrap().semantic_processor.is_none());
    assert!(ctx.ai_state().unwrap().canonical_record_batch.is_none());
    let summary = owner.take_summary_line().unwrap();
    assert!(summary.contains("outcome=cancelled"));
    assert!(summary.contains("reason=callback-dropped"));
    assert!(owner.take_summary_line().is_none());
}
