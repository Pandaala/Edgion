use super::*;

#[tokio::test]
async fn length_and_overwrite_require_complete_capture_and_follow_current_generation() {
    let (mut storage, view) = BodyStorage::with_limits(32, 64);
    assert_eq!(view.len(), Err(BodyReadError::Incomplete));
    assert_eq!(view.is_empty(), Err(BodyReadError::Incomplete));
    assert_eq!(
        view.overwrite_body(Bytes::from_static(b"early")),
        Err(BodyReadError::Incomplete)
    );
    assert_eq!(view.generation(), 0);
    assert!(!view.is_modified());

    capture(&mut storage, &[b"abc"]).await;
    assert_eq!(view.len(), Ok(3));
    assert_eq!(view.is_empty(), Ok(false));
    view.overwrite_body(Bytes::from_static(b"longer")).unwrap();
    assert_eq!(view.len(), Ok(6));
    assert_eq!(view.generation(), 1);
    view.overwrite_body(Bytes::new()).unwrap();
    assert_eq!(view.len(), Ok(0));
    assert_eq!(view.is_empty(), Ok(true));
    assert_eq!(view.generation(), 2);

    view.freeze_mutation();
    assert_eq!(
        view.overwrite_body(Bytes::from_static(b"frozen")),
        Err(BodyReadError::NotBuffered)
    );
    assert_eq!(view.len(), Ok(0));
    assert_eq!(view.generation(), 2);
}

#[tokio::test]
async fn mirror_snapshot_read_output_holds_lane_until_last_clone() {
    let limiter = Arc::new(Semaphore::new(1));
    let worker_limiter = Arc::clone(&limiter);
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        let output = run_storage_io_retained_with_limiter(
            worker_limiter,
            Duration::from_secs(1),
            StorageIoOperation::SnapshotRead,
            || Ok(Bytes::from(vec![b'x'; SCAN_CHUNK_SIZE])),
        )
        .await
        .unwrap();
        finished_tx.send(()).unwrap();
        output
    });
    finished_rx.await.unwrap();
    // The completed output may still be waiting inside the JoinHandle.
    assert_eq!(limiter.available_permits(), 0);
    let bytes = Bytes::from_owner(worker.await.unwrap());
    let transport = bytes.slice(..1);
    drop(bytes);
    assert_eq!(limiter.available_permits(), 0);
    drop(transport);
    assert_eq!(limiter.available_permits(), 1);
}
use std::future::{poll_fn, Future};
use std::task::Poll;

struct SnapshotMemoryOwner {
    bytes: Box<[u8]>,
    dropped: Arc<AtomicBool>,
}

impl AsRef<[u8]> for SnapshotMemoryOwner {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for SnapshotMemoryOwner {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

#[tokio::test]
async fn snapshot_isolation_releases_large_source_and_inline_backings() {
    let source_dropped = Arc::new(AtomicBool::new(false));
    let inline_dropped = Arc::new(AtomicBool::new(false));
    let source = Bytes::from_owner(SnapshotMemoryOwner {
        bytes: vec![b'a'; 1024 * 1024].into_boxed_slice(),
        dropped: Arc::clone(&source_dropped),
    });
    let inline = Bytes::from_owner(SnapshotMemoryOwner {
        bytes: vec![b'b'; 1024 * 1024].into_boxed_slice(),
        dropped: Arc::clone(&inline_dropped),
    });
    let snapshot = BodySnapshot::from_segments(vec![
        Segment::SourceRange {
            source: Arc::new(BodySource::Memory(source)),
            offset: 500,
            len: 3,
        },
        Segment::Inline(inline.slice(700..704)),
    ])
    .unwrap();
    drop(inline);
    let mut reader = BodySnapshotReader::new(Arc::new(snapshot), DEFAULT_STORAGE_OPERATION_TIMEOUT);
    assert_eq!(reader.memory_bytes_to_isolate(), Some(7));
    assert!(!source_dropped.load(Ordering::Acquire));
    assert!(!inline_dropped.load(Ordering::Acquire));
    reader.isolate_memory().unwrap();
    assert!(source_dropped.load(Ordering::Acquire));
    assert!(inline_dropped.load(Ordering::Acquire));
    assert_eq!(reader.len(), 7);
    assert_eq!(reader.next_chunk().await.unwrap().unwrap(), "aaa");
    assert_eq!(reader.next_chunk().await.unwrap().unwrap(), "bbbb");
    assert!(reader.next_chunk().await.unwrap().is_none());
}

#[tokio::test]
async fn snapshot_isolation_preserves_disk_ranges_without_memory_charge() {
    let file = Arc::new(tempfile().unwrap());
    write_all_at(&file, b"disk payload", 0).unwrap();
    let snapshot = BodySnapshot::from_segments(vec![Segment::SourceRange {
        source: Arc::new(BodySource::Disk {
            file: Arc::clone(&file),
            len: 12,
        }),
        offset: 5,
        len: 7,
    }])
    .unwrap();
    let mut reader = BodySnapshotReader::new(Arc::new(snapshot), DEFAULT_STORAGE_OPERATION_TIMEOUT);
    assert_eq!(reader.memory_bytes_to_isolate(), Some(0));
    reader.isolate_memory().unwrap();
    assert!(reader.snapshot.contains_disk());
    assert_eq!(reader.next_chunk().await.unwrap().unwrap(), "payload");
    assert!(reader.next_chunk().await.unwrap().is_none());
}

#[tokio::test]
async fn snapshot_isolation_preserves_committed_version_and_rejects_late_isolation() {
    let (mut storage, view) = BodyStorage::new(1024, 4096);
    storage.write(&Bytes::from_static(b"original")).await.unwrap();
    storage.finish().await.unwrap();
    let mut reader = view.snapshot_reader().unwrap();
    assert_eq!(reader.memory_bytes_to_isolate(), Some(8));
    reader.isolate_memory().unwrap();
    view.overwrite_body(Bytes::from_static(b"updated")).unwrap();
    assert_eq!(reader.next_chunk().await.unwrap().unwrap(), "original");
    assert_eq!(reader.memory_bytes_to_isolate(), None);
    assert_eq!(reader.isolate_memory(), Err(BodyReadError::Incomplete));
    assert!(reader.next_chunk().await.unwrap().is_none());
}

#[test]
fn snapshot_isolation_rejects_invalid_memory_range_without_panicking() {
    let snapshot = BodySnapshot::from_segments(vec![Segment::SourceRange {
        source: Arc::new(BodySource::Memory(Bytes::from_static(b"x"))),
        offset: u64::MAX,
        len: 1,
    }])
    .unwrap();
    let mut reader = BodySnapshotReader::new(Arc::new(snapshot), DEFAULT_STORAGE_OPERATION_TIMEOUT);
    assert_eq!(reader.memory_bytes_to_isolate(), None);
    assert_eq!(reader.isolate_memory(), Err(BodyReadError::Incomplete));
}

#[tokio::test]
async fn disk_read_and_join_failures_are_storage_failures() {
    let empty = tempfile().expect("anonymous temp file");
    assert_eq!(
        read_disk_body(Arc::new(empty), 1, DEFAULT_STORAGE_OPERATION_TIMEOUT,).await,
        Err(BodyReadError::StorageFailure)
    );

    let join = run_storage_io(
        DEFAULT_STORAGE_OPERATION_TIMEOUT,
        StorageIoOperation::MaterializeRead,
        || -> io::Result<Bytes> { panic!("intentional join failure") },
    )
    .await;
    assert!(matches!(join, Err(StorageIoError::Join(_))));
}

async fn capture(storage: &mut BodyStorage, chunks: &[&[u8]]) {
    for chunk in chunks {
        storage.write(&Bytes::copy_from_slice(chunk)).await.unwrap();
    }
    storage.finish().await.unwrap();
}

/// `replace_spans` is the primitive the AI prompt guard masks through, so
/// every one of its guarantees is exercised directly here rather than
/// through a mocked session.
mod replace_spans {
    use super::*;

