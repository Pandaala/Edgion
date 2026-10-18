#![allow(clippy::field_reassign_with_default)]

use super::super::SseDecoder;
use super::*;

#[test]
fn binary_message_borrows_contiguous_storage_and_copies_fragments() {
    let empty = RawWindow::new();
    assert!(matches!(empty.binary_bytes().unwrap(), Cow::Borrowed(bytes) if bytes.is_empty()));

    let mut single = RawWindow::new();
    single
        .retain(WindowRecord::binary(Bytes::from_static(b"payload")))
        .unwrap();
    let mut pointer = std::ptr::null();
    single
        .raw
        .for_each_range(ByteRange::new(0, 7), |part| pointer = part.as_ptr());
    let (view, stats) = crate::test_allocation_counter::count_allocation_stats(|| single.binary_bytes());
    let view = view.unwrap();
    assert!(matches!(view, Cow::Borrowed(_)));
    assert_eq!(view.as_ref(), b"payload");
    assert_eq!(view.as_ptr(), pointer);
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);

    let mut fragmented = RawWindow::new();
    for part in [b"pay".as_slice(), b"load".as_slice()] {
        fragmented
            .retain(WindowRecord::binary(Bytes::copy_from_slice(part)))
            .unwrap();
    }
    assert!(matches!(fragmented.binary_bytes().unwrap(), Cow::Owned(bytes) if bytes == b"payload"));

    let mut coalesced = RawWindow::new();
    for _ in 0..=MAX_SLICE_HANDLES {
        coalesced
            .retain(WindowRecord::binary(Bytes::from_static(b"a")))
            .unwrap();
    }
    assert_eq!(coalesced.raw.coalesce_count(), 1);
    let (view, stats) = crate::test_allocation_counter::count_allocation_stats(|| coalesced.binary_bytes());
    let view = view.unwrap();
    assert!(matches!(view, Cow::Borrowed(_)));
    assert_eq!(view.len(), MAX_SLICE_HANDLES + 1);
    assert!(view.iter().all(|byte| *byte == b'a'));
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
}

#[test]
fn binary_message_borrow_retains_admission_and_state_gates() {
    let mut window = RawWindow::new();
    window
        .retain(WindowRecord::binary(Bytes::from_static(b"payload")))
        .unwrap();
    let admitted = window.resident().unwrap() + window.raw.len();
    window.limits.combined = admitted - 1;
    assert_eq!(
        window.binary_bytes(),
        Err(WindowError::Limit(WindowLimit::CombinedResidentBytes))
    );
    window.limits.combined = admitted;
    assert_eq!(window.binary_bytes().unwrap().as_ref(), b"payload");
    window.resolve(WindowDecision::Discard).unwrap();
    assert_eq!(window.binary_bytes(), Err(WindowError::NotCollecting));
    window.start_next().unwrap();
    window.finish(WindowDecision::Discard).unwrap();
    assert_eq!(window.binary_bytes(), Err(WindowError::Finished));
}

fn key(choice: u32, field: LogicalField) -> LogicalBucketKey {
    LogicalBucketKey {
        choice_index: choice,
        field,
    }
}

fn fragment(bucket: LogicalBucketKey, ranges: Vec<ByteRange>) -> SemanticFragment {
    SemanticFragment {
        bucket,
        source: SemanticSource::Utf8Ranges(ranges),
    }
}

fn event(raw: Bytes, bucket: LogicalBucketKey, ranges: Vec<ByteRange>) -> WindowRecord {
    WindowRecord::raw(vec![raw])
        .complete()
        .with_fragments(vec![fragment(bucket, ranges)])
}

fn released(output: WindowOutput) -> Vec<u8> {
    match output {
        WindowOutput::Release(chunks) => chunks.into_iter().flatten().collect(),
        output => panic!("expected release, got {output:?}"),
    }
}

#[test]
fn raw_slice_iteration_preserves_handles_without_a_temporary_vector() {
    let source = [Bytes::from_static(b"first"), Bytes::new(), Bytes::from_static(b"last")];
    let pointers = source.each_ref().map(|bytes| bytes.as_ptr());
    let record = WindowRecord::raw(source.to_vec());
    let ((output, exhausted), stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        let mut slices = record.raw_slices_cloned().into_iter();
        let output: [Bytes; 3] = std::array::from_fn(|_| slices.next().unwrap());
        (output, slices.next().is_none())
    });
    assert!(exhausted);
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
    assert_eq!(record.slices, source);
    assert_eq!(output.each_ref().map(|bytes| bytes.as_ptr()), pointers);
    drop(record);
    assert_eq!(output, source);
}

#[test]
fn decoder_adapter_releases_every_record_byte_identically() {
    let source = Bytes::from_static(b"\xef\xbb\xbf: c\r\ndata: one\r\n\r\ndata: two\n\n");
    for split in 0..=source.len() {
        let mut decoder = SseDecoder::with_default_limit();
        let mut window = RawWindow::new();
        decoder.feed_records(source.slice(..split), |record| {
            window.retain(WindowRecord::from_sse_record(record)).unwrap()
        });
        decoder.feed_records(source.slice(split..), |record| {
            window.retain(WindowRecord::from_sse_record(record)).unwrap()
        });
        decoder.finish_records(|record| window.retain(WindowRecord::from_sse_record(record)).unwrap());
        assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), source);
    }
}

#[test]
fn raw_identity_and_cross_slice_semantics() {
    let raw = Bytes::from_static("a€𐍈".as_bytes());
    for split in 0..=raw.len() {
        let bucket = key(0, LogicalField::Content);
        let mut window = RawWindow::new();
        window
            .retain(
                WindowRecord::raw(vec![raw.slice(..split), raw.slice(split..)])
                    .with_fragments(vec![fragment(bucket, vec![ByteRange::new(0, raw.len())])]),
            )
            .unwrap();
        assert_eq!(window.snapshot(bucket).unwrap().materialize().unwrap(), "a€𐍈");
        assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), raw);
    }
}

