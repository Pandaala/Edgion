use super::*;
use crate::sse::SseDecoder;

fn observe(raw: &'static [u8]) -> CanonicalEventObservation {
    let mut decoder = SseDecoder::with_default_limit();
    let mut result = None;
    decoder.feed_records(Bytes::from_static(raw), |record| {
        if let SseRecordRef::Block { status, event } = record {
            result = Some(observe_canonical_block(status, &event, true));
        }
    });
    result.unwrap()
}

#[test]
fn form_feed_in_json_syntax_cannot_report_usage_and_preserves_raw_sse() {
    let raw = b"data: {\"usage\":\x0c{\"prompt_tokens\":7,\"completion_tokens\":9}}\n\n";
    let mut decoder = SseDecoder::with_default_limit();
    let mut events = 0;
    decoder.feed_each_bytes(Bytes::from_static(raw), |event| {
        events += 1;
        assert_eq!(event.raw().to_bytes().as_ref(), raw);
        let observed = observe_canonical_block(SseBlockStatus::Data, &event, true);
        assert_eq!(observed.usage, CanonicalUsageObservation::default());
        assert_eq!(observed.output_units, 0);
        assert!(observed.fragments.is_empty());
        assert!(!observed.semantic_available);
    });
    assert_eq!(events, 1);
    assert!(!decoder.malformed());
    assert_eq!(decoder.malformed_events(), 0);
}

#[test]
fn one_pass_observation_reports_usage_and_all_semantic_tokens() {
    let observed = observe(b"data: {\"choices\":[{\"index\":2,\"delta\":{\"content\":\"a\\\\u00e9\",\"reasoning_content\":\"r\",\"tool_calls\":[{\"index\":3,\"function\":{\"arguments\":\"{}\"}}]}}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":9,\"prompt_tokens_details\":{\"cached_tokens\":2}}}\n\n");
    assert!(observed.semantic_available);
    // Preserve the established quota estimate: one unit per qualifying
    // choice, even when that choice carries multiple semantic fields.
    assert_eq!(observed.output_units, 1);
    assert_eq!(observed.fragments.len(), 3);
    assert_eq!(observed.usage.prompt_tokens, Some(7));
    assert_eq!(observed.usage.cached_tokens, Some(2));
}

#[test]
fn token_crossing_synthetic_data_newline_is_unavailable() {
    let observed = observe(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a\ndata: b\"}}]}\n\n");
    assert!(!observed.semantic_available);
    assert!(observed.fragments.is_empty());
}

#[test]
fn duplicate_or_ambiguous_indices_disable_annotations_but_keep_usage() {
    let observed = observe(b"data: {\"choices\":[{\"index\":0,\"index\":1,\"delta\":{\"content\":\"x\"}}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":5}}\n\n");
    assert!(!observed.semantic_available);
    assert_eq!(observed.usage.prompt_tokens, Some(4));
}

#[test]
fn duplicate_or_wrong_typed_semantic_paths_are_unavailable() {
    for raw in [
            b"data: {\"choices\":[],\"choices\":[]}\n\n".as_slice(),
            b"data: {\"choices\":{}}\n\n".as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"x\",\"content\":\"y\"}}]}\n\n".as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":7}}]}\n\n".as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":{}}}]}\n\n".as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"x\",\"arguments\":\"y\"}}]}}]}\n\n".as_slice(),
        ] {
            let observed = observe(Box::leak(raw.to_vec().into_boxed_slice()));
            assert!(!observed.semantic_available, "raw={}", String::from_utf8_lossy(raw));
            assert!(observed.fragments.is_empty());
        }
}

#[test]
fn batch_exact_record_limit_then_falls_back_without_losing_raw_bytes() {
    let mut batch = CanonicalRecordBatch::new();
    for _ in 0..MAX_CANONICAL_BATCH_RECORDS {
        batch.push(WindowRecord::raw(vec![Bytes::from_static(b"x")])).unwrap();
    }
    batch.push(WindowRecord::raw(vec![Bytes::from_static(b"y")])).unwrap();
    assert_eq!(batch.status(), CanonicalSourceStatus::Unavailable);
    assert_eq!(batch.raw_len(), MAX_CANONICAL_BATCH_RECORDS + 1);
    let raw = batch.into_raw_bytes().unwrap();
    assert_eq!(
        &raw[..MAX_CANONICAL_BATCH_RECORDS],
        vec![b'x'; MAX_CANONICAL_BATCH_RECORDS]
    );
    assert_eq!(raw.last(), Some(&b'y'));
}