    const BODY: &[u8] = br#"{"a":"one","b":"two","c":"three"}"#;

    fn edit(range: std::ops::Range<u64>, text: &str) -> (std::ops::Range<u64>, Bytes) {
        (range, Bytes::from(text.to_string()))
    }

    /// Byte range of the Nth quoted value token in BODY.
    fn token(value: &str) -> std::ops::Range<u64> {
        let quoted = format!("\"{value}\"");
        let start = std::str::from_utf8(BODY).unwrap().find(&quoted).expect("token present") as u64;
        start..start + quoted.len() as u64
    }

    async fn view() -> (BodyStorage, Arc<BodyStorageView>) {
        let (mut storage, view) = BodyStorage::new(64 * 1024, 1024 * 1024);
        capture(&mut storage, &[BODY]).await;
        (storage, view)
    }

    #[tokio::test]
    async fn several_disjoint_edits_apply_as_one_generation() {
        let (mut storage, view) = view().await;
        let before = view.generation();
        let after = view
            .replace_spans(
                before,
                &[edit(token("one"), "\"1\""), edit(token("three"), "\"333333\"")],
            )
            .expect("batch applies");

        // One publish for the whole batch, not one per edit.
        assert_eq!(after, before + 1);
        assert_eq!(
            replay_all(&mut storage, 4096).await,
            Bytes::from_static(br#"{"a":"1","b":"two","c":"333333"}"#)
        );
    }

    #[tokio::test]
    async fn publication_returns_its_own_generation_after_a_later_commit() {
        let (_storage, view) = view().await;
        let (snapshot, before) = view.snapshot().unwrap();

        let first = view.publish_arc(before, Arc::clone(&snapshot)).unwrap();
        let second = view.publish_arc(first, snapshot).unwrap();

        assert_eq!(first, before + 1);
        assert_eq!(second, first + 1);
        assert_eq!(view.generation(), second);
    }

    #[tokio::test]
    async fn a_stale_generation_is_a_conflict_and_publishes_nothing() {
        let (mut storage, view) = view().await;
        let stale = view.generation();
        view.replace_spans(stale, &[edit(token("one"), "\"1\"")])
            .expect("first batch applies");

        // Offsets computed against the old snapshot must never be spliced
        // into the new one.
        let error = view
            .replace_spans(stale, &[edit(token("two"), "\"2\"")])
            .expect_err("stale generation must be refused");
        assert!(matches!(
            error,
            BodyOperationError::Body(BodyReadError::GenerationConflict)
        ));
        assert_eq!(
            replay_all(&mut storage, 4096).await,
            Bytes::from_static(br#"{"a":"1","b":"two","c":"three"}"#)
        );
    }

    #[tokio::test]
    async fn overlapping_unsorted_and_out_of_range_edits_are_refused_atomically() {
        let (mut storage, view) = view().await;
        let generation = view.generation();
        let one = token("one");
        let three = token("three");

        for bad in [
            // Descending order: offsets are consumed once, ascending.
            vec![edit(three.clone(), "\"x\""), edit(one.clone(), "\"y\"")],
            // Overlapping ranges.
            vec![
                edit(one.start..one.end, "\"x\""),
                edit(one.start + 1..one.end + 1, "\"y\""),
            ],
            // Inverted range.
            vec![edit(one.end..one.start, "\"x\"")],
            // Past the end of the body.
            vec![edit(0..u64::from(u32::MAX), "\"x\"")],
            // A no-op still participates in ordering validation.
            vec![edit(three.end..three.end, ""), edit(one.clone(), "\"x\"")],
        ] {
            assert!(
                view.replace_spans(generation, &bad).is_err(),
                "malformed edit set must be refused"
            );
        }

        // Nothing was published by any of the refused batches.
        assert_eq!(view.generation(), generation);
        assert_eq!(replay_all(&mut storage, 4096).await, Bytes::from_static(BODY));
    }

    #[tokio::test]
    async fn an_empty_edit_set_publishes_nothing() {
        let (_storage, view) = view().await;
        let generation = view.generation();
        assert_eq!(view.replace_spans(generation, &[]).expect("no-op"), generation);
        assert_eq!(view.generation(), generation);
        assert!(!view.is_modified());
    }

    #[tokio::test]
    async fn growth_past_the_physical_cap_fails_and_leaves_the_body_intact() {
        let (mut storage, view) = BodyStorage::new(64 * 1024, 64);
        capture(&mut storage, &[BODY]).await;
        let generation = view.generation();

        let oversized = format!("\"{}\"", "x".repeat(64));
        let error = view
            .replace_spans(generation, &[edit(token("one"), &oversized)])
            .expect_err("growth past the cap must fail");
        assert!(matches!(
            error,
            BodyOperationError::Body(BodyReadError::BufferLimitExceeded { .. })
        ));
        assert_eq!(view.generation(), generation);
        assert_eq!(replay_all(&mut storage, 4096).await, Bytes::from_static(BODY));
    }

    #[tokio::test]
    async fn a_shrinking_mask_is_forwarded_and_reframed() {
        let (mut storage, view) = view().await;
        let generation = view.generation();
        view.replace_spans(generation, &[edit(token("three"), "\"\"")])
            .expect("shrink applies");

        let replayed = replay_all(&mut storage, 4096).await;
        assert_eq!(replayed, Bytes::from_static(br#"{"a":"one","b":"two","c":""}"#));
        // Framing consumers read the mutated length, not the original.
        assert!(view.is_modified());
        assert_eq!(view.len().unwrap(), replayed.len() as u64);
    }

    #[tokio::test]
    async fn a_mask_over_a_disk_spilled_body_is_applied_and_replayed() {
        // A tiny memory budget forces the spill path.
        let (mut storage, view) = BodyStorage::new(8, 1024 * 1024);
        capture(&mut storage, &[BODY]).await;
        let generation = view.generation();

        view.replace_spans(generation, &[edit(token("two"), "\"22\"")])
            .expect("spilled mask applies");
        assert_eq!(
            replay_all(&mut storage, 4096).await,
            Bytes::from_static(br#"{"a":"one","b":"22","c":"three"}"#)
        );
    }

    #[tokio::test]
    async fn exhausting_the_segment_budget_fails_atomically() {
        let filler = format!("{{\"k\":\"{}\"}}", "ab".repeat(4096));
        let (mut storage, view) = BodyStorage::new(1024 * 1024, 1024 * 1024);
        capture(&mut storage, &[filler.as_bytes()]).await;
        let generation = view.generation();

        // Each edit splits the body further; enough of them exhaust
        // MAX_BODY_SEGMENTS.
        let edits: Vec<_> = (0..MAX_BODY_SEGMENTS)
            .map(|i| {
                let start = 8 + (i as u64) * 2;
                (start..start + 1, Bytes::from_static(b"z"))
            })
            .collect();
        assert!(view.replace_spans(generation, &edits).is_err());
        assert_eq!(view.generation(), generation);
        assert_eq!(replay_all(&mut storage, 4096).await.len(), filler.len());
    }

    #[tokio::test]
    async fn many_edits_traverse_a_maximally_segmented_snapshot_monotonically() {
        let (mut storage, view) = BodyStorage::with_limits(64, u64::MAX);
        capture(&mut storage, &[b"x"]).await;
        let segmented =
            BodySnapshot::from_segments(vec![Segment::Inline(Bytes::from_static(b"x")); MAX_BODY_SEGMENTS]).unwrap();
        let generation = view.publish(view.generation(), segmented).unwrap();
        let edits: Vec<_> = (0..MAX_BODY_SEGMENTS)
            .step_by(2)
            .map(|offset| (offset as u64..offset as u64 + 1, Bytes::from_static(b"z")))
            .collect();

        view.replace_spans(generation, &edits).unwrap();
        let replayed = replay_all(&mut storage, 1).await;
        assert_eq!(replayed.len(), MAX_BODY_SEGMENTS);
        assert!(replayed
            .iter()
            .enumerate()
            .all(|(index, byte)| { *byte == if index % 2 == 0 { b'z' } else { b'x' } }));
    }
}

async fn replay_all(storage: &mut BodyStorage, chunk_size: usize) -> Bytes {
    storage.rewind().await.unwrap();
    let mut body = BytesMut::new();
    while let Some(chunk) = storage.next_chunk(chunk_size).await.unwrap() {
        let len = chunk.len();
        body.extend_from_slice(&chunk);
        storage.consume(len);
    }
    body.freeze()
}

async fn read_snapshot(mut reader: Box<dyn RequestBodySnapshotReader>) -> (Vec<usize>, Bytes) {
    let mut chunk_lengths = Vec::new();
    let mut body = BytesMut::new();
    while let Some(chunk) = reader.next_chunk().await.unwrap() {
        chunk_lengths.push(chunk.len());
        body.extend_from_slice(&chunk);
    }
    (chunk_lengths, body.freeze())
}

fn disk_file(storage: &BodyStorage) -> &Arc<File> {
    match storage.view.initial.get().unwrap().source.as_ref() {
        BodySource::Disk { file, .. } => file,
        _ => panic!("test requires a finished disk-backed body"),
    }
}

#[tokio::test]
async fn completed_capture_retains_published_body_without_accepting_more_writes() {
    for memory_limit in [1, 64] {
        for body in [b"".as_slice(), b"complete"] {
            let (mut storage, view) = BodyStorage::with_limits(memory_limit, 64);
            capture(&mut storage, &[body]).await;
            storage.finish().await.unwrap();
            assert!(storage.write(&Bytes::new()).await.is_err());
            assert!(storage.write(&Bytes::from_static(b"late")).await.is_err());
            assert_eq!(replay_all(&mut storage, 2).await.as_ref(), body);
            drop(storage);
            assert_eq!(view.read_all().await.unwrap().as_ref(), body);
        }
    }
}

#[tokio::test]
async fn memory_and_disk_text_operations_cross_scan_boundaries() {
    let mut raw = vec![b'x'; SCAN_CHUNK_SIZE - 1];
    raw.extend_from_slice("€needle".as_bytes());
    raw.extend_from_slice(&[0xf0, 0x90]);
    for memory_limit in [raw.len() + 1, 1] {
        let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
        capture(&mut storage, &[&raw]).await;
        let contains = view
            .text_operation(BodyTextOperation::Contains("€needle�".to_string()), u64::MAX)
            .await
            .unwrap();
        assert_eq!(contains.value, BodyTextResult::Bool(true));
        assert_eq!(contains.bytes_scanned, raw.len() as u64);
        let len = view.text_operation(BodyTextOperation::CharLen, u64::MAX).await.unwrap();
        assert_eq!(
            len.value,
            BodyTextResult::CharLen(String::from_utf8_lossy(&raw).chars().count() as u64)
        );
    }
}

#[tokio::test]
async fn bounded_text_ranges_match_isolated_lossy_bytes_in_memory_spill_and_segments() {
    let raw = [b'a', 0xe2, 0x82, 0xac, b'z', 0xff, 0xe2, 0x82];
    for memory_limit in [64, 1] {
        for segmented in [false, true] {
            let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
            if segmented {
                capture(&mut storage, &[&raw[..2]]).await;
                view.append(Bytes::copy_from_slice(&raw[2..])).unwrap();
            } else {
                capture(&mut storage, &[&raw]).await;
            }
            for start in 0..=raw.len() {
                for end in start..=raw.len() {
                    let expected = String::from_utf8_lossy(&raw[start..end]);
                    for limit in [0, expected.len().saturating_sub(1), expected.len(), expected.len() + 1] {
                        let result = view
                            .read_text_range(start as u64, (end - start) as u64, limit, raw.len() as u64)
                            .await
                            .unwrap();
                        if expected.len() <= limit {
                            assert_eq!(result.value, BoundedTextResult::Complete(expected.to_string()));
                            assert_eq!(result.bytes_scanned, (end - start) as u64);
                        } else {
                            assert_eq!(result.value, BoundedTextResult::OutputLimitExceeded);
                            assert!(result.bytes_scanned > 0);
                            assert!(result.bytes_scanned <= (end - start) as u64);
                        }
                        assert!(!view.is_materialized());
                    }
                }
            }
            assert_eq!(
                view.read_text_range(1, u64::MAX, 32, 32).await.unwrap().value,
                BoundedTextResult::Complete(String::from_utf8_lossy(&raw[1..]).into_owned())
            );
            assert_eq!(
                view.read_text_range(raw.len() as u64 + 1, 0, 0, 0).await,
                Err(BodyOperationError::InvalidRange)
            );
            assert_eq!(
                view.read_text_range(0, raw.len() as u64, 0, raw.len() as u64 - 1).await,
                Err(BodyOperationError::ScanLimitExceeded {
                    limit: raw.len() as u64 - 1
                })
            );
        }
    }
}

#[tokio::test]
async fn bounded_text_overflow_charges_the_fetched_chunk_and_skips_the_tail() {
    let body = vec![b'x'; 3 * SCAN_CHUNK_SIZE + 17];
    for memory_limit in [body.len() + 1, 1] {
        let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
        capture(&mut storage, &[&body]).await;
        for (limit, charged) in [(3, SCAN_CHUNK_SIZE), (SCAN_CHUNK_SIZE, 2 * SCAN_CHUNK_SIZE)] {
            let result = view
                .read_text_range(0, u64::MAX, limit, body.len() as u64)
                .await
                .unwrap();
            assert_eq!(result.value, BoundedTextResult::OutputLimitExceeded);
            assert_eq!(result.bytes_scanned, charged as u64);
            assert!(!view.is_materialized());
        }

        // A mutation segment is a separate fetch boundary. Charge its
        // actual fetched length, not a nominal full 64 KiB chunk.
        view.prepend(Bytes::from_static(b"prefix")).unwrap();
        let result = view.read_text_range(0, u64::MAX, 3, u64::MAX).await.unwrap();
        assert_eq!(result.value, BoundedTextResult::OutputLimitExceeded);
        assert_eq!(result.bytes_scanned, 6);
    }
}

#[tokio::test]
async fn bounded_text_handles_utf8_split_at_storage_chunk_and_eof() {
    let mut body = vec![b'x'; SCAN_CHUNK_SIZE - 1];
    body.extend_from_slice("€".as_bytes());
    body.extend_from_slice(&[0xe2, 0x82]);
    let expected = String::from_utf8_lossy(&body);
    for memory_limit in [body.len() + 1, 1] {
        let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
        capture(&mut storage, &[&body]).await;
        let exact = view
            .read_text_range(0, u64::MAX, expected.len(), u64::MAX)
            .await
            .unwrap();
        assert_eq!(exact.value, BoundedTextResult::Complete(expected.to_string()));
        assert_eq!(exact.bytes_scanned, body.len() as u64);

        let overflow = view
            .read_text_range(0, u64::MAX, expected.len() - 1, u64::MAX)
            .await
            .unwrap();
        assert_eq!(overflow.value, BoundedTextResult::OutputLimitExceeded);
        assert_eq!(overflow.bytes_scanned, body.len() as u64);
        assert!(!view.is_materialized());
    }
}

#[tokio::test]
async fn disk_scan_does_not_materialize() {
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[b"prefix-body-suffix"]).await;
    assert_eq!(
        view.text_operation(BodyTextOperation::StartsWith("prefix".to_string()), u64::MAX)
            .await
            .unwrap()
            .value,
        BodyTextResult::Bool(true)
    );
    assert!(!view.is_materialized());
    let expected: [u8; 32] = Sha256::digest(b"prefix-body-suffix").into();
    assert_eq!(view.sha256_scan(u64::MAX).await.unwrap().value, expected);
    assert!(!view.is_materialized());
}

#[tokio::test]
async fn sha256_cache_scans_each_memory_or_disk_snapshot_once() {
    let body = b"cached request body";
    let expected: [u8; 32] = Sha256::digest(body).into();
    for memory_limit in [body.len() + 1, 1] {
        let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
        capture(&mut storage, &[body]).await;

        let first = view.sha256_scan(body.len() as u64).await.unwrap();
        let second = view.sha256_scan(0).await.unwrap();

        assert_eq!(first.value, expected);
        assert_eq!(first.bytes_scanned, body.len() as u64);
        assert_eq!(second.value, expected);
        assert_eq!(second.bytes_scanned, 0);
        assert!(!view.is_materialized());
    }
}

#[tokio::test]
async fn sha256_failed_scan_is_not_cached_and_concurrent_callers_share_success() {
    let body = vec![b'x'; SCAN_CHUNK_SIZE + 17];
    let expected: [u8; 32] = Sha256::digest(&body).into();
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[&body]).await;

    assert_eq!(
        view.sha256_scan(body.len() as u64 - 1).await.unwrap_err(),
        BodyOperationError::ScanLimitExceeded {
            limit: body.len() as u64 - 1
        }
    );

    let (left, right) = tokio::join!(view.sha256_scan(body.len() as u64), view.sha256_scan(body.len() as u64));
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.value, expected);
    assert_eq!(right.value, expected);
    assert_eq!(
        left.bytes_scanned + right.bytes_scanned,
        body.len() as u64,
        "exactly one concurrent initializer must scan the snapshot"
    );
    assert_eq!(view.sha256_scan(0).await.unwrap().bytes_scanned, 0);
    assert!(!view.is_materialized());
}

#[tokio::test]
async fn sha256_cache_is_generation_local_across_mutations_and_failures() {
    let (mut storage, view) = BodyStorage::with_limits(1, 16);
    capture(&mut storage, &[b"abc"]).await;

    async fn assert_new_generation(view: &BodyStorageView, body: &[u8]) {
        let expected: [u8; 32] = Sha256::digest(body).into();
        let first = view.sha256_scan(body.len() as u64).await.unwrap();
        assert_eq!(first.value, expected);
        assert_eq!(first.bytes_scanned, body.len() as u64);
        let cached = view.sha256_scan(0).await.unwrap();
        assert_eq!(cached.value, expected);
        assert_eq!(cached.bytes_scanned, 0);
    }

    assert_new_generation(&view, b"abc").await;
    assert_eq!(
        view.append(Bytes::from_static(b"body-too-large")),
        Err(BodyOperationError::Body(BodyReadError::BufferLimitExceeded {
            limit: 16
        }))
    );
    assert_eq!(view.sha256_scan(0).await.unwrap().bytes_scanned, 0);

    view.append(Bytes::from_static(b"d")).unwrap();
    assert_new_generation(&view, b"abcd").await;
    view.prepend(Bytes::from_static(b"0")).unwrap();
    assert_new_generation(&view, b"0abcd").await;
    view.remove(1..3).unwrap();
    assert_new_generation(&view, b"0cd").await;
    view.replace_literal(Bytes::from_static(b"c"), Bytes::from_static(b"XY"), 3)
        .await
        .unwrap();
    assert_new_generation(&view, b"0XYd").await;
    assert_eq!(
        view.replace_slice(1..3, Bytes::from_static(b"z")).await.unwrap(),
        Bytes::from_static(b"0zd")
    );
    assert_new_generation(&view, b"0zd").await;
    view.overwrite_body(Bytes::from_static(b"new")).unwrap();
    assert_new_generation(&view, b"new").await;
    view.clear().unwrap();
    assert_new_generation(&view, b"").await;
}

#[tokio::test]
async fn digest_completed_for_old_snapshot_cannot_populate_new_generation() {
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[b"old"]).await;
    let (old_snapshot, _) = view.snapshot().unwrap();

    view.overwrite_body(Bytes::from_static(b"new")).unwrap();
    let old_for_scan = Arc::clone(&old_snapshot);
    let old_digest = old_snapshot
        .sha256
        .get_or_try_init(|| async {
            view.scan_snapshot(old_for_scan, Sha256Scanner(Sha256::new()), u64::MAX)
                .await
                .map(|scan| scan.value)
        })
        .await
        .unwrap();
    let expected_old: [u8; 32] = Sha256::digest(b"old").into();
    assert_eq!(*old_digest, expected_old);

    let current = view.sha256_scan(u64::MAX).await.unwrap();
    let expected_new: [u8; 32] = Sha256::digest(b"new").into();
    assert_eq!(current.value, expected_new);
    assert_eq!(current.bytes_scanned, 3);
}

#[tokio::test]
async fn contiguous_snapshot_borrows_checked_range_and_keeps_owner_alive() {
    for inline in [false, true] {
        let dropped = Arc::new(AtomicBool::new(false));
        let source = Bytes::from_owner(SnapshotMemoryOwner {
            bytes: vec![b'x'; SCAN_CHUNK_SIZE + 31].into_boxed_slice(),
            dropped: Arc::clone(&dropped),
        });
        let expected_ptr = source[7..].as_ptr();
        let len = source.len() - 14;
        let segment = if inline {
            Segment::Inline(source.slice(7..7 + len))
        } else {
            Segment::SourceRange {
                source: Arc::new(BodySource::Memory(source.clone())),
                offset: 7,
                len: len as u64,
            }
        };
        drop(source);
        let snapshot = BodySnapshot::from_segments(vec![segment]).unwrap();
        let mut reader = BodySnapshotReader::new(Arc::new(snapshot), DEFAULT_STORAGE_OPERATION_TIMEOUT);
        assert_eq!(reader.try_take_contiguous_memory(len - 1), Ok(None));
        assert_eq!((reader.segment_index, reader.segment_offset), (0, 0));
        let borrowed = reader.try_take_contiguous_memory(len).unwrap().unwrap();
        assert_eq!(borrowed.as_ptr(), expected_ptr);
        assert_eq!(borrowed.len(), len);
        assert_eq!(reader.try_take_contiguous_memory(len), Ok(None));
        assert!(reader.next_chunk().await.unwrap().is_none());
        drop(reader);
        assert!(!dropped.load(Ordering::Acquire));
        assert!(borrowed.iter().all(|byte| *byte == b'x'));
        drop(borrowed);
        assert!(dropped.load(Ordering::Acquire));
    }
}

#[tokio::test]
async fn contiguous_snapshot_fallback_preserves_empty_disk_multisegment_and_started_reads() {
    for memory_limit in [1, SCAN_CHUNK_SIZE * 3] {
        let body = vec![b'x'; SCAN_CHUNK_SIZE + 17];
        let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
        capture(&mut storage, &[&body]).await;
        let mut reader = view.snapshot_reader().unwrap();
        if memory_limit == 1 {
            assert_eq!(reader.try_take_contiguous_memory(body.len()), Ok(None));
        }
        assert_eq!(reader.next_chunk().await.unwrap().unwrap().len(), SCAN_CHUNK_SIZE);
        assert_eq!(reader.try_take_contiguous_memory(body.len()), Ok(None));
        assert_eq!(read_snapshot(reader).await.1.as_ref(), &body[SCAN_CHUNK_SIZE..]);

        view.prepend(Bytes::from_static(b"prefix")).unwrap();
        let mut reader = view.snapshot_reader().unwrap();
        assert_eq!(reader.try_take_contiguous_memory(usize::MAX), Ok(None));
        let mut expected = b"prefix".to_vec();
        expected.extend_from_slice(&body);
        assert_eq!(read_snapshot(reader).await.1.as_ref(), expected);
        assert!(!view.is_materialized());
    }
    let mut reader = BodySnapshotReader::new(Arc::new(BodySnapshot::empty()), DEFAULT_STORAGE_OPERATION_TIMEOUT);
    assert_eq!(reader.try_take_contiguous_memory(128 * 1024), Ok(None));
    assert!(reader.next_chunk().await.unwrap().is_none());
}

#[tokio::test]
async fn contiguous_snapshot_preserves_mutation_version_and_isolated_owner() {
    let (mut storage, view) = BodyStorage::with_memory_limit(1024);
    capture(&mut storage, &[b"original"]).await;
    let mut old = view.snapshot_reader().unwrap();
    view.overwrite_body(Bytes::from_static(b"replacement")).unwrap();
    assert_eq!(old.try_take_contiguous_memory(32).unwrap().unwrap(), "original");
    assert_eq!(
        view.snapshot_reader()
            .unwrap()
            .try_take_contiguous_memory(32)
            .unwrap()
            .unwrap(),
        "replacement"
    );

    let dropped = Arc::new(AtomicBool::new(false));
    let source = Bytes::from_owner(SnapshotMemoryOwner {
        bytes: vec![b'a'; 1024 * 1024].into_boxed_slice(),
        dropped: Arc::clone(&dropped),
    });
    let snapshot = BodySnapshot::from_inline(source.slice(7..11)).unwrap();
    drop(source);
    let mut reader = BodySnapshotReader::new(Arc::new(snapshot), DEFAULT_STORAGE_OPERATION_TIMEOUT);
    reader.isolate_memory().unwrap();
    assert!(dropped.load(Ordering::Acquire));
    let isolated_ptr = BodySnapshotReader::memory_segment(&reader.snapshot.segments[0])
        .unwrap()
        .unwrap()
        .as_ptr();
    let borrowed = reader.try_take_contiguous_memory(4).unwrap().unwrap();
    assert_eq!(borrowed.as_ptr(), isolated_ptr);
    assert_eq!(reader.isolate_memory(), Err(BodyReadError::Incomplete));
    drop(reader);
    assert_eq!(borrowed, "aaaa");
}

#[test]
fn contiguous_snapshot_rejects_invalid_ranges_without_advancing() {
    for (offset, len, snapshot_len) in [(u64::MAX, 2, 2), (1, 4, 4), (0, 3, 2)] {
        let snapshot = BodySnapshot {
            segments: vec![Segment::SourceRange {
                source: Arc::new(BodySource::Memory(Bytes::from_static(b"abc"))),
                offset,
                len,
            }],
            len: snapshot_len,
            sha256: OnceCell::new(),
        };
        let mut reader = BodySnapshotReader::new(Arc::new(snapshot), DEFAULT_STORAGE_OPERATION_TIMEOUT);
        assert_eq!(
            reader.try_take_contiguous_memory(128 * 1024),
            Err(BodyReadError::Incomplete)
        );
        assert_eq!((reader.segment_index, reader.segment_offset), (0, 0));
    }
}

#[tokio::test]
async fn snapshot_reader_streams_memory_in_bounded_chunks() {
    let body = vec![b'm'; SCAN_CHUNK_SIZE * 2 + 17];
    let (mut storage, view) = BodyStorage::with_memory_limit(body.len() + 1);
    capture(&mut storage, &[&body]).await;

    let reader = view.snapshot_reader().unwrap();
    assert_eq!(reader.len(), body.len() as u64);
    let (chunk_lengths, streamed) = read_snapshot(reader).await;

    assert_eq!(chunk_lengths, vec![SCAN_CHUNK_SIZE, SCAN_CHUNK_SIZE, 17]);
    assert_eq!(streamed.as_ref(), body.as_slice());
    assert!(!view.is_materialized());
}

#[tokio::test]
async fn snapshot_reader_streams_spill_without_materializing() {
    let body = vec![b'd'; SCAN_CHUNK_SIZE + 11];
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[&body]).await;

