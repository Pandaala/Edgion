use super::*;

impl AiUsageShared {
    pub(crate) fn observation_disabled() -> Self {
        Self {
            observation_off: AtomicBool::new(true),
            ..Self::default()
        }
    }
}
use crate::ai::quota::QuotaFinalStatus;
use crate::plugins::runtime::traits::session::MockPluginSession;

fn mock_session() -> MockPluginSession {
    let mut session = MockPluginSession::new();
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    ai.expect_semantic_guard_active().return_const(false);
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
    session.expect_try_claim_response_terminal().return_const(true);
    session.expect_mark_response_terminal_emission().return_const(());
    session
}

async fn feed(processor: &mut UsageExtractionProcessor, chunks: &[(&[u8], bool)], kind: ResponseStreamKind) {
    let mut session = mock_session();
    // Tests that assert on recorded statuses build their own session; this
    // shared driver only has to tolerate them.
    session.expect_record_error().return_const(());
    let mut sink = ResponseBodySink::new();
    for (chunk, eos) in chunks {
        let original = Bytes::copy_from_slice(chunk);
        let mut body = Some(original.clone());
        processor.on_chunk(&mut body, *eos, kind, &mut sink, &mut session).await;
        assert_eq!(body, Some(original), "usage observation must not mutate the response");
    }
}

#[tokio::test]
async fn extracts_non_streaming_usage_at_eos() {
    let shared = Arc::new(AiUsageShared::default());
    let mut processor = UsageExtractionProcessor::new(shared.clone());
    feed(
        &mut processor,
        &[(
            br#"{"id":"x","usage":{"prompt_tokens":12,"completion_tokens":7,"total_tokens":19}}"#,
            true,
        )],
        ResponseStreamKind::None,
    )
    .await;
    assert_eq!(shared.reported_tokens(), Some((12, 7)));
}

#[test]
fn non_streaming_callback_adds_no_allocations_for_promotable_backing() {
    let control = Bytes::from(vec![b' '; 64].into_boxed_slice());
    let (control_clone, control_stats) = crate::test_allocation_counter::count_allocation_stats(|| control.clone());
    assert!(control_stats.calls > 0, "fixture must detect first-clone promotion");
    drop(control_clone);

    let measure = |bytes: Bytes| {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared);
        processor.observe_chunk(&Bytes::from_static(b"{"), ResponseStreamKind::None, false);
        let mut session = mock_session();
        let mut sink = ResponseBodySink::new();
        let original_ptr = bytes.as_ptr();
        let mut body = Some(bytes);
        let mut callback = processor.on_chunk(&mut body, false, ResponseStreamKind::None, &mut sink, &mut session);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            std::future::Future::poll(callback.as_mut(), &mut context)
        });
        assert!(result.is_ready());
        drop(callback);
        assert_eq!(body.as_ref().unwrap().as_ptr(), original_ptr);
        assert_eq!(body.as_ref().unwrap().as_ref(), &[b' '; 64]);
        assert!(!sink.is_terminated());
        stats
    };
    let baseline = measure(Bytes::from_static(&[b' '; 64]));
    let promotable = measure(Bytes::from(vec![b' '; 64].into_boxed_slice()));
    assert_eq!(promotable, baseline);
}

#[tokio::test]
async fn extracts_final_split_sse_usage_and_ignores_done() {
    let shared = Arc::new(AiUsageShared::default());
    let mut processor = UsageExtractionProcessor::new(shared.clone());
    feed(
        &mut processor,
        &[
            (b"data: {\"usage\":{\"prompt_tokens\":4,", false),
            (b"\"completion_tokens\":9}}\n\n", false),
            (b"data: [DONE]\n\n", true),
        ],
        ResponseStreamKind::Sse,
    )
    .await;
    assert_eq!(shared.reported_tokens(), Some((4, 9)));
}

