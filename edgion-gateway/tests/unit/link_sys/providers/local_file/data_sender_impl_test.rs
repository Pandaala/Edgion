use super::*;
use std::sync::atomic::Ordering;

/// A writer whose `write` succeeds into the BufWriter buffer but whose inner
/// `write` fails at flush time — exactly the buffered-Ok-but-flush-fails
/// shape that a full disk produces.
struct FlushFailWriter;

impl Write for FlushFailWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("write failed"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("flush failed"))
    }
}

#[test]
fn compute_healthy_threshold() {
    assert!(compute_healthy(0, true));
    assert!(compute_healthy(2, true)); // below threshold (3)
    assert!(!compute_healthy(3, true)); // at threshold
    assert!(!compute_healthy(0, false)); // disk over limit
}

#[test]
fn reopen_sink_if_renamed_recreates_after_external_rename() {
    // Models the backfill scenario: the active fallback file is renamed to
    // `*.backfilled-<ts>` out from under the writer. The writer must reclaim
    // a fresh active file so the next ES degrade does not write into the
    // already-backfilled (and forever-skipped) file.
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("es_fallback.log");
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let mut sink = BatchSink::new(
        BufWriter::new(file),
        LogType::Access,
        0,
        Arc::new(AtomicBool::new(true)),
        identity,
    );

    // Intact active file: no reopen needed.
    assert!(
        !reopen_sink_if_renamed(&mut sink, &base, |file| file),
        "intact file must not be reopened"
    );

    // Backfill renames the active file away.
    fs::rename(&base, dir.path().join("es_fallback.log.backfilled-1")).unwrap();
    assert!(!base.exists());

    // The writer reclaims a fresh active file at base_path.
    assert!(
        reopen_sink_if_renamed(&mut sink, &base, |file| file),
        "a renamed-away active file must be reopened"
    );
    assert!(base.exists(), "base path is recreated as a fresh active file");
}

#[test]
fn disk_ok_disabled_variants_are_healthy() {
    assert!(disk_ok(Path::new("."), None));
    assert!(disk_ok(Path::new("."), Some(0)));
    // A 100% threshold is effectively "only shed when truly full".
    assert!(disk_ok(Path::new("."), Some(100)));
}

#[test]
fn batch_sink_flips_unhealthy_after_threshold_flush_failures() {
    let healthy = Arc::new(AtomicBool::new(true));
    // Default-capacity BufWriter buffers the line, so write_line sees Ok;
    // the failure surfaces at flush, the real disk-full path.
    let mut sink = BatchSink::new(
        BufWriter::new(FlushFailWriter),
        LogType::Access,
        0,
        Arc::clone(&healthy),
        None,
    );

    for _ in 0..2 {
        sink.write_line("line");
        sink.flush();
    }
    assert!(healthy.load(Ordering::Relaxed), "2 failures < threshold, still healthy");

    sink.write_line("line");
    sink.flush(); // third failure
    assert!(!healthy.load(Ordering::Relaxed), "3rd flush failure flips unhealthy");
    assert_eq!(sink.current_size, 0, "failed flushes never advance on-disk size");
}

#[test]
fn batch_sink_recovers_on_successful_flush() {
    let healthy = Arc::new(AtomicBool::new(false));
    let mut sink = BatchSink::new(
        BufWriter::new(Vec::<u8>::new()),
        LogType::Access,
        0,
        Arc::clone(&healthy),
        None,
    );
    // Simulate a prior degraded state.
    sink.consecutive_flush_errors = 5;
    sink.last_published_healthy = false;

    sink.write_line("hello");
    sink.flush(); // Vec never fails -> success resets and republishes healthy
    assert!(healthy.load(Ordering::Relaxed), "successful flush recovers health");
    assert_eq!(sink.consecutive_flush_errors, 0);
    assert_eq!(sink.current_size, 6, "\"hello\" + newline promoted on flush Ok");
}

/// Inner writer whose every `write` fails immediately. With a capacity-0
/// `BufWriter`, each `writeln!` forwards straight through, so `write_line`
/// itself sees the error (exercises the buffer-level error path).
struct WriteFailWriter;