    let reader = view.snapshot_reader().unwrap();
    assert_eq!(reader.len(), body.len() as u64);
    let (chunk_lengths, streamed) = read_snapshot(reader).await;

    assert_eq!(chunk_lengths, vec![SCAN_CHUNK_SIZE, 11]);
    assert_eq!(streamed.as_ref(), body.as_slice());
    assert!(!view.is_materialized());
}

#[tokio::test]
async fn snapshot_reader_remains_stable_across_mutation() {
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[b"original"]).await;
    let old_reader = view.snapshot_reader().unwrap();

    view.prepend(Bytes::from_static(b"new-")).unwrap();
    view.remove(4..12).unwrap();

    assert_eq!(read_snapshot(old_reader).await.1, Bytes::from_static(b"original"));
    assert_eq!(
        read_snapshot(view.snapshot_reader().unwrap()).await.1,
        Bytes::from_static(b"new-")
    );
    assert!(!view.is_materialized());
}

#[tokio::test]
async fn snapshot_reader_handles_empty_body_without_chunks() {
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[]).await;

    let reader = view.snapshot_reader().unwrap();
    assert_eq!(reader.len(), 0);
    assert_eq!(read_snapshot(reader).await, (Vec::new(), Bytes::new()));
    assert!(!view.is_materialized());
}