#[tokio::test]
async fn malformed_or_partial_usage_remains_unreported() {
    let shared = Arc::new(AiUsageShared::default());
    let mut processor = UsageExtractionProcessor::new(shared.clone());
    feed(
        &mut processor,
        &[(br#"{"usage":{"prompt_tokens":1}}"#, true)],
        ResponseStreamKind::None,
    )
    .await;
    assert_eq!(shared.reported_tokens(), None);

    let malformed_shared = Arc::new(AiUsageShared::default());
    let mut malformed = UsageExtractionProcessor::new(malformed_shared.clone());
    feed(&mut malformed, &[(b"{not-json", true)], ResponseStreamKind::None).await;
    assert_eq!(malformed_shared.reported_tokens(), None);
}

#[tokio::test]
async fn non_streaming_exact_boundary_is_accepted_and_overflow_degrades() {
    use edgion_waf::seclang::{JsonStreamError, JsonStreamLimit};
    const PADDING: [u8; 4096] = [b' '; 4096];
    let limit = crate::ai::protocols::openai::json_usage::MAX_USAGE_SOURCE_BYTES;
    let prefix = br#"{"usage":{"prompt_tokens":2,"completion_tokens":3}}"#;
    for extra in [0, 1] {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(&mut processor, &[(prefix, false)], ResponseStreamKind::None).await;
        let mut remaining = limit + extra - prefix.len();
        while remaining >= PADDING.len() {
            feed(&mut processor, &[(&PADDING, false)], ResponseStreamKind::None).await;
            remaining -= PADDING.len();
        }
        feed(
            &mut processor,
            &[(&PADDING[..remaining], true)],
            ResponseStreamKind::None,
        )
        .await;
        if extra == 0 {
            assert!(!shared.observation_unavailable());
            assert_eq!(shared.reported_tokens(), Some((2, 3)));
        } else {
            assert!(shared.observation_unavailable());
            assert_eq!(shared.reported_tokens(), None);
            assert_eq!(
                processor.json.as_mut().unwrap().finish(),
                JsonUsageOutcome::Unavailable(JsonStreamError::Limit {
                    kind: JsonStreamLimit::SourceBytes,
                    limit
                })
            );
        }
    }
}

#[tokio::test]
async fn chat_estimates_decoded_output_across_choices_and_event_boundaries() {
    let shared = Arc::new(AiUsageShared::default());
    let mut processor = UsageExtractionProcessor::new(shared.clone());
    let single = concat!(
        ": heartbeat\nevent: chunk\nid: 99\ndata: {\"model\":\"extra metadata\",\"choices\":[\n",
        "data: {\"delta\":{\"role\":\"assistant\",\"content\":\"\\u20ac\\n\",\"reasoning_content\":\"x\"}},",
        "{\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"metadata\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: [DONE]\n\n",
    );
    feed(&mut processor, &[(single.as_bytes(), true)], ResponseStreamKind::Sse).await;
    assert_eq!(shared.output_chars(), 5);
    assert_eq!(shared.accounting_snapshot(8, true).usage.unwrap().output_tokens, 2);

    let split = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"€\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"\\n\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"x\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
    );
    let split_shared = Arc::new(AiUsageShared::default());
    let mut split_processor = UsageExtractionProcessor::new(split_shared.clone());
    for byte in split.as_bytes().chunks(1) {
        split_processor.observe_chunk(&Bytes::copy_from_slice(byte), ResponseStreamKind::Sse, false);
    }
    assert_eq!(split_shared.output_chars(), shared.output_chars());
    assert_eq!(
        split_shared.accounting_snapshot(8, true).usage.unwrap().output_tokens,
        2
    );
}

#[tokio::test]
async fn chat_partial_or_degraded_stream_only_estimates_decoded_content() {
    let oversized = vec![b'x'; crate::sse::DEFAULT_MAX_PENDING_EVENT_BYTES + 1];
    for with_content in [false, true] {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        if with_content {
            feed(
                &mut processor,
                &[(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n", false)],
                ResponseStreamKind::Sse,
            )
            .await;
        }
        for tail in [b"data: {\"choices\":".as_slice(), oversized.as_slice()] {
            feed(&mut processor, &[(tail, false)], ResponseStreamKind::Sse).await;
            let snapshot = shared.accounting_snapshot(8, true);
            assert_eq!(
                snapshot.usage.map(|usage| usage.output_tokens),
                with_content.then_some(1)
            );
        }
        assert!(shared.sse_degraded());
    }
}

#[tokio::test]
async fn chat_metadata_only_adds_zero_and_reported_usage_supersedes_estimation() {
    let shared = Arc::new(AiUsageShared::default());
    let mut processor = UsageExtractionProcessor::new(shared.clone());
    feed(
        &mut processor,
        &[(
            b"data: {\"id\":\"metadata\",\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            false,
        )],
        ResponseStreamKind::Sse,
    )
    .await;
    assert_eq!(shared.accounting_snapshot(2, true).usage.unwrap().output_tokens, 0);
    feed(
        &mut processor,
        &[(b"data: invalid\n\n", false)],
        ResponseStreamKind::Sse,
    )
    .await;
    assert_eq!(
        shared.accounting_snapshot(2, true).source,
        GenAiAttemptUsageSource::Unknown
    );
    feed(
        &mut processor,
        &[(
            b"data: {\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":17}}\n\n",
            true,
        )],
        ResponseStreamKind::Sse,
    )
    .await;
    let snapshot = shared.accounting_snapshot(2, true);
    assert_eq!(snapshot.source, GenAiAttemptUsageSource::Reported);
    assert_eq!(snapshot.usage.unwrap().output_tokens, 17);
}

#[tokio::test]
async fn chat_nullable_content_and_invalid_deltas_have_distinct_usage() {
    for (delta, expected) in [
        (r#"{"content":null}"#, Some(0)),
        (
            r#"{"content":null,"tool_calls":[{"function":{"arguments":"12345"}}]}"#,
            Some(2),
        ),
        (r#"{"content":true}"#, None),
        (r#"{"content":"a","content":"b"}"#, None),
    ] {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        let event = format!("data: {{\"choices\":[{{\"delta\":{delta}}}]}}\n\n");
        feed(&mut processor, &[(event.as_bytes(), true)], ResponseStreamKind::Sse).await;
        assert_eq!(
            shared
                .accounting_snapshot(5, true)
                .usage
                .map(|usage| usage.output_tokens),
            expected
        );
    }
}

#[tokio::test]
async fn many_small_sse_events_in_one_chunk_are_observed_without_batching() {
    let shared = Arc::new(AiUsageShared::default());
    let mut processor = UsageExtractionProcessor::new(shared.clone());
    let chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n".repeat(10_000);
    feed(&mut processor, &[(chunk.as_bytes(), true)], ResponseStreamKind::Sse).await;
    assert_eq!(shared.output_chars(), 10_000);
}

#[tokio::test]
async fn non_streaming_chat_without_reported_usage_is_unknown() {
    for (body, eos) in [
        (br#"{"choices":[{"message":{"content":"some text"}}]}"#.as_slice(), true),
        (b"{", false),
        (b"", true),
        (b"{not-json", true),
    ] {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(&mut processor, &[(body, eos)], ResponseStreamKind::None).await;
        let snapshot = shared.accounting_snapshot(5, true);
        assert_eq!(snapshot.source, GenAiAttemptUsageSource::Unknown);
        assert_eq!(snapshot.usage, None);
        assert_eq!(shared.output_chars(), 0);
    }
}

#[test]
fn output_counter_saturates() {
    let shared = AiUsageShared::default();
    shared.add_output_chars(u64::MAX);
    shared.add_output_chars(1);
    assert_eq!(shared.output_chars(), u64::MAX);
}

#[test]
fn reported_usage_keeps_component_high_water_marks() {
    let shared = AiUsageShared::default();
    shared.observe_reported_usage(10, 0, 2, 0);
    shared.observe_reported_usage(10, 7, 3, 1);
    shared.observe_reported_usage(9, 3, 1, 0);
    assert_eq!(
        shared.reported_usage(),
        Some(ReportedAiUsage {
            input_tokens: 10,
            output_tokens: 7,
            cache_read_input_tokens: 3,
            cache_write_input_tokens: 1,
        })
    );
}

#[tokio::test]
async fn quota_over_limit_appends_an_error_event_and_terminates_without_dropping_the_current_chunk() {
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    // terminate_on_over_limit = true: the route's `onExceeded` resolved
    // to `Reject`.
    let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);

    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiQuotaOverLimitTerminated)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    let mut body = Some(Bytes::from_static(b"data: {\"x\":1}\n\n"));
    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert!(sink.is_terminated(), "over-limit must end the stream");
    assert!(
        shared.gateway_terminated(),
        "settlement can only tell this apart from an upstream truncation through this latch, \
             because a terminate guarantees no eos callback ever follows"
    );
    let extra = sink.peek_extra();
    assert_eq!(extra.len(), 1);
    assert_eq!(
        extra[0], *QUOTA_EXCEEDED_SSE_EVENT,
        "the injected event must reuse the same message/type/code the 429 admission-time \
             rejection renders"
    );
    // This proves the in-flight chunk is not dropped when a terminate is
    // queued, NOT that the sink writes it before the terminate — that
    // ordering is a pump property (`ResponseBodySink::push`/`terminate`
    // have no ordering relationship with each other), exercised by the
    // fork's own `terminate_after_commit_ends_the_body_cleanly` test.
    assert_eq!(
        body,
        Some(Bytes::from_static(b"data: {\"x\":1}\n\n")),
        "the current chunk must not be dropped when the response is about to terminate"
    );
}

#[tokio::test]
async fn quota_over_limit_on_a_non_streaming_response_does_not_terminate() {
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    // requested_streaming = false: an ordinary non-streaming completion
    // has already finished generating by the time its body streams, so
    // there is no burn left to stop and terminating would only truncate
    // a JSON body for no benefit. terminate_on_over_limit = true (Reject)
    // isolates this from the separate onExceeded gate covered below.
    let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), false, true);

    let mut session = mock_session();
    let mut sink = ResponseBodySink::new();
    let mut body = Some(Bytes::from_static(
        br#"{"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    ));
    processor
        .on_chunk(&mut body, false, ResponseStreamKind::None, &mut sink, &mut session)
        .await;

    assert!(
        !sink.is_terminated(),
        "a non-streaming completion must not be cut short"
    );
    assert!(sink.peek_extra().is_empty());
}

#[tokio::test]
async fn quota_over_limit_is_not_terminated_under_throttle_or_downgrade() {
    // `terminate_on_over_limit = false` is what the plugin computes for
    // BOTH `Throttle` and `Downgrade` (see `AiProxy`'s plan construction)
    // -- the processor only ever sees the resolved boolean, so a single
    // `false` case here covers both. Either way the operator declared
    // "do not hard-fail this user", and that must still hold once the
    // stream is over limit, exactly as it did before mid-stream
    // termination existed.
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, false);

    let mut session = mock_session();
    let mut sink = ResponseBodySink::new();
    let mut body = Some(Bytes::from_static(b"data: {\"x\":1}\n\n"));
    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert!(
        !sink.is_terminated(),
        "Throttle/Downgrade must let an over-limit stream run to completion"
    );
    assert!(
        !shared.gateway_terminated(),
        "the latch must follow the terminate, not the over-limit condition: this stream ends \
             at its own eos and settles like any other"
    );
    assert!(sink.peek_extra().is_empty(), "no error event without a terminate");
    assert_eq!(
        body,
        Some(Bytes::from_static(b"data: {\"x\":1}\n\n")),
        "the chunk must pass through untouched"
    );
}

/// Builds one unterminated SSE event past the decoder's cap, which is what
/// permanently degrades it.
fn oversized_sse_event() -> Bytes {
    Bytes::from(vec![b'x'; crate::sse::DEFAULT_MAX_PENDING_EVENT_BYTES + 1])
}

fn malformed_sse_with_valid_middle() -> Bytes {
    Bytes::from_static(b"data: \xff\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"valid\"}}]}\n\ndata: \xfe\n\n")
}

#[tokio::test]
async fn first_malformed_sse_event_is_observed_once_and_tolerated() {
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDecodeMalformed)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();

    let original = Bytes::from_static(b"data: \xff\n\n");
    let mut body = Some(original.clone());
    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;
    let mut later = Some(Bytes::from_static(b"data: {\"choices\":[]}\n\n"));
    processor
        .on_chunk(&mut later, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert_eq!(body, Some(original), "observation must leave upstream bytes untouched");
    assert!(!sink.is_terminated());
    assert!(sink.peek_extra().is_empty());
    assert!(!shared.gateway_terminated());
}

#[tokio::test]
async fn second_cumulative_malformed_sse_event_fails_quota_closed() {
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDecodeMalformed)
        .times(1)
        .return_const(());
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseMalformedQuotaTerminated)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    let original = malformed_sse_with_valid_middle();
    let mut body = Some(original.clone());

    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert_eq!(
        body,
        Some(original),
        "the terminating callback must still forward its current bytes"
    );
    assert!(sink.is_terminated());
    assert!(shared.gateway_terminated());
    assert_eq!(sink.peek_extra().len(), 1);
    assert_eq!(sink.peek_extra()[0], *QUOTA_UNENFORCEABLE_SSE_EVENT);
}