#[test]
fn batch_raw_cap_is_inclusive_and_plus_one_returns_the_record() {
    let mut batch = CanonicalRecordBatch::new();
    batch
        .push(WindowRecord::raw(vec![Bytes::from(vec![
            b'x';
            MAX_CANONICAL_BATCH_RAW_BYTES
        ])]))
        .unwrap();
    let rejected = batch
        .push(WindowRecord::raw(vec![Bytes::from_static(b"y")]))
        .unwrap_err();
    assert_eq!(rejected.raw_len(), 1);
    assert_eq!(batch.raw_len(), MAX_CANONICAL_BATCH_RAW_BYTES);
}

#[tokio::test]
async fn semantic_only_source_preserves_split_raw_records_without_usage_counting() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::StreamingResponseProcessor;
    use parking_lot::Mutex;
    use pingora_proxy::ResponseBodySink;
    use std::sync::Arc;
    let records = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&records);
    let mut session = MockPluginSession::new();
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    ai.expect_semantic_guard_active().return_const(true);
    ai.expect_publish_canonical_records().returning(move |batch| {
        captured.lock().push(batch);
        Ok(())
    });
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
    let mut source = SemanticSseSource::new();
    let mut sink = ResponseBodySink::new();
    let raw = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":11}}\n\n";
    for (index, part) in raw.chunks(13).enumerate() {
        let mut chunk = Some(Bytes::copy_from_slice(part));
        source
            .on_chunk(
                &mut chunk,
                (index + 1) * 13 >= raw.len(),
                crate::ctx::ResponseStreamKind::Sse,
                &mut sink,
                &mut session,
            )
            .await;
        assert!(chunk.is_none());
    }
    assert_eq!(source.decoder.take_data_chars(), 0);
    let mut decoder = SseDecoder::with_default_limit();
    decoder.feed_each_bytes(Bytes::from_static(raw), |event| {
        let observation = observe_event(&event, false);
        assert_eq!(observation.usage.prompt_tokens, None);
        assert_eq!(observation.fragments.len(), 1);
    });
    let raw_bytes: Vec<u8> = records
        .lock()
        .drain(..)
        .flat_map(|batch| batch.into_raw_bytes().unwrap().to_vec())
        .collect();
    assert_eq!(raw_bytes, raw);
}

#[test]
fn typed_controls_and_poison_preserve_distinct_semantic_and_accounting_contracts() {
    for raw in [b": ping\n\n".as_slice(), b"id: cursor\n\n", b"event: heartbeat\n\n"] {
        let observed = observe(raw);
        assert!(observed.semantic_available);
        assert_eq!(observed.usage, CanonicalUsageObservation::default());
        assert_eq!(observed.output_units, 0);
        assert!(observed.fragments.is_empty());
    }
    for raw in [b"data:\n\n".as_slice(), b"data\n\n"] {
        assert!(!observe(raw).semantic_available);
    }
    let raw = b"id: \xff\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"text\"}}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":9}}\n\n";
    let mut decoder = SseDecoder::with_default_limit();
    decoder.feed_records(Bytes::from_static(raw), |record| {
        let SseRecordRef::Block { status, event } = record else {
            panic!("expected complete block")
        };
        assert_eq!(status, SseBlockStatus::Poisoned);
        assert_eq!(event.raw().to_bytes().as_ref(), raw);
        for account_usage in [false, true] {
            let observed = observe_canonical_block(status, &event, account_usage);
            assert!(!observed.semantic_available);
            assert!(observed.fragments.is_empty());
            assert_eq!(observed.output_units, 1);
            assert_eq!(observed.usage.prompt_tokens, account_usage.then_some(7));
            assert_eq!(observed.usage.completion_tokens, account_usage.then_some(9));
        }
    });
    assert_eq!(decoder.malformed_events(), 1);
}

