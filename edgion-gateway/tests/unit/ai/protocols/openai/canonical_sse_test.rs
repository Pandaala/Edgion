use super::*;
use crate::sse::SseDecoder;

fn observe(raw: &'static [u8]) -> CanonicalEventObservation {
    let mut decoder = SseDecoder::with_default_limit();
    let mut result = None;
    decoder.feed_each_bytes(Bytes::from_static(raw), |event| {
        result = Some(observe_canonical_event(&event))
    });
    result.unwrap()
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