#[tokio::test]
async fn multiple_invalid_lines_in_first_event_consume_one_tolerance() {
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDecodeMalformed)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    let mut body = Some(Bytes::from_static(b"data: \xff\ndata: \xfe\nevent: \xff\n\n"));

    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert!(!sink.is_terminated());
    assert!(sink.peek_extra().is_empty());
}

#[tokio::test]
async fn malformed_sse_never_terminates_without_every_reject_gate() {
    for (quota, requested_streaming, terminate_on_over_limit) in [
        (None, true, true),
        (Some(Arc::new(QuotaAccumulator::new(16))), true, false),
        (Some(Arc::new(QuotaAccumulator::new(16))), false, true),
    ] {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new_with_quota(
            shared.clone(),
            quota,
            requested_streaming,
            terminate_on_over_limit,
        );
        let mut session = mock_session();
        session
            .expect_record_error()
            .withf(|status| *status == EdgionErrorCode::AiSseDecodeMalformed)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();
        let mut body = Some(malformed_sse_with_valid_middle());

        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert!(!sink.is_terminated());
        assert!(sink.peek_extra().is_empty());
        assert!(!shared.gateway_terminated());
    }
}

#[tokio::test]
async fn over_limit_precedes_repeated_malformed_sse_events() {
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDecodeMalformed)
        .times(1)
        .return_const(());
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiQuotaOverLimitTerminated)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    let mut body = Some(malformed_sse_with_valid_middle());

    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert_eq!(sink.peek_extra().len(), 1);
    assert_eq!(sink.peek_extra()[0], *QUOTA_EXCEEDED_SSE_EVENT);
}