#[tokio::test]
async fn source_range_mutations_preserve_spill_and_replay() {
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[b"alpha-beta-alpha"]).await;
    view.prepend(Bytes::from_static(b"<")).unwrap();
    view.append(Bytes::from_static(b">")).unwrap();
    view.remove(7..12).unwrap();
    assert!(!view.is_materialized());
    assert_eq!(replay_all(&mut storage, 3).await, Bytes::from_static(b"<alpha-alpha>"));
    assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"<alpha-alpha>"));
}

#[tokio::test]
async fn literal_replace_is_streaming_transactional_and_stable_for_retry() {
    let (mut storage, view) = BodyStorage::with_limits(1, 64);
    capture(&mut storage, &[b"ababa--aba"]).await;
    let result = view
        .replace_literal(Bytes::from_static(b"aba"), Bytes::from_static(b"X"), 64)
        .await
        .unwrap();
    assert_eq!(result.bytes_scanned, 10);
    assert_eq!(replay_all(&mut storage, 1).await, Bytes::from_static(b"Xba--X"));
    assert_eq!(replay_all(&mut storage, 4).await, Bytes::from_static(b"Xba--X"));
    assert!(!view.is_materialized());
}

#[tokio::test]
async fn identity_literal_replace_does_not_scan_publish_or_materialize() {
    for memory_limit in [64, 1] {
        let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
        capture(&mut storage, &[b"same-same"]).await;
        let generation = view.generation();

        let result = view
            .replace_literal(Bytes::from_static(b"same"), Bytes::from_static(b"same"), 0)
            .await
            .unwrap();
        assert_eq!(result.bytes_scanned, 0);
        assert_eq!(view.generation(), generation);
        assert!(!view.is_modified());
        assert!(!view.is_materialized());
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"same-same"));
    }
}