impl Write for WriteFailWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("write failed"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn batch_sink_write_line_error_debounces() {
    let healthy = Arc::new(AtomicBool::new(true));
    let mut sink = BatchSink::new(
        BufWriter::with_capacity(0, WriteFailWriter),
        LogType::Access,
        0,
        Arc::clone(&healthy),
        None,
    );
    sink.write_line("a");
    assert!(sink.buffer_error_logged, "first buffer error sets the debounce flag");
    assert_eq!(sink.pending_lines, 0, "failed lines are not buffered");
    sink.write_line("b");
    assert!(sink.buffer_error_logged, "debounce flag stays set across failing lines");
    // A successful flush (WriteFailWriter.flush is Ok, buffer empty) clears it.
    sink.flush();
    assert!(
        !sink.buffer_error_logged,
        "successful flush resets the buffer-error debounce"
    );
}

#[test]
fn batch_sink_disk_signal_flips_health() {
    let healthy = Arc::new(AtomicBool::new(true));
    let mut sink = BatchSink::new(
        BufWriter::new(Vec::<u8>::new()),
        LogType::Access,
        0,
        Arc::clone(&healthy),
        None,
    );
    sink.set_disk_ok(false);
    assert!(!healthy.load(Ordering::Relaxed), "disk over threshold -> unhealthy");
    sink.set_disk_ok(true);
    assert!(healthy.load(Ordering::Relaxed), "disk back under threshold -> healthy");
}

/// Inner writer whose failure is toggled at runtime, to exercise the
/// idle-recovery path (fail while degrading, then succeed on the retry).
struct ToggleFailWriter {
    fail: Arc<AtomicBool>,
    file: fs::File,
}

impl Write for ToggleFailWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.fail.load(Ordering::Relaxed) {
            Err(std::io::Error::other("write failed"))
        } else {
            self.file.write(buf)
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.fail.load(Ordering::Relaxed) {
            Err(std::io::Error::other("flush failed"))
        } else {
            self.file.flush()
        }
    }
}

#[test]
fn batch_sink_idle_retry_recovers_after_failures_clear() {
    let healthy = Arc::new(AtomicBool::new(true));
    let fail = Arc::new(AtomicBool::new(true));
    let file = tempfile::tempfile().unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let inspect_file = file.try_clone().unwrap();
    let mut sink = BatchSink::new(
        BufWriter::new(ToggleFailWriter {
            fail: Arc::clone(&fail),
            file,
        }),
        LogType::Access,
        0,
        Arc::clone(&healthy),
        identity,
    );

    // Three failed flushes drive the sink unhealthy.
    for _ in 0..3 {
        sink.write_line("line");
        sink.flush();
    }
    assert!(!healthy.load(Ordering::Relaxed), "3 flush failures -> unhealthy");
    assert_eq!(sink.pending_bytes, 0, "failed batch accounting is cleared");
    assert_eq!(inspect_file.metadata().unwrap().len(), 0);

    // Disk clears; an idle retry (no new write_line, i.e. no traffic)
    // re-attempts the retained bytes and recovers the shed sink.
    fail.store(false, Ordering::Relaxed);
    sink.retry_flush_for_recovery();
    assert!(
        healthy.load(Ordering::Relaxed),
        "idle flush retry recovers a sink that receives no traffic"
    );
    assert_eq!(sink.consecutive_flush_errors, 0);
    assert_eq!(inspect_file.metadata().unwrap().len(), 15);
}

#[test]
fn size_rotation_keeps_base_path_active_when_archive_is_backfilled() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("es_fallback.log");
    let healthy = Arc::new(AtomicBool::new(true));
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let mut sink = BatchSink::new(BufWriter::new(file), LogType::Access, 0, healthy, identity);

    sink.write_line("before rotation");
    assert!(sink.flush());
    assert!(rotate_for_size(&mut sink, &base, 0, true, |file| file));

    let archive = get_size_rotated_path(&base, 1);
    assert!(base.exists(), "size rotation must reopen the base path as active");
    assert!(archive.exists(), "the prior base file becomes the indexed archive");

    let backfilled = dir.path().join("es_fallback.1.log.backfilled-1");
    fs::rename(&archive, &backfilled).unwrap();
    assert!(
        !reopen_sink_if_renamed(&mut sink, &base, |file| file),
        "renaming an archive must not look like an active-file rename"
    );

    sink.write_line("after rotation");
    assert!(sink.flush());
    assert_eq!(fs::read_to_string(&base).unwrap(), "after rotation\n");
    assert_eq!(fs::read_to_string(&backfilled).unwrap(), "before rotation\n");
}