#[tokio::test]
async fn decoder_degradation_precedes_repeated_malformed_sse_events() {
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);
    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDecodeMalformed)
        .times(1)
        .return_const(());
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDecodeDegraded)
        .times(1)
        .return_const(());
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDegradedQuotaTerminated)
        .times(1)
        .return_const(());
    let mut bytes = malformed_sse_with_valid_middle().to_vec();
    bytes.extend_from_slice(&oversized_sse_event());
    let mut body = Some(Bytes::from(bytes));
    let mut sink = ResponseBodySink::new();

    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert_eq!(sink.peek_extra().len(), 1);
    assert_eq!(sink.peek_extra()[0], *QUOTA_UNENFORCEABLE_SSE_EVENT);
    assert!(sink.is_terminated(), "only the degradation cause should terminate");
}

#[tokio::test]
async fn oversized_sse_event_must_not_disable_mid_stream_quota_enforcement() {
    // The regression this locks down: `quota.add_output_units` is only
    // reachable from inside `feed_each`'s visitor, and a degraded decoder
    // returns from `feed_each` immediately for the rest of the stream. So
    // once degraded, nothing grows the accumulator, no flush runs, and
    // `over_limit()` can never flip -- enforcement was silently off for the
    // remainder of the request while the upstream kept billing.
    let shared = Arc::new(AiUsageShared::default());
    // Deliberately NOT over limit: the point is that enforcement is
    // unreachable from here on, not that the limit was already hit.
    let quota = Arc::new(QuotaAccumulator::new(16));
    let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);

    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDecodeDegraded)
        .times(1)
        .return_const(());
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDegradedQuotaTerminated)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    let mut body = Some(oversized_sse_event());
    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert!(shared.sse_degraded());
    assert!(
        sink.is_terminated(),
        "a stream whose framing can no longer be parsed is a stream whose cost can no longer \
             be bounded, so quota enforcement must fail closed"
    );
    assert!(
        shared.gateway_terminated(),
        "this cut is as deliberate as the over-limit one, so it must settle its reservation \
             the same way instead of holding it for the rest of the window"
    );
    let extra = sink.peek_extra();
    assert_eq!(extra.len(), 1);
    assert_eq!(
        extra[0], *QUOTA_UNENFORCEABLE_SSE_EVENT,
        "the client must see the same error the admission path renders when quota cannot be \
             enforced, not a private degradation vocabulary"
    );
}

#[test]
fn every_injected_event_stays_parseable_after_an_unterminated_event() {
    // Pins the BEHAVIOR the leading blank line exists for, not its bytes:
    // asserting the sink carries the constant compares it to itself and
    // would keep passing if the prefix were deleted. The injection lands
    // at whatever byte offset the upstream's chunk boundary fell on, so a
    // conforming client can always be partway through a `data:` line when
    // it arrives -- certainly so on the degraded path, and reachable on
    // the over-limit path through ordinary TCP segmentation or a provider
    // writing one byte per chunk. Decoding that client's-eye view with the
    // same WHATWG decoder is what proves the boundary actually works.
    for (event, expected_code) in [
        (&QUOTA_EXCEEDED_SSE_EVENT, "token_quota_exceeded"),
        (&QUOTA_UNENFORCEABLE_SSE_EVENT, "token_quota_unavailable"),
    ] {
        let mut decoder = crate::sse::SseDecoder::with_default_limit();
        let mut events = Vec::new();
        decoder.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial", &mut events);
        decoder.feed(event.as_ref(), &mut events);

        assert_eq!(
            events.len(),
            2,
            "the injected error must arrive as its own event, not concatenated onto the truncated one"
        );
        assert_eq!(events[1].event.as_deref(), Some("error"));
        let value: serde_json::Value =
            serde_json::from_str(&events[1].data).expect("the injected error must parse as JSON client-side");
        assert_eq!(value["error"]["code"], expected_code);
    }
}

#[test]
fn an_injected_event_dispatches_nothing_extra_at_a_clean_boundary() {
    // The other half of the unconditional-prefix argument: a blank line
    // with nothing buffered is a no-op dispatch, so paying two bytes to
    // cover the mid-event case never costs a spurious event at a boundary
    // that was already clean (the Anthropic translator's output always is,
    // since it re-emits only complete events).
    let mut decoder = crate::sse::SseDecoder::with_default_limit();
    let mut events = Vec::new();
    decoder.feed(b"data: {\"choices\":[]}\n\n", &mut events);
    decoder.feed(QUOTA_EXCEEDED_SSE_EVENT.as_ref(), &mut events);

    assert_eq!(
        events.len(),
        2,
        "the leading blank line must not dispatch an empty event"
    );
    assert_eq!(events[1].event.as_deref(), Some("error"));
}

#[tokio::test]
async fn sse_degradation_without_quota_enforcement_only_records_the_signal() {
    // Degradation predates mid-stream enforcement and was harmless on its
    // own: usage falls back to the character estimate. That must stay
    // true, so an unquota'd stream is never cut for a framing reason.
    let shared = Arc::new(AiUsageShared::default());
    let mut processor = UsageExtractionProcessor::new(shared.clone());

    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDecodeDegraded)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    let mut body = Some(oversized_sse_event());
    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;
    // A second chunk proves the sticky decoder flag does not re-append the
    // status once per remaining chunk; `times(1)` above is the assertion.
    let mut more = Some(Bytes::from_static(b"data: {\"x\":1}\n\n"));
    processor
        .on_chunk(&mut more, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert!(shared.sse_degraded());
    assert!(!sink.is_terminated(), "no quota enforcement, so nothing to fail closed");
    assert!(sink.peek_extra().is_empty());
}

#[tokio::test]
async fn an_already_over_limit_stream_reports_over_limit_not_degradation() {
    // Both conditions true at once: the accurate reason is that the limit
    // was actually reached, and the two terminations must stay mutually
    // exclusive because either one alone ends the stream.
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);

    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiSseDecodeDegraded)
        .times(1)
        .return_const(());
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiQuotaOverLimitTerminated)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    let mut body = Some(oversized_sse_event());
    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    let extra = sink.peek_extra();
    assert_eq!(extra.len(), 1, "exactly one termination, never both");
    assert_eq!(extra[0], *QUOTA_EXCEEDED_SSE_EVENT);
}

