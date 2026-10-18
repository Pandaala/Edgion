use super::*;
use proptest::prelude::*;

fn decode_all(chunks: &[&[u8]]) -> (Vec<SseEvent>, bool) {
    let mut decoder = SseDecoder::with_default_limit();
    let mut events = Vec::new();
    for chunk in chunks {
        decoder.feed(chunk, &mut events);
    }
    (events, decoder.degraded())
}

#[test]
fn single_event_single_chunk() {
    let (events, degraded) = decode_all(&[b"data: hello\n\n"]);
    assert!(!degraded);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "hello");
    assert_eq!(events[0].event, None);
}

#[test]
fn event_split_across_chunks_mid_line_is_reassembled() {
    let (events, _) = decode_all(&[b"data: {\"a\"", b":1}\n\n"]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "{\"a\":1}");
}

#[test]
fn leading_utf8_bom_is_stripped() {
    let (events, degraded) = decode_all(&[b"\xef\xbb\xbfdata: hello\n\n"]);
    assert!(!degraded);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "hello");
}

#[test]
fn leading_utf8_bom_split_across_chunks_is_stripped() {
    let (events, degraded) = decode_all(&[b"\xef", b"\xbb", b"\xbfdata: hello\n\n"]);
    assert!(!degraded);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "hello");
}

#[test]
fn later_utf8_bom_is_ordinary_data() {
    let (events, degraded) = decode_all(&[b"data: first\n\ndata: \xef\xbb\xbfsecond\n\n"]);
    assert!(!degraded);
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].data.as_bytes(), b"\xef\xbb\xbfsecond");
}

#[test]
fn non_bom_prefix_is_replayed_through_the_event_limit() {
    let mut decoder = SseDecoder::new(2);
    let mut events = Vec::new();

    decoder.feed(b"\xef\xbb", &mut events);
    assert!(!decoder.degraded());
    decoder.feed(b"x", &mut events);

    assert!(decoder.degraded());
    assert!(events.is_empty());
}

#[test]
fn single_chunk_bom_replay_degradation_preserves_ordered_remainder() {
    let input = Bytes::from_static(b"\xef\xbbxremaining");
    let mut decoder = SseDecoder::new(2);
    let mut raw = Vec::new();

    collect_raw(&mut decoder, input.clone(), &mut raw);

    assert!(decoder.degraded());
    assert_eq!(raw, input);
}

#[test]
fn cross_chunk_bom_replay_degradation_preserves_ordered_remainder() {
    let mut decoder = SseDecoder::new(2);
    let mut raw = Vec::new();

    collect_raw(&mut decoder, Bytes::from_static(b"\xef\xbb"), &mut raw);
    collect_raw(&mut decoder, Bytes::from_static(b"xremaining"), &mut raw);

    assert!(decoder.degraded());
    assert_eq!(raw, b"\xef\xbbxremaining");
}

#[test]
fn crlf_line_endings_and_split_between_cr_and_lf() {
    let (events, _) = decode_all(&[b"data: a\r", b"\n\r\n"]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "a");
}

#[test]
fn multiple_events_in_one_chunk() {
    let (events, _) = decode_all(&[b"data: one\n\ndata: two\n\n"]);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].data, "one");
    assert_eq!(events[1].data, "two");
}

#[test]
fn feed_each_visits_many_events_without_a_batch() {
    let mut decoder = SseDecoder::with_default_limit();
    let chunk = "data: x\n\n".repeat(10_000);
    let mut count = 0;
    decoder.feed_each(chunk.as_bytes(), |event| {
        assert_eq!(event.data, "x");
        count += 1;
    });
    assert_eq!(count, 10_000);
    assert!(!decoder.degraded());
}

#[test]
fn event_type_id_and_multiline_data() {
    let (events, _) = decode_all(&[b"event: delta\nid: 7\ndata: l1\ndata: l2\n\n"]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.as_deref(), Some("delta"));
    assert_eq!(events[0].id.as_deref(), Some("7"));
    assert_eq!(events[0].data, "l1\nl2");
}

#[test]
fn comment_lines_and_dataless_blocks_dispatch_nothing() {
    let (events, _) = decode_all(&[b": keepalive\n\nevent: noop\n\n"]);
    assert!(events.is_empty());
}