#[tokio::test]
async fn physical_cap_failure_rolls_back_without_changing_replay() {
    let (mut storage, view) = BodyStorage::with_limits(1, 6);
    capture(&mut storage, &[b"abcabc"]).await;
    let error = view
        .replace_literal(Bytes::from_static(b"a"), Bytes::from_static(b"long"), 6)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        BodyOperationError::Body(BodyReadError::BufferLimitExceeded { limit: 6 })
    );
    assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"abcabc"));
}

#[tokio::test]
async fn range_budget_is_checked_before_output_capacity() {
    let source = Arc::new(BodySource::Disk {
        file: Arc::new(tempfile().unwrap()),
        len: u64::MAX,
    });
    let snapshot = Arc::new(BodySnapshot::from_source(Arc::clone(&source)).unwrap());
    let inner = Arc::new(BodyStorageViewInner {
        initial: OnceLock::new(),
        materialized: OnceCell::new(),
        mutation: RwLock::new(MutationState::default()),
        max_body_size_bytes: u64::MAX,
        storage_operation_timeout: DEFAULT_STORAGE_OPERATION_TIMEOUT,
    });
    inner.initial.set(InitialBody { source, snapshot }).ok().unwrap();
    let view = BodyStorageView::new(inner);

    assert_eq!(
        view.read_range(0, u64::MAX, 1).await,
        Err(BodyOperationError::ScanLimitExceeded { limit: 1 })
    );
    assert_eq!(
        view.read_text_range(0, u64::MAX, usize::MAX, 1).await,
        Err(BodyOperationError::ScanLimitExceeded { limit: 1 })
    );
}