#[tokio::test]
async fn quota_over_limit_push_failure_is_recorded_and_still_terminates() {
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);

    let mut session = mock_session();
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiQuotaErrorEventDropped)
        .times(1)
        .return_const(());
    session
        .expect_record_error()
        .withf(|status| *status == EdgionErrorCode::AiQuotaOverLimitTerminated)
        .times(1)
        .return_const(());
    let mut sink = ResponseBodySink::new();
    // Exhaust the per-batch emit budget an earlier processor might have
    // already spent, so this processor's own push cannot fit.
    sink.push(Bytes::from(vec![0u8; pingora_proxy::RESPONSE_BODY_EMIT_BUDGET]))
        .expect("filling the budget exactly must succeed");
    let mut body = Some(Bytes::from_static(b"data: {\"x\":1}\n\n"));
    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;

    assert!(
        sink.is_terminated(),
        "a dropped diagnostic event must not stop the gateway from terminating"
    );
}

#[test]
fn changes_body_length_tracks_every_precondition_the_terminate_gate_checks() {
    let shared = Arc::new(AiUsageShared::default());
    let observing = UsageExtractionProcessor::new(shared.clone());
    assert!(
        !observing.changes_body_length(),
        "pure observation must stay framing-neutral"
    );

    // Quota attached, streaming, Reject: this is the only combination
    // that can actually terminate or inject, so it is the only one that
    // must declare true.
    let quota = Arc::new(QuotaAccumulator::new(16));
    let enforcing = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);
    assert!(
        enforcing.changes_body_length(),
        "an attached quota accumulator under Reject can truncate or extend the body, so \
             content-length must be stripped"
    );

    // Quota attached, non-streaming: `on_chunk`'s gate requires
    // `requested_streaming`, so this processor never mutates the body
    // regardless of quota state -- declaring true here would strip
    // content-length from a response this processor provably never
    // touches.
    let quota = Arc::new(QuotaAccumulator::new(16));
    let non_streaming = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), false, true);
    assert!(
        !non_streaming.changes_body_length(),
        "a non-streaming completion is never mutated by this processor"
    );

    // Quota attached, streaming, but the route's onExceeded resolved to
    // Throttle/Downgrade (terminate_on_over_limit = false): the
    // termination gate can never fire, so this processor never mutates
    // the body here either.
    let quota = Arc::new(QuotaAccumulator::new(16));
    let throttled = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, false);
    assert!(
        !throttled.changes_body_length(),
        "Throttle/Downgrade never terminate, so content-length must not be stripped"
    );
}

#[test]
fn openai_incremental_quota_counts_only_semantic_output_deltas() {
    let units = |json: &[u8]| {
        let value: serde_json::Value = serde_json::from_slice(json).unwrap();
        approximate_openai_output_units(&value)
    };
    assert_eq!(
        units(br#"{"choices":[{"delta":{"role":"assistant"}},{"delta":{"content":""}}]}"#),
        0
    );
    assert_eq!(
        units(
            br#"{"choices":[{"delta":{"content":"hello"}},{"delta":{"tool_calls":[{"function":{"arguments":"x"}}]}}]}"#
        ),
        2
    );
    assert_eq!(
        units(
            br#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#
        ),
        0
    );
}

fn value_observation_oracle(json: &[u8]) -> Option<UsageEventObservation> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let reported_usage = value
        .get("usage")
        .and_then(serde_json::Value::as_object)
        .and_then(|usage| {
            Some(ReportedAiUsage {
                input_tokens: usage.get("prompt_tokens")?.as_u64()?,
                output_tokens: usage.get("completion_tokens")?.as_u64()?,
                cache_read_input_tokens: usage
                    .get("prompt_tokens_details")
                    .and_then(serde_json::Value::as_object)
                    .and_then(|details| details.get("cached_tokens"))
                    .and_then(serde_json::Value::as_u64)
                    .or_else(|| usage.get("cache_read_input_tokens").and_then(serde_json::Value::as_u64))
                    .unwrap_or(0),
                cache_write_input_tokens: usage
                    .get("cache_creation_input_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            })
        });
    Some(UsageEventObservation {
        reported_usage,
        output_units: approximate_openai_output_units(&value),
    })
}

#[test]
fn streaming_observer_matches_value_for_tolerant_numeric_and_duplicate_fields() {
    let cases: &[&[u8]] = &[
            br#"{"usage":{"prompt_tokens":7,"completion_tokens":9,"prompt_tokens_details":{"cached_tokens":3},"cache_read_input_tokens":2,"cache_creation_input_tokens":1}}"#,
            br#"{"usage":{"prompt_tokens":null,"completion_tokens":9}}"#,
            br#"{"usage":{"prompt_tokens":-1,"completion_tokens":9}}"#,
            br#"{"usage":{"prompt_tokens":1.5,"completion_tokens":9}}"#,
            br#"{"usage":{"prompt_tokens":18446744073709551616,"completion_tokens":9}}"#,
            br#"{"usage":{"prompt_tokens":1,"prompt_tokens":"wrong","completion_tokens":9}}"#,
            br#"{"usage":{"prompt_tokens":"wrong","prompt_tokens":1,"completion_tokens":9}}"#,
            br#"{"usage":{"prompt_tokens":1,"completion_tokens":2,"cache_read_input_tokens":4,"prompt_tokens_details":{"cached_tokens":5,"cached_tokens":null}}}"#,
            br#"{"usage":{"prompt_tokens":1,"completion_tokens":2,"cache_read_input_tokens":4,"prompt_tokens_details":false}}"#,
            br#"{"usage":{"prompt_tokens":1,"completion_tokens":2},"usage":null}"#,
            br#"{"usage":null,"usage":{"prompt_tokens":1,"completion_tokens":2}}"#,
        ];
    for json in cases {
        assert_eq!(
            observe_openai_streaming_event(json),
            value_observation_oracle(json),
            "{json:?}"
        );
    }
}

#[test]
fn streaming_observer_matches_value_for_choice_replacement_and_escapes() {
    let cases: &[&[u8]] = &[
            br#"{"choices":[{"delta":{"content":"hello","tool_calls":[{"function":{"arguments":"x"}}]}}]}"#,
            br#"{"choices":[{"delta":{"content":""}},{"delta":{"tool_calls":[{"function":{"arguments":""}},{"function":{"arguments":"x"}}]}}]}"#,
            br#"{"choices":[{"delta":{"content":"x","content":null}}]}"#,
            br#"{"choices":[{"delta":{"content":null,"content":"x"}}]}"#,
            br#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"x","arguments":3}}]}}]}"#,
            br#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"x"},"function":false}]}}]}"#,
            br#"{"choices":[{"delta":{"content":"x"},"delta":null}]}"#,
            br#"{"choices":[{"delta":null,"delta":{"content":"x"}}]}"#,
            br#"{"choices":[{"delta":{"content":"x"}}],"choices":false}"#,
            br#"{"choices":false,"choices":[{"delta":{"content":"x"}}]}"#,
            br#"{"cho\u0069ces":[{"del\u0074a":{"cont\u0065nt":"\u20ac"}}]}"#,
        ];
    for json in cases {
        assert_eq!(
            observe_openai_streaming_event(json),
            value_observation_oracle(json),
            "{json:?}"
        );
    }
}

#[test]
fn streaming_observer_counts_at_most_one_unit_per_qualifying_choice() {
    let observation = observe_openai_streaming_event(
            br#"{"choices":[{"delta":{"content":"x","tool_calls":[{"function":{"arguments":"a"}},{"function":{"arguments":"b"}}]}},{"delta":{"content":"y"}}]}"#,
        )
        .unwrap();
    assert_eq!(observation.output_units, 2);
}

#[test]
fn streaming_observer_discards_the_whole_malformed_document() {
    for json in [
        b"{not-json".as_slice(),
        br#"{"usage":{"prompt_tokens":1,"completion_tokens":2},"unknown":[1,}"#,
        br#"{"choices":[{"delta":{"content":"x"}}]} trailing"#,
    ] {
        assert_eq!(observe_openai_streaming_event(json), None);
    }
}

#[test]
fn streaming_observer_bounded_cross_product_matches_value_oracle() {
    let numeric_values = ["0", "17", "null", "-1", "1.25", "18446744073709551616", "{}", "[]"];
    let string_values = [r#"""#, r#""x""#, r#""\u20ac""#, "null", "7", "{}", "[]"];
    for prompt in numeric_values {
        for completion in numeric_values {
            let json = format!(r#"{{"usage":{{"prompt_tokens":{prompt},"completion_tokens":{completion}}}}}"#);
            assert_eq!(
                observe_openai_streaming_event(json.as_bytes()),
                value_observation_oracle(json.as_bytes()),
                "{json}"
            );
        }
    }
    for content in string_values {
        for arguments in string_values {
            let json = format!(
                r#"{{"choices":[{{"delta":{{"content":{content},"tool_calls":[{{"function":{{"arguments":{arguments}}}}}]}}}}]}}"#
            );
            assert_eq!(
                observe_openai_streaming_event(json.as_bytes()),
                value_observation_oracle(json.as_bytes()),
                "{json}"
            );
        }
    }
}

#[tokio::test]
async fn openai_streaming_processor_updates_quota_only_for_semantic_deltas() {
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(100));
    let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota.clone()), true, false);

    for (event, expected) in [
        (r#"{"choices":[{"delta":{"role":"assistant"}}]}"#, 0),
        (r#"{"choices":[{"delta":{"content":""}}]}"#, 0),
        (r#"{"choices":[{"delta":{"content":"hello"}}]}"#, 1),
        (
            r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":""}}]}}]}"#,
            1,
        ),
        (
            r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"x"}}]}}]}"#,
            2,
        ),
        (
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":7,"total_tokens":17}}"#,
            2,
        ),
    ] {
        let framed = format!("data: {event}\n\n");
        feed(&mut processor, &[(framed.as_bytes(), false)], ResponseStreamKind::Sse).await;
        assert_eq!(quota.observed_output_for_test(), expected);
    }
    feed(&mut processor, &[(b"data: [DONE]\n\n", true)], ResponseStreamKind::Sse).await;
    assert_eq!(shared.reported_tokens(), Some((10, 7)));
}