#[test]
fn empty_data_fields_dispatch_per_the_eventsource_algorithm() {
    let (events, degraded) = decode_all(&[
        b"data:\n\n",
        b"data: \n\n",
        b"data\n\n",
        b"data:\ndata:\n\n",
        b"event: ping\n\n",
        b"data:\n",
    ]);

    assert!(!degraded);
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].data, "");
    assert_eq!(events[1].data, "");
    assert_eq!(events[2].data, "");
    assert_eq!(events[3].data, "\n");
}

#[test]
fn done_sentinel_is_an_ordinary_data_event() {
    let (events, _) = decode_all(&[b"data: [DONE]\n\n"]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "[DONE]");
}

#[test]
fn space_after_colon_is_optional() {
    let (events, _) = decode_all(&[b"data:tight\n\n"]);
    assert_eq!(events[0].data, "tight");
}

#[test]
fn oversized_partial_line_degrades_permanently() {
    let mut decoder = SseDecoder::new(1024);
    let mut events = Vec::new();
    decoder.feed(&vec![b'x'; 2048], &mut events);
    assert!(decoder.degraded());
    assert!(events.is_empty());

    decoder.feed(b"data: late\n\n", &mut events);
    assert!(events.is_empty());
    assert!(decoder.degraded());
}

#[test]
fn many_complete_lines_without_dispatch_are_bounded() {
    let mut decoder = SseDecoder::new(128);
    let mut events = Vec::new();

    for _ in 0..32 {
        decoder.feed(b"data: x\n", &mut events);
        assert!(decoder.degraded() || decoder.block.raw.len() <= decoder.max_pending_bytes);
    }

    assert!(decoder.degraded());
    assert!(events.is_empty());
    assert_eq!(decoder.block.raw.len(), 0);
}

#[test]
fn crlf_bytes_count_toward_the_whole_event_limit() {
    let mut decoder = SseDecoder::new(8);
    let mut events = Vec::new();

    decoder.feed(b"data:x\r", &mut events);
    assert!(!decoder.degraded());
    assert_eq!(decoder.block.raw.len(), 7);

    decoder.feed(b"\n", &mut events);
    assert!(!decoder.degraded());
    assert_eq!(decoder.block.raw.len(), 8);

    decoder.feed(b"x", &mut events);
    assert!(decoder.degraded());
}

#[test]
fn invalid_utf8_in_a_field_line_is_skipped_not_fatal() {
    let mut decoder = SseDecoder::with_default_limit();
    let mut events = Vec::new();
    decoder.feed(b"data: \xff\xfe\n\ndata: ok\n\n", &mut events);
    let degraded = decoder.degraded();
    assert!(!degraded);
    assert!(decoder.malformed());
    assert_eq!(decoder.malformed_events(), 1);
    assert!(decoder.is_idle());
    assert_eq!(events.len(), 1, "the malformed event is dropped, the stream continues");
    assert_eq!(events[0].data, "ok");
}

#[test]
fn multiple_invalid_lines_poison_one_completed_event_once() {
    let mut decoder = SseDecoder::with_default_limit();
    let mut events = Vec::new();

    decoder.feed(b"data: \xff\ndata: \xfe\nevent: \xff\n\n", &mut events);

    assert!(events.is_empty());
    assert!(decoder.malformed());
    assert_eq!(decoder.malformed_events(), 1);
}

#[test]
fn two_poisoned_events_in_one_chunk_are_counted_separately() {
    let mut decoder = SseDecoder::with_default_limit();
    let mut events = Vec::new();

    decoder.feed(b"data: \xff\n\ndata: \xfe\n\n", &mut events);

    assert!(events.is_empty());
    assert_eq!(decoder.malformed_events(), 2);
}

#[test]
fn split_invalid_line_counts_only_when_its_event_completes() {
    let mut decoder = SseDecoder::with_default_limit();
    let mut events = Vec::new();

    decoder.feed(b"data: \xc3", &mut events);
    assert_eq!(decoder.malformed_events(), 0);
    decoder.feed(b"\x28\n", &mut events);
    assert!(decoder.malformed());
    assert_eq!(decoder.malformed_events(), 0);
    decoder.feed(b"\n", &mut events);

    assert!(events.is_empty());
    assert_eq!(decoder.malformed_events(), 1);
}