#[test]
fn batch_sink_idle_retry_is_noop_when_not_in_error() {
    // Not in a flush-error state: retry must not touch health or counters.
    let healthy = Arc::new(AtomicBool::new(true));
    let mut sink = BatchSink::new(
        BufWriter::new(Vec::<u8>::new()),
        LogType::Access,
        0,
        Arc::clone(&healthy),
        None,
    );
    sink.retry_flush_for_recovery();
    assert!(healthy.load(Ordering::Relaxed));
    assert_eq!(sink.consecutive_flush_errors, 0);
}

#[test]
fn external_rename_waits_for_failed_buffer_to_flush_before_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("fallback.log");
    let renamed = dir.path().join("fallback.log.backfilled-1");
    let fail = Arc::new(AtomicBool::new(true));
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let mut sink = BatchSink::new(
        BufWriter::new(ToggleFailWriter {
            fail: Arc::clone(&fail),
            file,
        }),
        LogType::Access,
        0,
        Arc::new(AtomicBool::new(true)),
        identity,
    );

    sink.write_line("retained");
    assert!(!sink.flush());
    fs::rename(&base, &renamed).unwrap();
    assert!(!reopen_sink_if_renamed(&mut sink, &base, |file| {
        ToggleFailWriter {
            fail: Arc::clone(&fail),
            file,
        }
    }));
    assert!(!base.exists(), "a failed old flush must block writer replacement");
    assert_eq!(sink.pending_bytes, 0, "failed batch accounting is cleared");

    fail.store(false, Ordering::Relaxed);
    assert!(reopen_sink_if_renamed(&mut sink, &base, |file| {
        ToggleFailWriter {
            fail: Arc::clone(&fail),
            file,
        }
    }));
    assert_eq!(sink.pending_bytes, 0);
    assert_eq!(sink.current_size, 0, "the fresh base starts with empty accounting");
    assert_eq!(fs::read_to_string(&renamed).unwrap(), "retained\n");
    assert_eq!(fs::read_to_string(&base).unwrap(), "");
}

#[test]
fn missing_base_reopen_failure_marks_attachment_unhealthy() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("fallback.log");
    let renamed = dir.path().join("fallback.log.backfilled-1");
    let healthy = Arc::new(AtomicBool::new(true));
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let mut sink = BatchSink::new(BufWriter::new(file), LogType::Access, 0, Arc::clone(&healthy), identity);

    sink.write_line("retained");
    fs::rename(&base, &renamed).unwrap();
    assert!(!reopen_sink_if_renamed_with(
        &mut sink,
        &base,
        |_| Err(std::io::Error::other("reopen failed")),
        |file| file,
    ));

    assert_eq!(sink.pending_bytes, 0, "the old writer was flushed first");
    assert!(!sink.path_ok, "canonical attachment is no longer confirmed");
    assert!(!healthy.load(Ordering::Relaxed));
    assert_eq!(fs::read_to_string(&renamed).unwrap(), "retained\n");
    assert!(!base.exists());
}

#[test]
fn unhealthy_attachment_reopens_externally_recreated_base_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("fallback.log");
    let renamed = dir.path().join("fallback.log.backfilled-1");
    let healthy = Arc::new(AtomicBool::new(true));
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let mut sink = BatchSink::new(BufWriter::new(file), LogType::Access, 0, Arc::clone(&healthy), identity);

    fs::rename(&base, &renamed).unwrap();
    fs::File::create(&base).unwrap();
    sink.set_path_ok(false);
    sink.write_line("old inode");

    assert!(reopen_sink_if_renamed(&mut sink, &base, |file| file));
    assert!(sink.path_ok);
    assert!(healthy.load(Ordering::Relaxed));
    assert_eq!(fs::read_to_string(&renamed).unwrap(), "old inode\n");

    sink.write_line("canonical");
    assert!(sink.flush());
    assert_eq!(fs::read_to_string(&base).unwrap(), "canonical\n");
}