#[test]
fn accounting_quality_precedence_and_checked_arithmetic() {
    let shared = AiUsageShared::default();
    shared.set_input_chars(8);
    assert_eq!(
        shared.accounting_snapshot(2, true).source,
        GenAiAttemptUsageSource::Estimated
    );
    shared.json_started.store(true, Ordering::Release);
    assert_eq!(
        shared.accounting_snapshot(2, false).unavailable,
        Some(AiUsageUnavailableReason::IncompleteJson)
    );
    shared.json_complete.store(true, Ordering::Release);
    assert_eq!(
        shared.accounting_snapshot(2, true).source,
        GenAiAttemptUsageSource::Unknown
    );
    shared.observe_reported_usage(u64::MAX, 0, u64::MAX, 1);
    assert_eq!(shared.reported_usage().unwrap().cache_read_input_tokens, 0);
    assert_eq!(shared.reported_usage().unwrap().cache_write_input_tokens, 0);
    assert_eq!(
        shared.accounting_snapshot(2, true).source,
        GenAiAttemptUsageSource::Reported
    );
    shared.observe_reported_usage(u64::MAX, 1, 0, 0);
    let snapshot = shared.accounting_snapshot(2, true);
    assert_eq!(snapshot.unavailable, Some(AiUsageUnavailableReason::ArithmeticOverflow));
    assert_eq!(snapshot.usage, None);
    shared.mark_observation_unavailable();
    assert_eq!(
        shared.accounting_snapshot(2, true).source,
        GenAiAttemptUsageSource::Unknown
    );
    let off = AiUsageShared::observation_disabled();
    assert_eq!(
        off.accounting_snapshot(2, true).source,
        GenAiAttemptUsageSource::NotObserved
    );
    assert_eq!(off.accounting_snapshot(2, true).usage, None);
}