#[test]
fn json_escape_semantics_split_at_every_byte() {
    let raw = Bytes::from_static(br#""h\u00e9\ud83d\ude00\\\"""#);
    for split in 0..=raw.len() {
        let bucket = key(0, LogicalField::Content);
        let mut window = RawWindow::new();
        let source = SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(0, raw.len())]);
        window
            .retain(
                WindowRecord::raw(vec![raw.slice(..split), raw.slice(split..)])
                    .with_fragments(vec![SemanticFragment { bucket, source }]),
            )
            .unwrap();
        assert_eq!(window.snapshot(bucket).unwrap().materialize().unwrap(), "hé😀\\\"");
    }
}

#[test]
fn semantic_failure_permits_atomic_raw_only_release_and_next_generation() {
    for invalid in [Bytes::from_static(b"\xff"), Bytes::from_static(b"not-json")] {
        let bucket = key(0, LogicalField::Content);
        let source = if invalid[0] == 0xff {
            SemanticSource::Utf8Ranges(vec![ByteRange::new(0, invalid.len())])
        } else {
            SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(0, invalid.len())])
        };
        let mut window = RawWindow::new();
        window
            .retain(WindowRecord::raw(vec![invalid.clone()]).with_fragments(vec![SemanticFragment { bucket, source }]))
            .unwrap();
        let expected = window.snapshot(bucket).unwrap().materialize().unwrap_err();
        assert_eq!(
            window.resolve(WindowDecision::Replace(Bytes::from_static(b"replacement"))),
            Err(expected)
        );
        assert_eq!(window.resolve(WindowDecision::Discard).unwrap(), WindowOutput::Discard);
        window.start_next().unwrap();
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"next")]))
            .unwrap();
        assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"next");
    }
}

#[test]
fn discard_never_materializes_semantics_or_prepares_overlap() {
    let bucket = key(0, LogicalField::Content);
    let invalid = Bytes::from_static(b"\xff");
    let mut overlap = RawWindow::new();
    overlap
        .retain(
            WindowRecord::raw(vec![invalid.clone()]).with_fragments(vec![fragment(bucket, vec![ByteRange::new(0, 1)])]),
        )
        .unwrap();
    assert_eq!(overlap.resolve(WindowDecision::Discard).unwrap(), WindowOutput::Discard);
    overlap.start_next().unwrap();

    let raw = Bytes::from_static(b"value");
    let mut combined = RawWindow::new();
    combined
        .retain(event(raw.clone(), bucket, vec![ByteRange::new(0, raw.len())]))
        .unwrap();
    combined.limits.combined = combined.raw.resident_capacity();
    assert_eq!(
        combined.snapshot(bucket).unwrap().materialize(),
        Err(WindowError::Limit(WindowLimit::CombinedResidentBytes))
    );
    assert_eq!(
        combined.resolve(WindowDecision::Discard).unwrap(),
        WindowOutput::Discard
    );
    combined.start_next().unwrap();
}

#[test]
fn one_byte_stream_coalesces_once_and_is_linear() {
    let mut limits = Limits::default();
    limits.ranges = usize::MAX;
    limits.lines = usize::MAX;
    limits.events = usize::MAX;
    limits.metadata = usize::MAX;
    limits.combined = usize::MAX;
    let mut window = RawWindow::with_limits(limits);
    let bytes = MAX_SLICE_HANDLES + 10_000;
    for _ in 0..bytes {
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
            .unwrap();
    }
    let (_, _, handles, coalesces, copied) = window.accounting();
    assert_eq!(handles, 1);
    assert_eq!(coalesces, 1);
    assert_eq!(copied, bytes);
    assert_eq!(
        released(window.resolve(WindowDecision::Release).unwrap()),
        vec![b'x'; bytes]
    );
}

#[test]
fn one_large_record_coalesces_without_unbounded_retained_metadata() {
    let count = MAX_SLICE_HANDLES + 1;
    let mut window = RawWindow::new();
    window
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"x"); count]))
        .unwrap();
    let (logical, resident, handles, coalesces, copied) = window.accounting();
    assert_eq!(logical, count);
    assert_eq!(handles, 1);
    assert_eq!(coalesces, 1);
    assert_eq!(copied, count);
    assert_eq!(window.raw.slice_metadata_capacity(), 0);
    assert!(resident <= MAX_RESIDENT_BYTES);
    let before_empty_append = window.accounting();
    window.retain(WindowRecord::raw(vec![Bytes::new(); count])).unwrap();
    assert_eq!(window.accounting(), before_empty_append);
    assert_eq!(
        released(window.resolve(WindowDecision::Release).unwrap()),
        vec![b'x'; count]
    );

    let mut empty = RawWindow::new();
    empty.retain(WindowRecord::raw(vec![Bytes::new(); count])).unwrap();
    assert_eq!(empty.accounting(), (0, 0, 0, 0, 0));
    assert_eq!(released(empty.resolve(WindowDecision::Release).unwrap()), b"");
}

#[test]
fn oversized_input_handle_tables_are_rejected_without_mutation() {
    for bytes in [Bytes::from_static(b"x"), Bytes::new()] {
        let mut window = RawWindow::new();
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"prior")]))
            .unwrap();
        let record = WindowRecord::raw(vec![bytes; 10_000]);
        let pointer = record.slices.as_ptr();
        let (error, record) = window.try_retain(record).unwrap_err();
        assert_eq!(error, WindowError::Limit(WindowLimit::MetadataBytes));
        assert_eq!(record.slices.as_ptr(), pointer);
        assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), b"prior");
    }
}