#[test]
fn canonical_writer_preserves_json_bytes_limits_and_semantic_ranges() {
    let content = "quoted \"text\"\\line\n\té";
    let reasoning = "same text";
    let arguments = r#"{"city":"Montréal"}"#;
    let value = serde_json::json!({
        "choices": [{"index": 2, "delta": {
            "content": content,
            "reasoning_content": reasoning,
            "tool_calls": [{"index": 3, "function": {"arguments": arguments}}]
        }}],
        "escaped\"key": [null, true, false, 42, -3, 1.25, "other"]
    });
    let expected = format!("data: {}\n\n", serde_json::to_string(&value).unwrap());
    for limit in 0..expected.len() {
        assert!(write_canonical_record(&value, limit).is_err(), "limit {limit}");
    }
    let (bytes, record) = write_canonical_record(&value, expected.len()).unwrap();
    assert_eq!(bytes.as_ref(), expected.as_bytes());
    assert_eq!(record.into_raw_slices().concat(), expected.as_bytes());

    let mut writer = ValueWriter {
        bytes: Vec::new(),
        fragments: Vec::new(),
        limit: expected.len(),
    };
    writer.append(b"data: ").unwrap();
    writer.value(&value, WriteContext::Root).unwrap();
    writer.append(b"\n\n").unwrap();
    assert_eq!(writer.bytes, expected.as_bytes());
    assert_eq!(writer.fragments.len(), 3);
    for fragment in writer.fragments {
        assert_eq!(fragment.bucket.choice_index, 2);
        let expected_text = match fragment.bucket.field {
            LogicalField::Content => content,
            LogicalField::ReasoningContent => reasoning,
            LogicalField::ToolArguments { tool_call_index } => {
                assert_eq!(tool_call_index, 3);
                arguments
            }
        };
        let SemanticSource::JsonStringTokenRanges(ranges) = fragment.source else {
            panic!("expected JSON token range");
        };
        assert_eq!(ranges.len(), 1);
        let range = ranges[0];
        let decoded: String = serde_json::from_slice(&writer.bytes[range.start..range.end]).unwrap();
        assert_eq!(decoded, expected_text);
    }
}

#[test]
fn canonical_scalar_serialization_needs_no_temporary_buffer() {
    let value = Value::String("é\"\\\n".repeat(256));
    let expected = serde_json::to_vec(&value).unwrap();
    let mut writer = ValueWriter {
        bytes: Vec::with_capacity(expected.len()),
        fragments: Vec::new(),
        limit: expected.len(),
    };
    let (result, allocations) =
        crate::test_allocation_counter::count_allocations(|| writer.value(&value, WriteContext::Other));
    assert!(result.is_ok());
    assert_eq!(writer.bytes, expected);
    assert_eq!(allocations, 0);
}

#[test]
fn batch_raw_materialization_allocates_only_its_final_output() {
    let (empty, allocations) =
        crate::test_allocation_counter::count_allocations(|| CanonicalRecordBatch::new().into_raw_bytes());
    assert!(empty.unwrap().is_empty());
    assert_eq!(allocations, 0);

    // Zero selects annotated records; two selects sliced fallback. A count
    // above the batch record limit also exceeds the chain's handle threshold.
    for fallback_pieces in [0, 2, MAX_CANONICAL_BATCH_RECORDS + 1] {
        let mut batch = CanonicalRecordBatch::new();
        if fallback_pieces != 0 {
            for _ in 0..MAX_CANONICAL_BATCH_RECORDS {
                batch.push(WindowRecord::raw(Vec::new())).unwrap();
            }
        }
        let mut expected = Vec::new();
        for index in 0..fallback_pieces.max(2) {
            let piece = if index % 2 == 0 {
                b"a".as_slice()
            } else {
                b"bc".as_slice()
            };
            batch.push(WindowRecord::raw(vec![Bytes::from_static(piece)])).unwrap();
            expected.extend_from_slice(piece);
        }
        assert_eq!(
            batch.status(),
            if fallback_pieces == 0 {
                CanonicalSourceStatus::Available
            } else {
                CanonicalSourceStatus::Unavailable
            }
        );
        let (raw, allocations) = crate::test_allocation_counter::count_allocations(|| batch.into_raw_bytes());
        assert_eq!(raw.unwrap().as_ref(), expected);
        assert_eq!(allocations, 1, "fallback pieces: {fallback_pieces}");
    }
}