#[tokio::test]
async fn byte_no_ops_do_not_publish_a_generation() {
    let (mut empty_storage, empty_view) = BodyStorage::with_memory_limit(64);
    capture(&mut empty_storage, &[]).await;
    empty_view.clear().unwrap();
    assert_eq!(empty_view.generation(), 0);
    assert!(!empty_view.is_modified());

    let (mut storage, view) = BodyStorage::with_memory_limit(64);
    capture(&mut storage, &[b"body"]).await;
    view.remove(2..2).unwrap();
    let generation = view.generation();
    assert_eq!(
        view.replace_spans(
            generation,
            &[(0..0, Bytes::new()), (2..2, Bytes::new()), (4..4, Bytes::new()),],
        )
        .unwrap(),
        generation
    );
    assert_eq!(
        view.replace_slice(1..1, Bytes::new()).await.unwrap(),
        Bytes::from_static(b"body")
    );
    assert_eq!(view.generation(), 0);
    assert!(!view.is_modified());
    assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"body"));
}

#[tokio::test]
async fn oversized_edit_input_is_refused_without_publication() {
    let (mut storage, view) = BodyStorage::with_memory_limit(64);
    capture(&mut storage, &[b"body"]).await;
    let edits = vec![(0..0, Bytes::from_static(b"x")); MAX_BODY_SEGMENTS + 1];

    assert_eq!(
        view.replace_spans(view.generation(), &edits),
        Err(BodyOperationError::SegmentLimitExceeded {
            limit: MAX_BODY_SEGMENTS
        })
    );
    assert_eq!(view.generation(), 0);
    assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"body"));
}

#[tokio::test]
async fn scan_limit_reports_actual_early_exit_and_hard_limit() {
    let (mut storage, view) = BodyStorage::with_memory_limit(64);
    capture(&mut storage, &[b"needle and trailing bytes"]).await;
    let early = view
        .text_operation(BodyTextOperation::Contains("needle".to_string()), 3)
        .await
        .unwrap_err();
    assert_eq!(early, BodyOperationError::ScanLimitExceeded { limit: 3 });
    let found = view
        .text_operation(BodyTextOperation::Contains("needle".to_string()), 6)
        .await
        .unwrap();
    assert_eq!(found.value, BodyTextResult::Bool(true));
    assert_eq!(found.bytes_scanned, 6);
}