#[test]
fn ranges_are_counted_from_actual_fragments_and_failure_is_atomic() {
    let mut limits = Limits::default();
    limits.ranges = 3; // one raw record plus two semantic ranges
    let bucket = key(0, LogicalField::Content);
    let mut window = RawWindow::with_limits(limits);
    window
        .retain(event(
            Bytes::from_static(b"ab"),
            bucket,
            vec![ByteRange::new(0, 1), ByteRange::new(1, 2)],
        ))
        .unwrap();
    let before = window.accounting();
    assert_eq!(
        window.retain(event(Bytes::from_static(b"c"), bucket, vec![ByteRange::new(0, 1)])),
        Err(WindowError::Limit(WindowLimit::RawRanges))
    );
    assert_eq!(window.accounting(), before);
    assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"ab");
}

#[test]
fn count_limits_accept_exact_and_reject_plus_one_atomically() {
    let mut raw_limits = Limits::default();
    raw_limits.raw = 1;
    raw_limits.resident = 1;
    raw_limits.combined = 128 * 1024;
    let mut raw = RawWindow::with_limits(raw_limits);
    raw.retain(WindowRecord::raw(vec![Bytes::from_static(b"x")])).unwrap();
    assert_eq!(
        raw.retain(WindowRecord::raw(vec![Bytes::from_static(b"y")])),
        Err(WindowError::Limit(WindowLimit::RawBytes))
    );
    assert_eq!(released(raw.resolve(WindowDecision::Release).unwrap()), b"x");

    let mut resident_limits = Limits::default();
    resident_limits.resident = 1;
    resident_limits.combined = 1024 * 1024;
    let mut resident = RawWindow::with_limits(resident_limits);
    resident
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
        .unwrap();
    assert_eq!(
        resident.retain(WindowRecord::raw(vec![Bytes::from_static(b"y")])),
        Err(WindowError::Limit(WindowLimit::ResidentBytes))
    );
    assert_eq!(released(resident.resolve(WindowDecision::Release).unwrap()), b"x");

    for (line_limit, event_limit, expected) in [
        (1, usize::MAX, WindowLimit::Lines),
        (usize::MAX, 1, WindowLimit::Events),
    ] {
        let mut limits = Limits::default();
        limits.lines = line_limit;
        limits.events = event_limit;
        let mut window = RawWindow::with_limits(limits);
        let record = if expected == WindowLimit::Lines {
            WindowRecord::raw(vec![Bytes::from_static(b"a\n")])
        } else {
            WindowRecord::raw(vec![Bytes::from_static(b"a")]).complete()
        };
        window.retain(record).unwrap();
        let extra = if expected == WindowLimit::Lines {
            WindowRecord::raw(vec![Bytes::from_static(b"b\n")])
        } else {
            WindowRecord::raw(vec![Bytes::from_static(b"b")]).complete()
        };
        assert_eq!(window.retain(extra), Err(WindowError::Limit(expected)));
    }
}

#[test]
fn handle_limit_exact_then_one_time_coalesce() {
    let mut window = RawWindow::new();
    window
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"x"); MAX_SLICE_HANDLES]))
        .unwrap();
    assert_eq!(window.accounting().2, MAX_SLICE_HANDLES);
    assert_eq!(window.accounting().3, 0);
    window
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
        .unwrap();
    assert_eq!(window.accounting().2, 1);
    assert_eq!(window.accounting().3, 1);
}

#[test]
fn bucket_fragment_metadata_snapshot_overlap_and_replacement_bounds() {
    let a = key(0, LogicalField::Content);
    let b = key(0, LogicalField::ToolArguments { tool_call_index: 1 });
    let mut limits = Limits::default();
    limits.buckets = 1;
    limits.fragments = 1;
    let mut window = RawWindow::with_limits(limits);
    window
        .retain(event(Bytes::from_static(b"a"), a, vec![ByteRange::new(0, 1)]))
        .unwrap();
    assert_eq!(
        window.retain(event(Bytes::from_static(b"b"), a, vec![ByteRange::new(0, 1)])),
        Err(WindowError::Limit(WindowLimit::FragmentsPerBucket))
    );
    assert_eq!(
        window.retain(event(Bytes::from_static(b"b"), b, vec![ByteRange::new(0, 1)])),
        Err(WindowError::Limit(WindowLimit::Buckets))
    );

    let mut limits = Limits::default();
    limits.snapshot = 3;
    let mut snapshot = RawWindow::with_limits(limits);
    snapshot
        .retain(event(Bytes::from_static(b"four"), a, vec![ByteRange::new(0, 4)]))
        .unwrap();
    assert_eq!(
        snapshot.snapshot(a).unwrap().materialize(),
        Err(WindowError::Limit(WindowLimit::SnapshotBytes))
    );
    assert_eq!(released(snapshot.resolve(WindowDecision::Release).unwrap()), b"four");

    let mut limits = Limits::default();
    limits.snapshot = 4;
    let mut snapshot_exact = RawWindow::with_limits(limits);
    snapshot_exact
        .retain(event(Bytes::from_static(b"four"), a, vec![ByteRange::new(0, 4)]))
        .unwrap();
    assert_eq!(snapshot_exact.snapshot(a).unwrap().materialize().unwrap(), "four");

    let mut limits = Limits::default();
    limits.replacement = 3;
    let mut replacement = RawWindow::with_limits(limits);
    assert_eq!(
        replacement
            .resolve(WindowDecision::Replace(Bytes::from_static(b"123")))
            .unwrap(),
        WindowOutput::Replace(Bytes::from_static(b"123"))
    );
    replacement.start_next().unwrap();
    assert_eq!(
        replacement.resolve(WindowDecision::Replace(Bytes::from_static(b"1234"))),
        Err(WindowError::Limit(WindowLimit::ReplacementBytes))
    );
    assert_eq!(
        replacement.resolve(WindowDecision::Discard).unwrap(),
        WindowOutput::Discard
    );

    let mut limits = Limits::default();
    limits.metadata = 1;
    limits.combined = 1024;
    let mut metadata = RawWindow::with_limits(limits);
    assert_eq!(
        metadata.retain(WindowRecord::raw(vec![Bytes::from_static(b"x")])),
        Err(WindowError::Limit(WindowLimit::MetadataBytes))
    );

    let probe = SliceChain::default()
        .inspect_append(&[Bytes::from_static(b"x")], MAX_RAW_BYTES, MAX_RESIDENT_BYTES)
        .unwrap();
    let exact_metadata = probe.metadata + size_of::<Bytes>();
    let mut limits = Limits::default();
    limits.metadata = exact_metadata;
    let mut metadata_exact = RawWindow::with_limits(limits);
    metadata_exact
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
        .unwrap();

    let exact_combined = probe.additional_resident + exact_metadata;
    let mut limits = Limits::default();
    limits.combined = exact_combined;
    let mut combined_exact = RawWindow::with_limits(limits);
    combined_exact
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
        .unwrap();
    let mut limits = Limits::default();
    limits.combined = exact_combined - 1;
    let mut combined_plus_one = RawWindow::with_limits(limits);
    assert_eq!(
        combined_plus_one.retain(WindowRecord::raw(vec![Bytes::from_static(b"x")])),
        Err(WindowError::Limit(WindowLimit::CombinedResidentBytes))
    );
}