#[tokio::test]
async fn ordinary_json_usage_is_provisional_and_malformed_tail_is_unknown() {
    let shared = Arc::new(AiUsageShared::default());
    let mut processor = UsageExtractionProcessor::new(shared.clone());
    feed(
        &mut processor,
        &[(br#"{"usage":{"prompt_tokens":2,"completion_tokens":3}}"#, false)],
        ResponseStreamKind::None,
    )
    .await;
    assert_eq!(shared.reported_usage(), None);
    assert_eq!(
        shared.accounting_snapshot(2, true).source,
        GenAiAttemptUsageSource::Unknown
    );
    feed(&mut processor, &[(b"garbage", true)], ResponseStreamKind::None).await;
    assert_eq!(shared.reported_usage(), None);
    assert_eq!(
        shared.accounting_snapshot(2, true).source,
        GenAiAttemptUsageSource::Unknown
    );
}

#[tokio::test]
async fn encoded_passthrough_does_not_parse_or_count_transformed_bytes() {
    let shared = Arc::new(AiUsageShared::default());
    let mut processor = UsageExtractionProcessor::new(shared.clone());
    let mut header = pingora_http::ResponseHeader::build(200, None).unwrap();
    header.append_header("content-encoding", "identity").unwrap();
    header.append_header("content-encoding", "gzip").unwrap();
    processor.on_response_header(&mut header);
    header.remove_header("content-encoding");
    feed(
        &mut processor,
        &[(br#"{"usage":{"prompt_tokens":2,"completion_tokens":3}}"#, true)],
        ResponseStreamKind::None,
    )
    .await;
    assert_eq!(shared.output_chars(), 0);
    assert_eq!(shared.reported_usage(), None);
    assert_eq!(
        shared.accounting_snapshot(2, true).source,
        GenAiAttemptUsageSource::Unknown
    );
    assert!(processor.json.is_none());
}

#[test]
fn public_off_initialization_suppresses_converter_numeric_publication() {
    use edgion_resources::resources::edgion_plugins::AiUsageObservation;
    let usage = AiUsageShared::with_observation(AiUsageObservation::Off);
    usage.set_input_chars(100);
    usage.add_output_chars(200);
    usage.observe_reported_usage(7, 11, 0, 0);
    assert_eq!(usage.input_chars(), 0);
    assert_eq!(usage.output_chars(), 0);
    assert_eq!(usage.reported_usage(), None);
    let snapshot = usage.accounting_snapshot(99, true);
    assert_eq!(snapshot.source, GenAiAttemptUsageSource::NotObserved);
    assert_eq!(snapshot.usage, None);
}

#[tokio::test]
async fn responses_reports_consumption_for_all_native_json_terminal_statuses() {
    for status in ["completed", "incomplete", "failed", "cancelled"] {
        let shared = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Provider));
        let quota = Arc::new(QuotaAccumulator::new(1));
        let mut processor = UsageExtractionProcessor::new_responses(shared.clone(), Some(quota.clone()), false, false);
        let body = format!(
            r#"{{"output":[{{"text":"repeated output"}}],"status":"{status}","usage":{{"input_tokens":11,"output_tokens":7}}}}"#
        );
        let chunks: Vec<_> = body
            .as_bytes()
            .iter()
            .enumerate()
            .map(|(i, _)| (&body.as_bytes()[i..i + 1], i + 1 == body.len()))
            .collect();
        feed(&mut processor, &chunks, ResponseStreamKind::None).await;
        assert_eq!(
            shared.accounting_snapshot(99999, true).source,
            GenAiAttemptUsageSource::Reported
        );
        assert_eq!(shared.reported_tokens(), Some((11, 7)));
        assert_eq!(quota.observed_output_for_test(), 7);
        assert_eq!(shared.permits_provider_success(), status == "completed");
        assert_eq!(shared.output_chars(), 0);
    }
}

#[tokio::test]
async fn responses_missing_invalid_or_partial_json_usage_never_becomes_an_estimate() {
    for (body, eos) in [
        (
            br#"{"status":"completed","output":[{"text":"some output"}]}"#.as_slice(),
            true,
        ),
        (br#"{"status":"completed","usage":null}"#, true),
        (
            br#"{"status":"completed","usage":{"input_tokens":11,"output_tokens":7}}garbage"#,
            true,
        ),
        (
            br#"{"status":"in_progress","usage":{"input_tokens":11,"output_tokens":7}}"#,
            true,
        ),
        (
            br#"{"status":"completed","usage":{"input_tokens":11,"output_tokens":7}}"#,
            false,
        ),
    ] {
        let shared = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Provider));
        shared.set_input_chars(4000);
        let mut processor = UsageExtractionProcessor::new_responses(shared.clone(), None, false, false);
        feed(&mut processor, &[(body, eos)], ResponseStreamKind::None).await;
        let snapshot = shared.accounting_snapshot(1000, true);
        assert_eq!(snapshot.source, GenAiAttemptUsageSource::Unknown);
        assert!(snapshot.usage.is_none());
        assert_eq!(
            shared.permits_provider_success(),
            eos && !shared.observation_unavailable()
                && shared.response_outcome() == crate::ai::protocols::openai::json_usage::ResponseOutcome::Completed
        );
    }
}

#[tokio::test]
async fn responses_sse_deltas_and_done_repeats_only_charge_one_terminal_usage_total() {
    let shared = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Provider));
    let quota = Arc::new(QuotaAccumulator::new(1));
    let mut processor = UsageExtractionProcessor::new_responses(shared.clone(), Some(quota.clone()), true, false);
    let body = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"text\",\"sequence_number\":1}\n\ndata: {\"type\":\"response.output_text.done\",\"text\":\"text\",\"sequence_number\":2}\n\ndata: {\"type\":\"response.completed\",\"sequence_number\":3,\"response\":{\"status\":\"completed\",\"output\":[{\"text\":\"text\"}],\"usage\":{\"input_tokens\":11,\"output_tokens\":7}}}\n\n";
    let chunks: Vec<_> = body
        .iter()
        .enumerate()
        .map(|(i, _)| (&body[i..i + 1], i + 1 == body.len()))
        .collect();
    feed(&mut processor, &chunks, ResponseStreamKind::Sse).await;
    assert_eq!(shared.reported_tokens(), Some((11, 7)));
    assert_eq!(quota.observed_output_for_test(), 7);
    assert_eq!(shared.output_chars(), 0);
    assert!(shared.permits_provider_success());
    assert_eq!(processor.response_sequence, Some(3));
}

#[tokio::test]
async fn responses_sse_requires_matching_terminal_and_valid_framing() {
    for body in [
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"text\"}\n\n".as_slice(),
        b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"failed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n",
        b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\ndata: unfinished",
    ] {
        let shared = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Provider));
        let mut processor = UsageExtractionProcessor::new_responses(shared.clone(), None, true, false);
        feed(&mut processor, &[(body,true)], ResponseStreamKind::Sse).await;
        assert_eq!(shared.accounting_snapshot(42,true).source, GenAiAttemptUsageSource::Unknown);
        assert!(!shared.permits_provider_success());
    }
}

#[test]
fn responses_off_preserves_transport_latency_but_does_not_claim_model_success_or_usage() {
    let shared = AiUsageShared::with_reported_only(AiUsageObservation::Off);
    shared.observe_reported_usage(1, 2, 0, 0);
    assert_eq!(
        shared.accounting_snapshot(42, true).source,
        GenAiAttemptUsageSource::NotObserved
    );
    assert!(shared.permits_latency_sample());
    assert!(!shared.permits_provider_success());
}