#[cfg(unix)]
#[test]
fn healthy_attachment_reopens_recreated_base_and_preserves_retained_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("access.log");
    let archived = dir.path().join("access.archived.log");
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let healthy = Arc::new(AtomicBool::new(true));
    let mut sink = BatchSink::new(BufWriter::new(file), LogType::Access, 0, Arc::clone(&healthy), identity);
    sink.write_line("retained");
    fs::rename(&base, &archived).unwrap();
    fs::write(&base, "existing\n").unwrap();
    assert!(sink.path_ok && healthy.load(Ordering::Relaxed));

    assert!(reopen_sink_if_renamed_with(
        &mut sink,
        &base,
        |path| {
            assert!(
                !healthy.load(Ordering::Relaxed),
                "old flush cannot restore attachment health"
            );
            open_log_file(path)
        },
        |file| file,
    ));
    assert_ne!(sink.identity, identity);
    assert_eq!(sink.current_size, 9);
    assert!(sink.path_ok && healthy.load(Ordering::Relaxed));
    assert_eq!(fs::read_to_string(&archived).unwrap(), "retained\n");
    sink.write_line("canonical");
    assert!(sink.flush());
    assert_eq!(fs::read_to_string(&base).unwrap(), "existing\ncanonical\n");
    assert!(!reopen_sink_if_renamed_with(
        &mut sink,
        &base,
        |_| panic!("matching attachment must not reopen"),
        |file| file,
    ));
}

#[cfg(unix)]
#[test]
fn recreated_base_waits_for_failed_flush_and_recovers_without_new_lines() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("access.log");
    let archived = dir.path().join("access.archived.log");
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let fail = Arc::new(AtomicBool::new(true));
    let healthy = Arc::new(AtomicBool::new(true));
    let mut sink = BatchSink::new(
        BufWriter::new(ToggleFailWriter {
            fail: Arc::clone(&fail),
            file,
        }),
        LogType::Access,
        0,
        Arc::clone(&healthy),
        identity,
    );
    sink.write_line("retained");
    fs::rename(&base, &archived).unwrap();
    fs::write(&base, "existing\n").unwrap();
    assert!(!reopen_sink_if_renamed_with(
        &mut sink,
        &base,
        |_| panic!("failed old flush must block open"),
        |file| ToggleFailWriter {
            fail: Arc::clone(&fail),
            file
        },
    ));
    assert_eq!(sink.identity, identity);
    assert!(!sink.path_ok && !healthy.load(Ordering::Relaxed));
    assert!(!sink.file.buffer().is_empty());
    fail.store(false, Ordering::Relaxed);
    assert!(reopen_sink_if_renamed(&mut sink, &base, |file| ToggleFailWriter {
        fail: Arc::clone(&fail),
        file,
    }));
    assert_eq!(fs::read_to_string(&archived).unwrap(), "retained\n");
    assert_eq!(fs::read_to_string(&base).unwrap(), "existing\n");
    assert_eq!(sink.current_size, 9);
    assert!(sink.path_ok && healthy.load(Ordering::Relaxed));
}

#[cfg(unix)]
#[test]
fn failed_recreated_base_open_blocks_rotation_and_later_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("access.log");
    let old = dir.path().join("old.log");
    let archive = dir.path().join("access.1.log");
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let healthy = Arc::new(AtomicBool::new(true));
    let mut sink = BatchSink::new(BufWriter::new(file), LogType::Access, 0, Arc::clone(&healthy), identity);
    sink.write_line("retained");
    fs::rename(&base, &old).unwrap();
    fs::write(&base, "new generation\n").unwrap();
    // Rotation itself must refuse to archive a detached writer's newer base.
    assert_eq!(
        rotate_active_file(&mut sink, &base, &archive, 0, true, |file| file),
        RotationOutcome::Failed
    );
    assert!(!archive.exists());
    assert!(!reopen_sink_if_renamed_with(
        &mut sink,
        &base,
        |_| Err(std::io::Error::other("open failed")),
        |file| file,
    ));
    assert_eq!(sink.identity, identity);
    assert!(!healthy.load(Ordering::Relaxed));
    assert_eq!(
        rotate_active_file(&mut sink, &base, &archive, 0, true, |file| file),
        RotationOutcome::Failed
    );
    assert!(!archive.exists());
    assert_eq!(fs::read_to_string(&old).unwrap(), "retained\n");
    assert_eq!(fs::read_to_string(&base).unwrap(), "new generation\n");
    assert!(reopen_sink_if_renamed(&mut sink, &base, |file| file));
    sink.write_line("recovered");
    assert!(sink.flush());
    assert_eq!(fs::read_to_string(&base).unwrap(), "new generation\nrecovered\n");
}