#[tokio::test]
async fn empty_text_patterns_and_empty_metadata_do_not_scan() {
    let (mut storage, view) = BodyStorage::with_memory_limit(64);
    capture(&mut storage, &[b"non-empty"]).await;
    for operation in [
        BodyTextOperation::StartsWith(String::new()),
        BodyTextOperation::EndsWith(String::new()),
        BodyTextOperation::Contains(String::new()),
    ] {
        let result = view.text_operation(operation, 0).await.unwrap();
        assert_eq!(result.value, BodyTextResult::Bool(true));
        assert_eq!(result.bytes_scanned, 0);
    }
    let result = view.text_operation(BodyTextOperation::IsEmpty, 0).await.unwrap();
    assert_eq!(result.value, BodyTextResult::Bool(false));
    assert_eq!(result.bytes_scanned, 0);
}

#[tokio::test]
async fn rewind_keeps_snapshot_stable_during_later_mutation() {
    let (mut storage, view) = BodyStorage::with_memory_limit(64);
    capture(&mut storage, &[b"original"]).await;
    storage.rewind().await.unwrap();
    view.overwrite_body(Bytes::from_static(b"new")).unwrap();
    let mut old = BytesMut::new();
    while let Some(chunk) = storage.next_chunk(2).await.unwrap() {
        let len = chunk.len();
        old.extend_from_slice(&chunk);
        storage.consume(len);
    }
    assert_eq!(old.freeze(), Bytes::from_static(b"original"));
    assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"new"));
}

#[tokio::test]
async fn replace_slice_compatibility_is_atomic() {
    let (mut storage, view) = BodyStorage::with_limits(1, 6);
    capture(&mut storage, &[b"abcdef"]).await;
    assert_eq!(
        view.replace_slice(1..2, Bytes::from_static(b"123456")).await,
        Err(BodyReadError::BufferLimitExceeded { limit: 6 })
    );
    assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"abcdef"));
    assert_eq!(
        view.replace_slice(1..2, Bytes::from_static(b"Z")).await.unwrap(),
        Bytes::from_static(b"aZcdef")
    );
}

#[tokio::test]
async fn segment_limit_failure_is_atomic() {
    let (mut storage, view) = BodyStorage::with_limits(64, u64::MAX);
    capture(&mut storage, &[b"x"]).await;
    for _ in 1..MAX_BODY_SEGMENTS {
        view.append(Bytes::from_static(b"x")).unwrap();
    }
    assert_eq!(view.len().unwrap(), MAX_BODY_SEGMENTS as u64);
    let generation = view.generation();
    assert_eq!(
        view.append(Bytes::from_static(b"x")),
        Err(BodyOperationError::SegmentLimitExceeded {
            limit: MAX_BODY_SEGMENTS
        })
    );
    assert_eq!(
        view.prepend(Bytes::from_static(b"x")),
        Err(BodyOperationError::SegmentLimitExceeded {
            limit: MAX_BODY_SEGMENTS
        })
    );
    assert_eq!(view.generation(), generation);
    assert_eq!(view.len().unwrap(), MAX_BODY_SEGMENTS as u64);
}

#[tokio::test]
async fn replay_cursor_is_segment_local_and_resets_for_retry() {
    let (mut storage, view) = BodyStorage::with_limits(64, u64::MAX);
    capture(&mut storage, &[b"x"]).await;
    let segmented =
        BodySnapshot::from_segments(vec![Segment::Inline(Bytes::from_static(b"x")); MAX_BODY_SEGMENTS]).unwrap();
    view.publish(view.generation(), segmented).unwrap();

    for _ in 0..2 {
        assert_eq!(replay_all(&mut storage, 1).await.len(), MAX_BODY_SEGMENTS);
        assert_eq!(storage.replay_segment_index, MAX_BODY_SEGMENTS);
        assert_eq!(storage.replay_segment_offset, 0);
    }
}

#[tokio::test]
async fn multi_chunk_spill_materializes_directly_after_mutation() {
    let body = vec![b'd'; SCAN_CHUNK_SIZE * 2 + 17];
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[&body]).await;
    view.append(Bytes::from_static(b"-tail")).unwrap();

    let materialized = view.read_all().await.unwrap();
    assert_eq!(&materialized[..body.len()], body.as_slice());
    assert_eq!(&materialized[body.len()..], b"-tail");
    assert!(!view.is_materialized());
}

#[test]
fn memory_scan_borrows_chunks_without_promoting_unique_bytes() {
    struct BorrowedScanner {
        address: usize,
        seen: usize,
    }

    impl RawBodyScanner for BorrowedScanner {
        type Output = usize;

        fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
            assert_eq!(chunk.as_ptr() as usize, self.address + self.seen);
            assert!(!chunk.is_empty() && chunk.len() <= SCAN_CHUNK_SIZE);
            assert!(chunk.iter().all(|byte| *byte == b'x'));
            self.seen += chunk.len();
            Ok(ScanControl::Continue)
        }

        fn finish(self) -> Result<usize, BodyOperationError> {
            Ok(self.seen)
        }
    }

    let len = SCAN_CHUNK_SIZE * 2 + 17;
    for source_range in [false, true] {
        let offset = if source_range { 19 } else { 0 };
        // Unique boxed storage exposes the first Bytes::slice promotion cost.
        let bytes = Bytes::from(vec![b'x'; offset + len].into_boxed_slice());
        let address = bytes.as_ptr() as usize + offset;
        let segment = if source_range {
            Segment::SourceRange {
                source: Arc::new(BodySource::Memory(bytes)),
                offset: offset as u64,
                len: len as u64,
            }
        } else {
            Segment::Inline(bytes)
        };
        let snapshot = Arc::new(BodySnapshot::from_segments(vec![segment]).unwrap());
        let scanner = BorrowedScanner { address, seen: 0 };
        let (result, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            scan_snapshot_owned(Arc::clone(&snapshot), scanner, len as u64, None)
        });
        let result = result.unwrap();
        assert_eq!(result.bytes_scanned, len as u64);
        assert_eq!(result.value, len);
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }
}

struct SlowCountingScanner {
    chunks: Arc<std::sync::atomic::AtomicUsize>,
}

impl RawBodyScanner for SlowCountingScanner {
    type Output = ();

    fn push(&mut self, _chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        self.chunks.fetch_add(1, Ordering::AcqRel);
        std::thread::sleep(std::time::Duration::from_millis(10));
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(())
    }
}

struct BlockingContextScanner {
    caller: std::thread::ThreadId,
    ran_off_caller: bool,
}

impl RawBodyScanner for BlockingContextScanner {
    type Output = bool;

    fn requires_blocking_context(&self) -> bool {
        true
    }

    fn push(&mut self, _chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        self.ran_off_caller = std::thread::current().id() != self.caller;
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(self.ran_off_caller)
    }
}

struct FinishCountingScanner {
    finishes: Arc<std::sync::atomic::AtomicUsize>,
}

impl RawBodyScanner for FinishCountingScanner {
    type Output = ();

    fn requires_blocking_context(&self) -> bool {
        true
    }