#[test]
fn overlap_is_one_global_max_min_utf8_safe_budget() {
    let mut limits = Limits::default();
    limits.overlap = 7;
    let a = key(0, LogicalField::Content);
    let b = key(1, LogicalField::ReasoningContent);
    let mut window = RawWindow::with_limits(limits);
    window
        .retain(event(
            Bytes::from_static("aéé".as_bytes()),
            a,
            vec![ByteRange::new(0, 5)],
        ))
        .unwrap();
    window
        .retain(event(Bytes::from_static(b"123456"), b, vec![ByteRange::new(0, 6)]))
        .unwrap();
    window.resolve(WindowDecision::Release).unwrap();
    window.start_next().unwrap();
    let av = window.snapshot(a).unwrap().materialize().unwrap();
    let bv = window.snapshot(b).unwrap().materialize().unwrap();
    assert_eq!(av, "éé");
    assert_eq!(bv, "456");
    assert!(av.len() + bv.len() <= 7);
}

#[test]
fn configurable_overlap_applies_per_bucket_then_global_fairness() {
    let a = key(0, LogicalField::Content);
    let b = key(1, LogicalField::Content);
    let mut window = RawWindow::with_policy(RawWindowPolicy {
        per_bucket_overlap_bytes: 3,
        global_overlap_bytes: 4,
        max_replacement_bytes: MAX_REPLACEMENT_BYTES,
    })
    .unwrap();
    window
        .retain(event(Bytes::from_static(b"abcdef"), a, vec![ByteRange::new(0, 6)]))
        .unwrap();
    window
        .retain(event(Bytes::from_static(b"123456"), b, vec![ByteRange::new(0, 6)]))
        .unwrap();
    window.resolve(WindowDecision::Release).unwrap();
    window.start_next().unwrap();
    let views = window.snapshot_all().unwrap();
    assert_eq!(views[0].context, "ef");
    assert_eq!(views[1].context, "56");
}

#[test]
fn semantic_replacement_preserves_non_semantic_bytes_and_is_atomic() {
    let key = key(0, LogicalField::Content);
    let raw = Bytes::from_static(
        br#": keep
data: {"choices":[{"index":0,"delta":{"content":"old","metadata":"same"}}]}

"#,
    );
    let token_start = raw.windows(5).position(|part| part == b"\"old\"").unwrap();
    let mut window = RawWindow::new();
    window
        .retain(
            WindowRecord::raw(vec![raw.clone()])
                .complete()
                .with_fragments(vec![SemanticFragment {
                    bucket: key,
                    source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(token_start, token_start + 5)]),
                }]),
        )
        .unwrap();
    assert_eq!(window.stats().unwrap().current_semantic_bytes, 3);
    assert_eq!(window.snapshot_all().unwrap()[0].current, "old");
    assert_eq!(
        window.resolve_semantic_replace(Vec::new()),
        Err(WindowError::InvalidReplacement)
    );
    let output = window
        .resolve_semantic_replace(vec![SemanticReplacement {
            key,
            current: "new\\\"value".into(),
        }])
        .unwrap();
    let WindowOutput::Replace(output) = output else {
        panic!("expected replacement")
    };
    let mut expected = raw[..token_start].to_vec();
    expected.extend_from_slice(br#""new\\\"value""#);
    expected.extend_from_slice(&raw[token_start + 5..]);
    assert_eq!(output.as_ref(), expected);
    window.start_next().unwrap();
    assert_eq!(window.snapshot_all().unwrap()[0].context, "new\\\"value");
}

#[test]
fn failed_try_retain_returns_the_exact_record_for_fresh_generation_retry() {
    let mut limits = Limits::default();
    limits.raw = 1;
    limits.resident = 1;
    let mut window = RawWindow::with_limits(limits);
    window
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"a")]))
        .unwrap();
    let (error, record) = window
        .try_retain(WindowRecord::raw(vec![Bytes::from_static(b"b")]))
        .unwrap_err();
    assert_eq!(error, WindowError::Limit(WindowLimit::RawBytes));
    assert_eq!(record.raw_len(), 1);
    assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"a");
}