#[test]
fn valid_event_does_not_reset_cumulative_malformed_count() {
    let mut decoder = SseDecoder::with_default_limit();
    let mut events = Vec::new();

    decoder.feed(b"data: \xff\n\ndata: valid\n\ndata: \xfe\n\n", &mut events);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "valid");
    assert!(decoder.malformed());
    assert_eq!(decoder.malformed_events(), 2);
}

#[test]
fn idle_reports_incomplete_event_state() {
    let mut decoder = SseDecoder::with_default_limit();
    let mut events = Vec::new();
    decoder.feed(b"data: partial", &mut events);
    assert!(!decoder.is_idle());
    decoder.feed(b"\n\n", &mut events);
    assert!(decoder.is_idle());
}

fn collect_raw(decoder: &mut SseDecoder, chunk: Bytes, output: &mut Vec<u8>) {
    decoder.feed_records(chunk, |record| match record {
        SseRecordRef::Preamble(bytes) | SseRecordRef::FramingContinuation(bytes) => output.extend_from_slice(bytes),
        SseRecordRef::Block { event, .. } => event.raw().for_each_slice(|bytes| output.extend_from_slice(bytes)),
        SseRecordRef::EosTail(raw) | SseRecordRef::Opaque(raw) => {
            raw.for_each_slice(|bytes| output.extend_from_slice(bytes))
        }
    });
}

#[test]
fn raw_records_preserve_every_byte_for_all_two_and_three_way_splits() {
    let input = b"\xef\xbb\xbf: keep\r\ndata: h\xc3\xa9\r\nunknown: x\r\n\r\ndata: \xff\n\ntail";
    for first in 0..=input.len() {
        for second in first..=input.len() {
            let mut decoder = SseDecoder::with_default_limit();
            let mut raw = Vec::new();
            for part in [&input[..first], &input[first..second], &input[second..]] {
                collect_raw(&mut decoder, Bytes::copy_from_slice(part), &mut raw);
            }
            decoder.finish_records(|record| match record {
                SseRecordRef::EosTail(bytes) | SseRecordRef::Opaque(bytes) => {
                    bytes.for_each_slice(|part| raw.extend_from_slice(part))
                }
                _ => {}
            });
            assert_eq!(raw, input, "first={first}, second={second}");
        }
    }
}

proptest! {
    #[test]
    fn arbitrary_bytes_and_chunking_preserve_raw_identity(
        input in prop::collection::vec(any::<u8>(), 0..4096),
        chunk_sizes in prop::collection::vec(1usize..128, 1..64),
    ) {
        let mut decoder = SseDecoder::with_default_limit();
        let mut raw = Vec::new();
        let mut offset = 0;
        let mut sizes = chunk_sizes.into_iter().cycle();
        while offset < input.len() {
            let end = offset.saturating_add(sizes.next().unwrap()).min(input.len());
            collect_raw(&mut decoder, Bytes::copy_from_slice(&input[offset..end]), &mut raw);
            offset = end;
        }
        decoder.finish_records(|record| match record {
            SseRecordRef::EosTail(bytes) | SseRecordRef::Opaque(bytes) => {
                bytes.for_each_slice(|part| raw.extend_from_slice(part));
            }
            _ => {}
        });
        prop_assert_eq!(raw, input);
    }
}

#[test]
fn one_byte_incomplete_event_coalesces_once_and_stays_bounded() {
    let mut decoder = SseDecoder::with_default_limit();
    for byte in std::iter::repeat_n(b'x', 4096) {
        decoder.feed_each_bytes(Bytes::from(vec![byte]), |_| {});
    }
    assert_eq!(decoder.block.raw.coalesce_count(), 1);
    assert_eq!(decoder.block.raw.handle_count(), 1);
    assert!(decoder.block.raw.resident_capacity() <= DEFAULT_MAX_PENDING_EVENT_BYTES);
    assert!(decoder.block.raw.copied_bytes() <= decoder.block.raw.len());
}