#[cfg(unix)]
#[test]
fn raced_candidate_path_is_rejected_and_stable_checkpoint_recovers() {
    for recreate in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("access.log");
        let old = dir.path().join("old.log");
        let raced = dir.path().join("raced.log");
        let file = open_log_file(&base).unwrap();
        let identity = Some(opened_file_state(&file).unwrap().0);
        let healthy = Arc::new(AtomicBool::new(true));
        let mut sink = BatchSink::new(BufWriter::new(file), LogType::Access, 0, Arc::clone(&healthy), identity);
        sink.write_line("retained");
        fs::rename(&base, &old).unwrap();
        assert!(!reopen_sink_if_renamed_with(
            &mut sink,
            &base,
            |path| {
                let candidate = open_log_file(path)?;
                fs::rename(path, &raced)?;
                if recreate {
                    fs::write(path, "new generation\n")?;
                }
                // Descriptor metadata remains valid after pathname replacement.
                assert!(candidate.metadata().is_ok());
                Ok(candidate)
            },
            |file| file,
        ));
        assert_eq!(sink.identity, identity);
        assert!(!sink.path_ok && !healthy.load(Ordering::Relaxed));
        assert_eq!(fs::read_to_string(&old).unwrap(), "retained\n");
        assert_eq!(fs::read_to_string(&raced).unwrap(), "");
        assert!(reopen_sink_if_renamed(&mut sink, &base, |file| file));
        assert!(sink.path_ok && healthy.load(Ordering::Relaxed));
        sink.write_line("canonical");
        assert!(sink.flush());
        let expected = if recreate {
            "new generation\ncanonical\n"
        } else {
            "canonical\n"
        };
        assert_eq!(fs::read_to_string(&base).unwrap(), expected);
    }
}

#[cfg(unix)]
#[test]
fn active_path_metadata_error_preserves_writer_until_path_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("logs");
    fs::create_dir(&parent).unwrap();
    let base = parent.join("access.log");
    let moved = dir.path().join("old-logs");
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let healthy = Arc::new(AtomicBool::new(true));
    let mut sink = BatchSink::new(BufWriter::new(file), LogType::Access, 0, Arc::clone(&healthy), identity);
    sink.write_line("retained");
    fs::rename(&parent, &moved).unwrap();
    fs::write(&parent, "not a directory").unwrap();
    assert!(active_path_matches(&base, identity).is_err());
    assert!(!reopen_sink_if_renamed_with(
        &mut sink,
        &base,
        |_| panic!("metadata error must defer reopening"),
        |file| file,
    ));
    assert_eq!(sink.identity, identity);
    assert!(!sink.path_ok && !healthy.load(Ordering::Relaxed));
    assert!(!sink.file.buffer().is_empty());
    fs::remove_file(&parent).unwrap();
    assert!(reopen_sink_if_renamed(&mut sink, &base, |file| file));
    assert_eq!(fs::read_to_string(moved.join("access.log")).unwrap(), "retained\n");
    sink.write_line("canonical");
    assert!(sink.flush());
    assert_eq!(fs::read_to_string(&base).unwrap(), "canonical\n");
}

#[cfg(unix)]
#[test]
fn unknown_identity_cannot_confirm_unix_attachment() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("access.log");
    let file = open_log_file(&base).unwrap();
    let mut sink = BatchSink::new(
        BufWriter::new(file),
        LogType::Access,
        0,
        Arc::new(AtomicBool::new(true)),
        None,
    );
    assert!(!active_path_matches(&base, sink.identity).unwrap());
    assert!(reopen_sink_if_renamed(&mut sink, &base, |file| file));
    assert!(active_path_matches(&base, sink.identity).unwrap());
}