#[test]
fn try_retain_charges_original_and_prospective_metadata_exactly() {
    let mut slices = Vec::with_capacity(3);
    slices.push(Bytes::from_static(b"x"));
    let record = WindowRecord::raw(slices);
    let transient = record.metadata_bytes();
    let probe = SliceChain::default()
        .inspect_append(&record.slices, MAX_RAW_BYTES, MAX_RESIDENT_BYTES)
        .unwrap();
    let prospective = probe.additional_metadata;
    let exact = transient.checked_add(prospective).unwrap();

    let mut exact_limits = Limits::default();
    exact_limits.metadata = exact;
    exact_limits.combined = usize::MAX;
    let mut exact_window = RawWindow::with_limits(exact_limits);
    exact_window.try_retain(record.clone()).unwrap();
    assert_eq!(released(exact_window.resolve(WindowDecision::Release).unwrap()), b"x");

    let mut plus_one_limits = Limits::default();
    plus_one_limits.metadata = exact - 1;
    plus_one_limits.combined = usize::MAX;
    let mut plus_one_window = RawWindow::with_limits(plus_one_limits);
    let original_vec_ptr = record.slices.as_ptr();
    let original_bytes_ptr = record.slices[0].as_ptr();
    let original_capacity = record.slices.capacity();
    let (error, returned) = plus_one_window.try_retain(record).unwrap_err();
    assert_eq!(error, WindowError::Limit(WindowLimit::MetadataBytes));
    assert_eq!(returned.slices.as_ptr(), original_vec_ptr);
    assert_eq!(returned.slices[0].as_ptr(), original_bytes_ptr);
    assert_eq!(returned.slices.capacity(), original_capacity);
    assert_eq!(returned.slices[0], Bytes::from_static(b"x"));
    assert_eq!(plus_one_window.accounting().0, 0);
}

#[test]
fn try_retain_many_handles_fails_bounded_and_returns_original_handles() {
    let handle_count = MAX_SLICE_HANDLES + 1;
    let slices = vec![Bytes::from_static(b"x"); handle_count];
    let record = WindowRecord::raw(slices);
    let original_vec_ptr = record.slices.as_ptr();
    let original_handle_ptrs: Vec<_> = record.slices.iter().map(|bytes| bytes.as_ptr()).collect();
    let original_capacity = record.slices.capacity();

    let mut limits = Limits::default();
    limits.metadata = record.metadata_bytes().saturating_sub(1);
    limits.combined = usize::MAX;
    let mut window = RawWindow::with_limits(limits);
    let (error, returned) = window.try_retain(record).unwrap_err();
    assert_eq!(error, WindowError::Limit(WindowLimit::MetadataBytes));
    assert_eq!(returned.slices.as_ptr(), original_vec_ptr);
    assert_eq!(returned.slices.capacity(), original_capacity);
    assert_eq!(returned.slices.len(), handle_count);
    assert!(returned
        .slices
        .iter()
        .zip(original_handle_ptrs)
        .all(|(bytes, pointer)| bytes.as_ptr() == pointer));
    assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"");
}

#[test]
fn replacement_preview_is_non_mutating_and_finish_is_terminal() {
    let key = key(0, LogicalField::Content);
    let raw = Bytes::from_static(
        br#"data: {"choices":[{"index":0,"delta":{"content":"x"}}]}

"#,
    );
    let start = raw.windows(3).position(|part| part == b"\"x\"").unwrap();
    let mut window = RawWindow::new();
    window
        .retain(WindowRecord::raw(vec![raw]).with_fragments(vec![SemanticFragment {
            bucket: key,
            source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(start, start + 3)]),
        }]))
        .unwrap();
    let replacement = vec![SemanticReplacement {
        key,
        current: "exact".into(),
    }];
    let preview = window
        .preview_semantic_replace(replacement.iter().map(|entry| (entry.key, entry.current.as_str())))
        .unwrap();
    assert_eq!(window.snapshot_all().unwrap()[0].current, "x");
    let WindowOutput::Replace(output) = window.finish_semantic_replace(replacement).unwrap() else {
        panic!("expected replacement");
    };
    assert_eq!(preview, output.len());
    assert_eq!(window.start_next(), Err(WindowError::Finished));
}

#[test]
fn zero_text_limit_allows_empty_replacement_without_limiting_encoded_output() {
    let key = key(0, LogicalField::Content);
    let raw = Bytes::from_static(
        br#"data: {"choices":[{"index":0,"delta":{"content":"x"}}]}

"#,
    );
    let start = raw.windows(3).position(|part| part == b"\"x\"").unwrap();
    let mut window = RawWindow::with_policy(RawWindowPolicy {
        per_bucket_overlap_bytes: 0,
        global_overlap_bytes: 0,
        max_replacement_bytes: 0,
    })
    .unwrap();
    window
        .retain(
            WindowRecord::raw(vec![raw.clone()]).with_fragments(vec![SemanticFragment {
                bucket: key,
                source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(start, start + 3)]),
            }]),
        )
        .unwrap();
    let WindowOutput::Replace(output) = window
        .resolve_semantic_replace(vec![SemanticReplacement {
            key,
            current: String::new(),
        }])
        .unwrap()
    else {
        panic!("expected replacement")
    };
    assert_eq!(output.len(), raw.len().saturating_sub(1));
}