    fn push(&mut self, _: &[u8]) -> Result<ScanControl, BodyOperationError> {
        unreachable!("finish-only scanner must not receive body bytes")
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        self.finishes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

#[tokio::test]
async fn blocking_work_waits_for_scan_lane_admission() {
    let limiter = Arc::new(Semaphore::new(1));
    let held = Arc::clone(&limiter).acquire_owned().await.unwrap();
    let finishes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let scanner = FinishCountingScanner {
        finishes: Arc::clone(&finishes),
    };
    let mut future = Box::pin(run_blocking_scan_with_limiter(move || scanner.finish(), limiter));

    poll_fn(|context| match future.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("blocking finish bypassed scan-lane admission"),
    })
    .await;
    assert_eq!(finishes.load(Ordering::Acquire), 0);

    drop(held);
    future.await.unwrap();
    assert_eq!(finishes.load(Ordering::Acquire), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_scanner_uses_blocking_pool_for_small_memory_body() {
    let (mut storage, view) = BodyStorage::with_memory_limit(64);
    capture(&mut storage, &[b"small"]).await;
    let result = view
        .scan_raw(
            BlockingContextScanner {
                caller: std::thread::current().id(),
                ran_off_caller: false,
            },
            u64::MAX,
        )
        .await
        .unwrap();
    assert!(result.value);
}

#[tokio::test]
async fn cancelled_disk_scan_stops_between_chunks() {
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    let body = vec![b'x'; SCAN_CHUNK_SIZE * 16];
    capture(&mut storage, &[&body]).await;
    let chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let task_view = Arc::clone(&view);
    let task_chunks = Arc::clone(&chunks);
    let task = tokio::spawn(async move {
        task_view
            .scan_raw(SlowCountingScanner { chunks: task_chunks }, u64::MAX)
            .await
    });
    while chunks.load(Ordering::Acquire) == 0 {
        tokio::task::yield_now().await;
    }
    task.abort();
    let _ = task.await;
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    assert!(
        chunks.load(Ordering::Acquire) < 16,
        "cancelled disk scan must not consume the complete snapshot"
    );
}

#[tokio::test]
async fn cancelled_disk_peek_replays_same_chunk() {
    let (mut storage, _view) = BodyStorage::with_memory_limit(1);
    let body = vec![b'x'; 256 * 1024];
    capture(&mut storage, &[&body]).await;
    storage.rewind().await.unwrap();
    let mut peek = Box::pin(storage.next_chunk(64 * 1024));
    let completed = poll_fn(|cx| match peek.as_mut().poll(cx) {
        Poll::Pending => Poll::Ready(None),
        Poll::Ready(result) => Poll::Ready(Some(result)),
    })
    .await;
    drop(peek);
    let expected = Bytes::copy_from_slice(&body[..64 * 1024]);
    if let Some(result) = completed {
        assert_eq!(result.unwrap(), Some(expected.clone()));
    }
    assert_eq!(storage.next_chunk(64 * 1024).await.unwrap(), Some(expected));
}

#[tokio::test]
async fn truncated_spill_fails_for_scan_and_replay() {
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[b"abcdefghij"]).await;
    let file = disk_file(&storage);
    file.set_len(4).unwrap();
    assert_eq!(
        view.sha256_scan(u64::MAX).await.unwrap_err(),
        BodyOperationError::Body(BodyReadError::StorageFailure)
    );
    storage.rewind().await.unwrap();
    assert!(storage.next_chunk(64).await.is_err());
}

#[tokio::test]
async fn later_chunk_truncation_is_a_storage_failure() {
    let body = vec![b'x'; SCAN_CHUNK_SIZE * 2 + 17];
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[&body]).await;
    disk_file(&storage).set_len(SCAN_CHUNK_SIZE as u64 + 3).unwrap();

    assert_eq!(
        view.sha256_scan(u64::MAX).await.unwrap_err(),
        BodyOperationError::Body(BodyReadError::StorageFailure)
    );
    assert_eq!(view.read_all().await, Err(BodyReadError::StorageFailure));
}

#[test]
fn positional_read_retries_short_reads_until_filled() {
    let source = b"abcdefgh";
    let mut calls = 0usize;
    let body = read_at_most_with(source.len(), 0, |target, offset| {
        calls += 1;
        let start = usize::try_from(offset).unwrap();
        let count = target.len().min(2).min(source.len() - start);
        target[..count].copy_from_slice(&source[start..start + count]);
        Ok(count)
    })
    .unwrap();
    assert_eq!(body, source);
    assert_eq!(calls, 4);
}

#[test]
fn literal_match_limit_rejects_before_growing_past_the_bound() {
    let mut scanner = LiteralMatchScanner::new(vec![b'a']);
    let input = vec![b'a'; MAX_BODY_SEGMENTS + 1];

    assert_eq!(
        scanner.push(&input),
        Err(BodyOperationError::SegmentLimitExceeded {
            limit: MAX_BODY_SEGMENTS
        })
    );
    assert_eq!(scanner.matches.len(), MAX_BODY_SEGMENTS);
}

#[tokio::test]
async fn dropping_storage_releases_its_anonymous_spill_while_snapshot_view_stays_readable() {
    let (mut storage, view) = BodyStorage::with_memory_limit(1);
    capture(&mut storage, &[b"persistent-body"]).await;
    drop(storage);
    assert_eq!(
        view.read_range(0, 64, 64).await.unwrap().value,
        Bytes::from_static(b"persistent-body")
    );
}

#[tokio::test]
async fn resolved_memory_only_capacity_refuses_spill_and_bounds_mutation() {
    let policy = crate::request_body::ResolvedRequestBodyConfig {
        memory_buffer_bytes: 4,
        max_buffered_size_bytes: 8,
        max_body_size_bytes: 16,
        spill_to_disk: false,
        ..Default::default()
    };
    let (mut storage, view) = BodyStorage::with_storage_timeout(
        policy.memory_buffer_bytes,
        policy.capture_limit(),
        policy.storage_operation_timeout,
    );
    storage.write(&Bytes::from_static(b"four")).await.unwrap();
    let error = storage.write(&Bytes::from_static(b"!")).await.unwrap_err();
    assert!(matches!(error.etype(), ErrorType::HTTPStatus(413)));
    assert!(matches!(storage.storage, CaptureStorage::Memory(_)));
    storage.finish().await.unwrap();
    assert!(view.overwrite_body(Bytes::from_static(b"longer")).is_err());
    assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"four"));
    // A separately created complete owner is not retroactively constrained.
    let (mut existing, existing_view) = BodyStorage::with_limits(1, 16);
    capture(&mut existing, &[b"existing"]).await;
    existing_view
        .overwrite_body(Bytes::from_static(b"still-longer"))
        .unwrap();
    assert_eq!(replay_all(&mut existing, 3).await, Bytes::from_static(b"still-longer"));
}

#[tokio::test]
async fn owner_capacity_allows_mutation_beyond_initial_capture_length() {
    let (mut storage, view) = BodyStorage::with_limits(2, 8);
    capture(&mut storage, &[b"tiny"]).await;
    view.overwrite_body(Bytes::from_static(b"expanded")).unwrap();
    assert!(view.overwrite_body(Bytes::from_static(b"oversized")).is_err());
    assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"expanded"));
}