#[cfg(unix)]
#[test]
fn time_rotation_refreshes_identity_and_rollback_never_overwrites_new_base() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("access.log");
    let archive = dir.path().join("access.hour.log");
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let mut sink = BatchSink::new(
        BufWriter::new(file),
        LogType::Access,
        0,
        Arc::new(AtomicBool::new(true)),
        identity,
    );
    sink.write_line("archived");
    assert_eq!(
        rotate_active_file(&mut sink, &base, &archive, 0, true, |file| file),
        RotationOutcome::Rotated
    );
    assert_ne!(sink.identity, identity);
    assert!(!reopen_sink_if_renamed(&mut sink, &base, |file| file));
    sink.write_line("active");
    assert!(sink.flush());
    assert!(restore_archived_base(&archive, &base).is_err());
    assert_eq!(fs::read_to_string(&archive).unwrap(), "archived\n");
    assert_eq!(fs::read_to_string(&base).unwrap(), "active\n");
    fs::remove_file(&base).unwrap();
    restore_archived_base(&archive, &base).unwrap();
    assert!(!archive.exists());
    assert!(active_path_matches(&base, identity).unwrap());
    assert_eq!(fs::read_to_string(&base).unwrap(), "archived\n");
}

#[test]
fn time_rotation_waits_for_failed_buffer_to_flush_before_replace() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("access.log");
    let archive = dir.path().join("access.2026-01-01.log");
    let fail = Arc::new(AtomicBool::new(true));
    let file = open_log_file(&base).unwrap();
    let identity = Some(opened_file_state(&file).unwrap().0);
    let mut sink = BatchSink::new(
        BufWriter::new(ToggleFailWriter {
            fail: Arc::clone(&fail),
            file,
        }),
        LogType::Access,
        0,
        Arc::new(AtomicBool::new(true)),
        identity,
    );

    sink.write_line("retained");
    assert!(!sink.flush());
    assert_eq!(
        rotate_active_file(&mut sink, &base, &archive, 0, true, |file| {
            ToggleFailWriter {
                fail: Arc::clone(&fail),
                file,
            }
        }),
        RotationOutcome::Failed
    );
    assert!(base.exists());
    assert!(!archive.exists());
    assert_eq!(sink.pending_bytes, 0, "failed batch accounting is cleared");

    fail.store(false, Ordering::Relaxed);
    assert_eq!(
        rotate_active_file(&mut sink, &base, &archive, 0, true, |file| {
            ToggleFailWriter {
                fail: Arc::clone(&fail),
                file,
            }
        }),
        RotationOutcome::Rotated
    );
    assert_eq!(sink.pending_bytes, 0);
    assert_eq!(sink.current_size, 0);
    assert_eq!(fs::read_to_string(&archive).unwrap(), "retained\n");
    assert_eq!(fs::read_to_string(&base).unwrap(), "");
}

#[test]
fn archive_creation_never_overwrites_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("access.log");
    let archive = dir.path().join("access.1.log");
    fs::write(&base, "active\n").unwrap();
    fs::write(&archive, "existing\n").unwrap();

    let error = archive_base_no_replace(&base, &archive).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&base).unwrap(), "active\n");
    assert_eq!(fs::read_to_string(&archive).unwrap(), "existing\n");
}

#[tokio::test]
async fn send_marks_unhealthy_when_writer_thread_is_gone() {
    // A dropped receiver models a dead/exited writer thread: send() must
    // flip healthy false so first-healthy fallback skips this zombie sink.
    let mut writer = LocalFileWriter::with_path("unused.log");
    let (tx, rx) = std::sync::mpsc::sync_channel::<String>(1);
    drop(rx);
    writer.sender = Some(tx);
    writer.healthy.store(true, Ordering::Relaxed);
    assert!(writer.healthy(), "precondition: sink starts healthy");

    writer.send("x".to_string()).await.unwrap();

    assert!(
        !writer.healthy(),
        "Disconnected channel (dead writer thread) must mark the sink unhealthy"
    );
}

#[tokio::test]
async fn send_stays_healthy_when_queue_is_full() {
    // A full bounded queue is backpressure, NOT a health fault:
    // the entry is dropped but the sink stays healthy.
    let mut writer = LocalFileWriter::with_path("unused.log");
    // Capacity 1, receiver kept alive but never drained -> next send is Full.
    let (tx, _rx) = std::sync::mpsc::sync_channel::<String>(1);
    tx.try_send("first".to_string()).unwrap();
    writer.sender = Some(tx);
    writer.healthy.store(true, Ordering::Relaxed);

    writer.send("second".to_string()).await.unwrap();

    assert!(writer.healthy(), "a full queue is backpressure, not a health fault");
}