#[test]
fn semantic_rewrite_streams_raw_gaps_across_many_slices() {
    let key = key(0, LogicalField::Content);
    let prefix = Bytes::from(vec![b'a'; 300_000]);
    let token = Bytes::from_static(b"\"x\"");
    let suffix = Bytes::from(vec![b'b'; 300_000]);
    let mut window = RawWindow::new();
    window
        .retain(
            WindowRecord::raw(vec![prefix, token, suffix]).with_fragments(vec![SemanticFragment {
                bucket: key,
                source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(300_000, 300_003)]),
            }]),
        )
        .unwrap();
    let WindowOutput::Replace(output) = window
        .resolve_semantic_replace(vec![SemanticReplacement {
            key,
            current: "z".into(),
        }])
        .unwrap()
    else {
        panic!("expected replacement")
    };
    assert_eq!(output.len(), 600_003);
    assert_eq!(&output[299_999..300_004], b"a\"z\"b");
}

#[test]
fn lifecycle_models_processor_output_and_terminate_order() {
    #[derive(Debug, PartialEq, Eq)]
    enum Action {
        Current(Vec<u8>),
        Extra(Vec<u8>),
        Terminate,
        Eos,
    }
    let mut actions = Vec::new();
    let mut window = RawWindow::new();
    window
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"held")]))
        .unwrap();
    for bytes in match window.resolve(WindowDecision::Release).unwrap() {
        WindowOutput::Release(chunks) => chunks,
        _ => unreachable!(),
    } {
        actions.push(Action::Current(bytes.to_vec()));
    }
    window.start_next().unwrap();
    window
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"drop")]))
        .unwrap();
    assert_eq!(window.resolve(WindowDecision::Discard).unwrap(), WindowOutput::Discard);
    window.start_next().unwrap();
    window
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"old")]))
        .unwrap();
    if let WindowOutput::Replace(bytes) = window
        .finish(WindowDecision::Replace(Bytes::from_static(b"terminal")))
        .unwrap()
    {
        actions.push(Action::Extra(bytes.to_vec()));
    }
    actions.push(Action::Terminate);
    actions.push(Action::Eos);
    assert_eq!(
        actions,
        vec![
            Action::Current(b"held".to_vec()),
            Action::Extra(b"terminal".to_vec()),
            Action::Terminate,
            Action::Eos
        ]
    );
}

fn json_record(bucket: LogicalBucketKey, text: &str) -> WindowRecord {
    let raw = Bytes::from(serde_json::to_vec(text).unwrap());
    let end = raw.len();
    WindowRecord::raw(vec![raw]).with_fragments(vec![SemanticFragment {
        bucket,
        source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(0, end)]),
    }])
}

// Independently enumerate currently owned capacities for boundary tests;
// do not ask the production admission helper for the expected threshold.
fn owned_capacity(window: &RawWindow) -> usize {
    window.raw.resident_capacity()
        + window.raw.slice_metadata_capacity()
        + window.buckets.capacity() * size_of::<Bucket>()
        + window
            .buckets
            .iter()
            .map(|bucket| {
                bucket.overlap.capacity()
                    + bucket.fragments.capacity() * size_of::<StoredFragment>()
                    + bucket
                        .fragments
                        .iter()
                        .map(|fragment| fragment.source.range_capacity() * size_of::<ByteRange>())
                        .sum::<usize>()
            })
            .sum::<usize>()
}

#[test]
fn next_context_contains_only_emitted_text_across_three_generations() {
    let bucket = key(0, LogicalField::Content);
    for (action, expected) in [
        ("replace", "abéX"),
        ("empty", "abé"),
        ("discard", "abé"),
        ("opaque", ""),
    ] {
        let mut window = RawWindow::new();
        window.retain(json_record(bucket, "abé")).unwrap();
        window.resolve(WindowDecision::Release).unwrap();
        assert_eq!(window.resident().unwrap(), owned_capacity(&window));
        window.start_next().unwrap();
        window.retain(json_record(bucket, "hidden")).unwrap();
        match action {
            "replace" | "empty" => {
                window
                    .resolve_semantic_replace(vec![SemanticReplacement {
                        key: bucket,
                        current: if action == "replace" { "X".into() } else { String::new() },
                    }])
                    .unwrap();
            }
            "discard" => {
                window.resolve(WindowDecision::Discard).unwrap();
            }
            _ => {
                window
                    .resolve(WindowDecision::Replace(Bytes::from_static(b"opaque")))
                    .unwrap();
            }
        }
        assert_eq!(window.resident().unwrap(), owned_capacity(&window));
        window.start_next().unwrap();
        window.retain(json_record(bucket, "next")).unwrap();
        let snapshot = window.snapshot_all().unwrap();
        assert_eq!(snapshot[0].context, expected, "{action}");
        assert_eq!(snapshot[0].current, "next");
    }
}

#[test]
fn semantic_replacement_fairness_spans_prior_and_new_utf8_text() {
    let a = key(0, LogicalField::Content);
    let b = key(1, LogicalField::Content);
    let mut window = RawWindow::with_policy(RawWindowPolicy {
        per_bucket_overlap_bytes: 8,
        global_overlap_bytes: 8,
        max_replacement_bytes: MAX_REPLACEMENT_BYTES,
    })
    .unwrap();
    window.retain(json_record(a, "abcé")).unwrap();
    window.retain(json_record(b, "12345")).unwrap();
    window.resolve(WindowDecision::Release).unwrap();
    window.start_next().unwrap();
    window.retain(json_record(a, "hidden")).unwrap();
    window.retain(json_record(b, "hidden")).unwrap();
    window
        .resolve_semantic_replace(vec![
            SemanticReplacement {
                key: b,
                current: "Y".into(),
            },
            SemanticReplacement {
                key: a,
                current: "X".into(),
            },
        ])
        .unwrap();
    window.start_next().unwrap();
    let snapshot = window.snapshot_all().unwrap();
    assert_eq!(snapshot[0].context, "céX");
    assert_eq!(snapshot[1].context, "345Y");
}