#[tokio::test]
async fn responses_heartbeat_blocks_do_not_invalidate_usage_or_trigger_active_reject() {
    let body=b": ping\n\nid: transport-id\n\nevent: heartbeat\n\ndata: {\"type\":\"response.completed\",\"sequence_number\":3,\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":11,\"output_tokens\":7}}}\n\n: final ping\n\n";
    for active in [false, true] {
        let shared = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Provider));
        let quota = Arc::new(QuotaAccumulator::new(100));
        let mut processor = UsageExtractionProcessor::new_responses(shared.clone(), Some(quota.clone()), true, active);
        let mut session = mock_session();
        let mut sink = ResponseBodySink::new();
        for (index, byte) in body.iter().enumerate() {
            let mut chunk = Some(Bytes::copy_from_slice(std::slice::from_ref(byte)));
            processor
                .on_chunk(
                    &mut chunk,
                    index + 1 == body.len(),
                    ResponseStreamKind::Sse,
                    &mut sink,
                    &mut session,
                )
                .await;
            assert!(!sink.is_terminated());
            assert!(sink.peek_extra().is_empty());
        }
        assert_eq!(shared.reported_tokens(), Some((11, 7)));
        assert_eq!(quota.observed_output_for_test(), 7);
        assert!(!shared.observation_unavailable());
        assert!(shared.permits_provider_success());
    }
}

#[tokio::test]
async fn poisoned_chat_payload_keeps_accounting_with_or_without_guarding() {
    use crate::ai::protocols::openai::canonical_sse::CanonicalSourceStatus;
    use parking_lot::Mutex;
    for guarding in [false, true] {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota.clone()), true, true);
        let batches = Arc::new(Mutex::new(Vec::new()));
        let captured = batches.clone();
        let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
        ai.expect_semantic_guard_active().return_const(guarding);
        ai.expect_publish_canonical_records().returning(move |batch| {
            captured.lock().push(batch);
            Ok(())
        });
        let mut session = MockPluginSession::new();
        session
            .expect_ai()
            .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
        session
            .expect_record_error()
            .withf(|code| *code == EdgionErrorCode::AiSseDecodeMalformed)
            .times(1)
            .return_const(());
        let raw = Bytes::from_static(b"id: \xff\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"text\"}}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":9}}\n\n");
        let mut chunk = Some(raw.clone());
        let mut sink = ResponseBodySink::new();
        processor
            .on_chunk(&mut chunk, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;
        assert_eq!(shared.reported_tokens(), Some((7, 9)));
        assert_eq!(quota.observed_output_for_test(), 1);
        assert_eq!(processor.sse.malformed_events(), 1);
        assert!(!shared.observation_unavailable());
        assert!(!sink.is_terminated());
        if guarding {
            assert!(chunk.is_none());
            let batch = batches.lock().pop().unwrap();
            assert_eq!(batch.status(), CanonicalSourceStatus::Unavailable);
            assert_eq!(batch.into_raw_bytes().unwrap(), raw);
        } else {
            assert_eq!(chunk, Some(raw));
            assert!(batches.lock().is_empty());
        }
    }
}

#[test]
fn unchanged_reported_usage_avoids_publication_after_normalization() {
    let shared = AiUsageShared::default();
    assert_eq!(shared.reported_usage(), None);
    shared.observe_reported_usage(0, 0, 0, 0);
    assert_eq!(
        shared.reported_usage(),
        Some(ReportedAiUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
        })
    );
    let (_, allocations) = crate::test_allocation_counter::count_allocations(|| {
        shared.observe_reported_usage(0, 0, 0, 0);
    });
    assert_eq!(allocations, 0);

    shared.observe_reported_usage(10, 7, 3, 1);
    let expected = shared.reported_usage();
    for (input, output, read, write) in [(10, 7, 3, 1), (9, 3, 1, 0)] {
        let (_, allocations) = crate::test_allocation_counter::count_allocations(|| {
            shared.observe_reported_usage(input, output, read, write);
        });
        assert_eq!(shared.reported_usage(), expected);
        assert_eq!(allocations, 0);
    }

    shared.observe_reported_usage(10, 7, u64::MAX, 1);
    let corrected = Some(ReportedAiUsage {
        input_tokens: 10,
        output_tokens: 7,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
    });
    assert_eq!(shared.reported_usage(), corrected);
    let (_, allocations) = crate::test_allocation_counter::count_allocations(|| {
        shared.observe_reported_usage(10, 7, u64::MAX, 1);
    });
    assert_eq!(shared.reported_usage(), corrected);
    assert_eq!(allocations, 0);
}

#[test]
fn cost_fraction_formatting_preserves_decimal_bytes_with_fewer_allocations() {
    let pricing = edgion_resources::resources::edgion_backend::AiModelPricing {
        input_per_million_usd: "0.000001".to_string(),
        output_per_million_usd: "0".to_string(),
        cache_read_input_per_million_usd: None,
        cache_write_input_per_million_usd: None,
    }
    .parsed()
    .unwrap();
    let mut cases = vec![
        0,
        999_999_999_999,
        1_000_000_000_000,
        1_000_000_000_001,
        1_234_567_890_120,
        u64::MAX,
    ];
    cases.extend((0..12).map(|power| 10_u64.pow(power)));
    for input_tokens in cases {
        let usage = ReportedAiUsage {
            input_tokens,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
        };
        let whole = u128::from(input_tokens) / 1_000_000_000_000;
        let fraction = u128::from(input_tokens) % 1_000_000_000_000;
        let (expected, previous_allocations) = crate::test_allocation_counter::count_allocations(|| {
            if fraction == 0 {
                whole.to_string()
            } else {
                let fraction = format!("{fraction:012}");
                format!("{whole}.{}", fraction.trim_end_matches('0'))
            }
        });
        let (actual, allocations) =
            crate::test_allocation_counter::count_allocations(|| estimated_cost_usd(pricing, usage));
        assert_eq!(actual, expected, "input tokens: {input_tokens}");
        if fraction == 0 {
            assert_eq!(allocations, previous_allocations);
        } else {
            assert!(
                allocations < previous_allocations,
                "input tokens: {input_tokens}, {allocations} versus {previous_allocations}"
            );
        }
    }
}