#[test]
fn data_character_counting_is_opt_in_and_uses_parser_boundaries() {
    let mut unused = SseDecoder::with_default_limit();
    unused.feed_each_bytes(Bytes::from_static(b"data: ignored\n\n"), |_| {});
    assert_eq!(unused.take_data_chars(), 0);

    let mut decoder = SseDecoder::with_default_limit();
    decoder.enable_data_char_counting();
    decoder.feed_each_bytes(Bytes::from_static(b"data: h\xc3"), |_| {});
    assert_eq!(
        decoder.take_data_chars(),
        2,
        "h and the leading UTF-8 byte are observed incrementally"
    );
    decoder.feed_each_bytes(Bytes::from_static(b"\xa9\r"), |_| {});
    decoder.feed_each_bytes(Bytes::from_static(b"\ndata: x\n\n"), |_| {});
    assert_eq!(
        decoder.take_data_chars(),
        2,
        "the continuation completes the prior scalar; the second data line adds a semantic newline and x"
    );
}

#[test]
fn metadata_charge_accepts_exact_limit_and_rejects_limit_plus_one() {
    let range_size = std::mem::size_of::<ByteRange>();
    let line_capacity = MAX_EVENT_METADATA_BYTES / range_size;
    assert_eq!(decoder_metadata_charge(0, line_capacity, 0), MAX_EVENT_METADATA_BYTES);
    assert!(decoder_metadata_charge(1, line_capacity, 0) > MAX_EVENT_METADATA_BYTES);
}

#[test]
fn metadata_accounts_for_handles_lines_and_parsed_data_ranges() {
    let mut decoder = SseDecoder::with_default_limit();
    for _ in 0..MAX_EVENT_LINES {
        if decoder.degraded() {
            break;
        }
        decoder.feed_each_bytes(Bytes::from_static(b"data: x\n"), |_| {});
        if !decoder.degraded() {
            let charge = decoder_metadata_charge(
                decoder.block.raw.slice_metadata_capacity(),
                decoder.block.lines.capacity(),
                decoder.block.data_lines,
            );
            assert!(charge <= MAX_EVENT_METADATA_BYTES);
            assert!(
                decoder.block.lines.len()
                    + decoder.block.data_lines
                    + usize::from(decoder.block.has_event)
                    + usize::from(decoder.block.has_id)
                    <= MAX_EVENT_RANGES
            );
        }
    }
    assert!(
        decoder.degraded(),
        "combined metadata/range pressure must degrade before it can exceed a cap"
    );
}

#[test]
fn literal_checks_preserve_event_and_data_semantics_across_every_split() {
    let fixtures: &[&[u8]] = &[
        b"event: message_start\ndata: [DONE]\n\n",
        b"event: message_start\nevent: message_delta\ndata: [DONE]\n\n",
        b"event: message_stop\ndata: [DONE]\n\n",
        b"event: error\ndata: [DONE]\n\n",
        b"event: Message_Start\ndata: [DONE]\n\n",
        b"event:\ndata: [DONE]\n\n",
        b"data: [DONE]\n\n",
        "event: é\ndata: [DONE]\n\n".as_bytes(),
        b"event: \xff\ndata: [DONE]\n\n",
        b"event: error\ndata: [DONE]\ndata:\n\n",
    ];
    for input in fixtures {
        for split in 0..=input.len() {
            let mut decoder = SseDecoder::with_default_limit();
            let mut blocks = 0;
            for part in [&input[..split], &input[split..]] {
                decoder.feed_records(Bytes::copy_from_slice(part), |record| {
                    let SseRecordRef::Block { event, .. } = record else {
                        return;
                    };
                    blocks += 1;
                    for literal in [
                        "message_start",
                        "message_delta",
                        "message_stop",
                        "error",
                        "Message_Start",
                        "",
                        "é",
                    ] {
                        let expected = event.event().as_deref() == Some(literal);
                        let (matched, allocations) =
                            crate::test_allocation_counter::count_allocations(|| event.event_is(literal));
                        assert_eq!(matched, expected);
                        assert_eq!(allocations, 0);
                    }
                    let expected = event.data.len() == 1
                        && event
                            .raw
                            .range(event.data[0])
                            .is_some_and(|value| value.as_ref() == b"[DONE]");
                    let (matched, allocations) =
                        crate::test_allocation_counter::count_allocations(|| event.data_is_literal(b"[DONE]"));
                    assert_eq!(matched, expected);
                    assert_eq!(allocations, 0);
                });
            }
            assert_eq!(blocks, 1);
        }
    }
}