#[test]
fn context_only_bucket_cannot_install_non_emitted_replacement() {
    let a = key(0, LogicalField::Content);
    let b = key(1, LogicalField::Content);
    let mut window = RawWindow::new();
    window.retain(json_record(a, "prior")).unwrap();
    window.resolve(WindowDecision::Release).unwrap();
    window.start_next().unwrap();
    window.retain(json_record(b, "current")).unwrap();
    assert_eq!(
        window.resolve_semantic_replace(vec![
            SemanticReplacement {
                key: a,
                current: "ghost".into()
            },
            SemanticReplacement {
                key: b,
                current: "new".into()
            },
        ]),
        Err(WindowError::InvalidReplacement)
    );
    assert_eq!(window.snapshot_all().unwrap()[1].current, "current");
    let output = window
        .resolve_semantic_replace(vec![
            SemanticReplacement {
                key: a,
                current: String::new(),
            },
            SemanticReplacement {
                key: b,
                current: "new".into(),
            },
        ])
        .unwrap();
    assert_eq!(output, WindowOutput::Replace(Bytes::from_static(b"\"new\"")));
    window.start_next().unwrap();
    assert_eq!(window.snapshot_all().unwrap()[0].context, "prior");
}

#[test]
fn raw_only_release_resets_context_across_an_unobservable_span() {
    let bucket = key(0, LogicalField::Content);
    let mut window = RawWindow::new();
    window.retain(json_record(bucket, "prior")).unwrap();
    window.resolve(WindowDecision::Release).unwrap();
    window.start_next().unwrap();
    window
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"unknown")]).semantic_unavailable())
        .unwrap();
    assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"unknown");
    window.start_next().unwrap();
    window.retain(json_record(bucket, "next")).unwrap();
    assert_eq!(window.snapshot_all().unwrap()[0].context, "");
}

#[test]
fn terminal_replacement_charges_inputs_but_never_builds_overlap() {
    let bucket = key(0, LogicalField::Content);
    for (finish, below, succeeds) in [(true, false, true), (true, true, false), (false, false, false)] {
        let mut window = RawWindow::new();
        window.retain(json_record(bucket, "old")).unwrap();
        let mut text = String::with_capacity(128);
        text.push('z');
        let mut replacements = Vec::with_capacity(2);
        replacements.push(SemanticReplacement {
            key: bucket,
            current: text,
        });
        let input = replacements.capacity() * size_of::<SemanticReplacement>() + replacements[0].current.capacity();
        let peak = owned_capacity(&window) + input + size_of::<ReplacementEdit>() + 3;
        window.limits.combined = 0;
        assert_eq!(window.preview_semantic_replace([(bucket, "z")]).unwrap(), 3);
        window.limits.combined = peak - usize::from(below);
        let output = if finish {
            window.finish_semantic_replace(replacements)
        } else {
            window.resolve_semantic_replace(replacements)
        };
        if succeeds {
            assert_eq!(output.unwrap(), WindowOutput::Replace(Bytes::from_static(b"\"z\"")));
            assert_eq!(window.start_next(), Err(WindowError::Finished));
        } else {
            assert_eq!(output, Err(WindowError::Limit(WindowLimit::CombinedResidentBytes)));
            window.limits.combined = MAX_COMBINED_RESIDENT_BYTES;
            assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), b"\"old\"");
        }
    }
}

#[test]
fn json_token_visitor_matches_serde_with_exact_scratch_and_output_caps() {
    let tokens: &[&[u8]] = &[
        br#""""#,
        br#""a\b\f\n\r\t\"\\\/""#,
        br#""\u20ac\u00e9""#,
        br#""\uD834\uDD1E""#,
        b" \t\"value\"\r\n",
        br#""\uD834""#,
        br#""\uDD1E""#,
        br#""\uD834\u0061""#,
        br#""\uXY00""#,
        br#""\q""#,
        br#""ok" false"#,
        b"\"\xff\"",
        b"\"a\n\"",
        b"\"tail\\",
    ];
    let bucket = key(0, LogicalField::Content);
    for &token in tokens {
        let oracle = serde_json::from_slice::<String>(token);
        for below in [false, true] {
            let mut window = RawWindow::new();
            window
                .retain(
                    WindowRecord::raw(vec![Bytes::copy_from_slice(token)]).with_fragments(vec![SemanticFragment {
                        bucket,
                        source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(0, token.len())]),
                    }]),
                )
                .unwrap();
            window.limits.combined =
                owned_capacity(&window) + token.len() + oracle.as_ref().map_or(0, |s| s.len()) - usize::from(below);
            let actual = window.snapshot(bucket).unwrap().materialize();
            if below {
                assert_eq!(
                    actual,
                    Err(WindowError::Limit(WindowLimit::CombinedResidentBytes)),
                    "{token:?}"
                );
            } else if let Ok(expected) = &oracle {
                assert_eq!(actual.as_ref().unwrap(), expected, "{token:?}");
            } else {
                assert_eq!(actual, Err(WindowError::InvalidJsonString), "{token:?}");
            }
        }
    }
}

proptest::proptest! {
    #[test]
    fn json_token_count_and_write_match_serialized_allowed_scalars(
        chars in proptest::collection::vec(proptest::sample::select(vec!['a', 'é', '€', 'π', '𝄞', '"', '\\', '\n', '\t', '\r', '\0']), 0..128)
    ) {
        let expected: String = chars.into_iter().collect();
        let token = serde_json::to_vec(&expected).unwrap();
        let mut counted = 0;
        visit_json_string(&token, &mut |part| { counted += part.len(); Ok(()) }).unwrap();
        let mut actual = String::with_capacity(counted);
        visit_json_string(&token, &mut |part| { actual.push_str(part); Ok(()) }).unwrap();
        proptest::prop_assert_eq!(counted, expected.len());
        proptest::prop_assert_eq!(actual, expected);
    }
}

#[test]
fn snapshot_all_charges_all_strings_context_and_result_metadata() {
    let a = key(0, LogicalField::Content);
    let b = key(1, LogicalField::Content);
    for below in [false, true] {
        let mut window = RawWindow::new();
        window.retain(json_record(a, "x")).unwrap();
        window.retain(json_record(b, "y")).unwrap();
        window.resolve(WindowDecision::Release).unwrap();
        window.start_next().unwrap();
        window.retain(json_record(a, "a")).unwrap();
        window.retain(json_record(b, "b")).unwrap();
        window.limits.combined = owned_capacity(&window) + 3 + 2 * size_of::<SemanticWindow>() + 4 - usize::from(below);
        let result = window.snapshot_all();
        if below {
            assert_eq!(result, Err(WindowError::Limit(WindowLimit::CombinedResidentBytes)));
        } else {
            let snapshot = result.unwrap();
            assert_eq!((&*snapshot[0].context, &*snapshot[0].current), ("x", "a"));
            assert_eq!((&*snapshot[1].context, &*snapshot[1].current), ("y", "b"));
        }
    }
}

#[test]
fn reused_raw_table_is_charged_once_and_rejection_preserves_input() {
    let mut window = RawWindow::new();
    window
        .retain(WindowRecord::raw(vec![Bytes::from_static(b"a")]))
        .unwrap();
    let record = WindowRecord::raw(vec![Bytes::from_static(b"b")]);
    let pointer = record.slices.as_ptr();
    let entry_size = window.raw.slice_metadata_capacity() / MAX_SLICE_HANDLES;
    let peak = owned_capacity(&window) + record.metadata_bytes() + entry_size + 1;
    window.limits.combined = peak - 1;
    let (error, record) = window.try_retain(record).unwrap_err();
    assert_eq!(error, WindowError::Limit(WindowLimit::CombinedResidentBytes));
    assert_eq!(record.slices.as_ptr(), pointer);
    assert_eq!(window.raw.len(), 1);
    window.limits.combined = peak;
    window.try_retain(record).unwrap();
    assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), b"ab");
}

#[test]
fn coalescing_and_arena_append_admit_before_consuming_the_record() {
    let mut window = RawWindow::new();
    for _ in 0..MAX_SLICE_HANDLES {
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"a")]))
            .unwrap();
    }
    for (text, additional) in [(b"y", MAX_RAW_BYTES), (b"z", 0)] {
        let record = WindowRecord::raw(vec![Bytes::from_static(text)]);
        let pointer = record.slices.as_ptr();
        let peak = owned_capacity(&window) + record.metadata_bytes() + additional;
        window.limits.combined = peak - 1;
        let (error, record) = window.try_retain(record).unwrap_err();
        assert_eq!(error, WindowError::Limit(WindowLimit::CombinedResidentBytes));
        assert_eq!(record.slices.as_ptr(), pointer);
        window.limits.combined = peak;
        window.try_retain(record).unwrap();
    }
    assert_eq!(window.raw.coalesce_count(), 1);
    let mut expected = vec![b'a'; MAX_SLICE_HANDLES];
    expected.extend_from_slice(b"yz");
    assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), expected);
}

#[test]
fn repeated_bucket_append_moves_ranges_without_cloning_allocations() {
    let bucket = key(0, LogicalField::Content);
    let mut window = RawWindow::new();
    let mut pointers = Vec::new();
    let mut capacity_changes = 0;
    let mut previous_capacity = 0;
    for index in 0..130 {
        let mut record = json_record(bucket, "v");
        if index == 2 {
            let original_ranges = record.fragments[0].source.ranges().as_ptr();
            let before = window.accounting();
            window.limits.combined = 0;
            let (error, returned) = window.try_retain(record).unwrap_err();
            assert_eq!(error, WindowError::Limit(WindowLimit::CombinedResidentBytes));
            assert_eq!(returned.fragments[0].source.ranges().as_ptr(), original_ranges);
            assert_eq!(returned.fragments[0].source.ranges(), &[ByteRange::new(0, 3)]);
            assert_eq!(window.accounting(), before);
            window.limits.combined = MAX_COMBINED_RESIDENT_BYTES;
            record = returned;
        }
        pointers.push(record.fragments[0].source.ranges().as_ptr());
        window.retain(record).unwrap();
        assert_eq!(window.resident().unwrap(), owned_capacity(&window));
        let fragments = &window.buckets[0].fragments;
        for (fragment, &pointer) in fragments.iter().zip(&pointers) {
            assert_eq!(fragment.source.ranges().as_ptr(), pointer);
        }
        capacity_changes += usize::from(fragments.capacity() != previous_capacity);
        previous_capacity = fragments.capacity();
    }
    assert_eq!(capacity_changes, 9);
}

#[test]
fn long_stream_releases_without_generation_growth() {
    let mut window = RawWindow::new();
    for i in 0..10_000 {
        let raw = Bytes::from(format!("data:{i}\n\n"));
        window
            .retain(WindowRecord::raw(
                raw.as_ref()
                    .iter()
                    .map(|byte| Bytes::copy_from_slice(&[*byte]))
                    .collect(),
            ))
            .unwrap();
        assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), raw);
        window.start_next().unwrap();
        assert_eq!(window.accounting().0, 0);
    }
}
