//! DataSender trait implementation for LocalFileWriter

use anyhow::Result;
use async_trait::async_trait;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::rotation::{
    cleanup_old_files, find_next_size_index, get_rotated_path, get_rotation_key, get_size_rotated_path, open_log_file,
};
use super::{LocalFileWriter, LogType};
use crate::link_sys::DataSender;
use crate::observe::global_metrics;
use edgion_resources::output::RotationStrategy;

/// Consecutive failed flushes before the sink reports unhealthy. Debounces a
/// single transient FS hiccup; a persistent disk-full / permission error trips
/// it within this many batches.
const UNHEALTHY_FLUSH_ERROR_THRESHOLD: u64 = 3;

/// Derive the health flag from the two live signals. Pure, so it is unit-tested
/// in isolation. (Kept trivial on purpose; the load-bearing logic lives in
/// `BatchSink`, which the FailWriter tests exercise end-to-end.)
fn compute_healthy(consecutive_flush_errors: u64, disk_ok: bool) -> bool {
    consecutive_flush_errors < UNHEALTHY_FLUSH_ERROR_THRESHOLD && disk_ok
}

/// Increment the dropped-log counter for the given log type.
fn record_drop(log_type: LogType) {
    match log_type {
        LogType::Access => global_metrics().access_log_dropped(),
        LogType::Ssl => global_metrics().ssl_log_dropped(),
        LogType::Tcp => global_metrics().tcp_log_dropped(),
        LogType::Tls => global_metrics().tls_log_dropped(),
        LogType::Udp => global_metrics().udp_log_dropped(),
    }
}

/// Filesystem usage percent (0-100) for the volume holding `path`, or `None`
/// when it cannot be determined. Unix-only; other targets return `None`.
#[cfg(unix)]
fn disk_usage_percent(path: &Path) -> Option<u8> {
    let s = rustix::fs::statvfs(path).ok()?;
    let total = s.f_blocks;
    if total == 0 {
        return None;
    }
    // f_bavail = blocks available to unprivileged users — the space we can
    // actually write into. This excludes root-reserved blocks, so usage reads
    // slightly high and shedding is marginally conservative (fine for a
    // non-root log writer).
    let used = total.saturating_sub(s.f_bavail);
    Some(((used as u128 * 100) / total as u128).min(100) as u8)
}

#[cfg(not(unix))]
fn disk_usage_percent(_path: &Path) -> Option<u8> {
    None
}

/// Whether the disk is below the configured usage threshold. `None`/`Some(0)`
/// disables the check (always healthy). A probe error is treated as healthy: do
/// not shed a fallback sink on an inconclusive probe.
fn disk_ok(path: &Path, threshold_percent: Option<u8>) -> bool {
    match threshold_percent {
        None | Some(0) => true,
        Some(t) => disk_usage_percent(path).map(|u| u < t).unwrap_or(true),
    }
}

/// Per-thread sink state for the LocalFile writer.
///
/// Owns the buffered writer and drives the shared `healthy` flag from REAL
/// outcomes. Crucially, `write_line` only buffers — `writeln!` into a
/// `BufWriter` returns `Ok` long before bytes reach the disk, so a disk-full
/// error surfaces only at `flush`. The flush outcome is therefore the
/// authoritative success/failure signal for the consecutive-error counter and
/// for promoting buffered bytes into the tracked on-disk size.
struct BatchSink<W: Write> {
    file: BufWriter<W>,
    log_type: LogType,
    /// Tracked on-disk size; advanced only on a successful flush.
    current_size: u64,
    /// Bytes buffered in the current batch; promoted on flush Ok, discarded on Err.
    pending_bytes: u64,
    /// Lines buffered in the current batch; counted as dropped on flush Err.
    pending_lines: u64,
    /// Consecutive failed flushes; feeds the health threshold.
    consecutive_flush_errors: u64,
    /// Debounce for the buffer-level write-error log. `write_line` does not
    /// touch `consecutive_flush_errors`, so it needs its own one-shot flag;
    /// reset on the next successful flush so a persistent failure logs once.
    buffer_error_logged: bool,
    /// Latest disk-probe result.
    disk_ok: bool,
    /// Shared flag read by `healthy()` on the hot path.
    healthy: Arc<AtomicBool>,
    /// Last published value, so transitions log exactly once.
    last_published_healthy: bool,
}

impl<W: Write> BatchSink<W> {
    fn new(file: BufWriter<W>, log_type: LogType, current_size: u64, healthy: Arc<AtomicBool>) -> Self {
        Self {
            file,
            log_type,
            current_size,
            pending_bytes: 0,
            pending_lines: 0,
            consecutive_flush_errors: 0,
            buffer_error_logged: false,
            disk_ok: true,
            healthy,
            // init() optimistically published `true`; match it so the first
            // real degrade is logged as a transition.
            last_published_healthy: true,
        }
    }

    /// Buffer one line. A (rare) buffer-level write error drops the line
    /// immediately; otherwise it accrues to the pending batch.
    fn write_line(&mut self, line: &str) {
        match writeln!(self.file, "{}", line) {
            Ok(()) => {
                self.pending_bytes += line.len() as u64 + 1; // +1 for newline
                self.pending_lines += 1;
            }
            Err(e) => {
                // A full BufWriter flushes mid-write, so on a full disk this arm
                // can fire for most of a batch. Debounce with a dedicated flag
                // (the flush counter is not incremented here) so a persistent
                // failure logs once, not once per line.
                if !self.buffer_error_logged {
                    tracing::error!(error = %e, "Failed to buffer log line for local file");
                    self.buffer_error_logged = true;
                }
                record_drop(self.log_type);
            }
        }
    }

    /// Flush the batch — where disk-full surfaces. Promote or discard the
    /// pending counts, update the flush-error counter, republish health.
    fn flush(&mut self) {
        match self.file.flush() {
            Ok(()) => {
                self.current_size += self.pending_bytes;
                self.buffer_error_logged = false;
                if self.consecutive_flush_errors > 0 {
                    tracing::info!(
                        recovered_after = self.consecutive_flush_errors,
                        "Local file log flush recovered after errors"
                    );
                    self.consecutive_flush_errors = 0;
                }
            }
            Err(e) => {
                if self.consecutive_flush_errors == 0 {
                    tracing::error!(error = %e, "Failed to flush log file");
                }
                self.consecutive_flush_errors += 1;
                // The whole buffered batch is lost; count every line, and do NOT
                // advance current_size (pending_bytes never reached disk).
                // NOTE: `BufWriter` retains unwritten bytes after a failed inner
                // write, so if the disk later recovers a subsequent flush may
                // still land these lines. The drop count is therefore
                // best-effort-pessimistic, not exact.
                for _ in 0..self.pending_lines {
                    record_drop(self.log_type);
                }
            }
        }
        self.pending_bytes = 0;
        self.pending_lines = 0;
        self.publish_health();
    }

    /// Update the disk-probe signal and republish health.
    fn set_disk_ok(&mut self, ok: bool) {
        self.disk_ok = ok;
        self.publish_health();
    }

    /// Idle-tick recovery: when the sink is in a flush-error state, re-attempt
    /// the flush so a recovered disk lets it return to healthy WITHOUT new
    /// traffic. A shed sink receives none (first-healthy routing skips it), so
    /// without this it could never observe recovery and would stay unhealthy
    /// until restart. No-op when not in a flush-error state; the disk-usage
    /// signal recovers separately via `set_disk_ok` on the same tick.
    fn retry_flush_for_recovery(&mut self) {
        if self.consecutive_flush_errors == 0 {
            return;
        }
        // A flush-error state means `BufWriter` still holds the unwritten bytes,
        // so this flush genuinely re-attempts them: success proves the disk is
        // writable again. An empty buffer cannot reach here (the counter is only
        // raised by a failed flush, which retains its bytes).
        if self.file.flush().is_ok() {
            self.buffer_error_logged = false;
            tracing::info!(
                recovered_after = self.consecutive_flush_errors,
                "Local file log flush recovered after errors (idle retry)"
            );
            self.consecutive_flush_errors = 0;
            self.publish_health();
        }
    }

    /// Publish the combined health flag, logging once on a transition (I4).
    fn publish_health(&mut self) {
        let now = compute_healthy(self.consecutive_flush_errors, self.disk_ok);
        if now != self.last_published_healthy {
            if now {
                tracing::warn!("LocalFile log sink recovered to healthy");
            } else {
                tracing::warn!(
                    consecutive_flush_errors = self.consecutive_flush_errors,
                    disk_ok = self.disk_ok,
                    "LocalFile log sink degraded to unhealthy"
                );
            }
            self.healthy.store(now, Ordering::Relaxed);
            self.last_published_healthy = now;
        }
    }
}

#[async_trait]
impl DataSender<String> for LocalFileWriter {
    async fn init(&mut self) -> Result<()> {
        let base_path = self.full_path();
        let rotation = self.rotation.clone();

        // Create parent directory if it doesn't exist
        if let Some(parent) = base_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Create bounded sync channel for writes
        let queue_size = self.get_queue_size();
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(queue_size);
        let log_type = self.log_type;
        let disk_threshold = self.disk_usage_threshold_percent;

        // Publish optimistic health BEFORE spawning so the spawned thread can
        // only ever correct it downward (open failure / write failure / disk).
        // This avoids a race where a late init store clobbers the thread's false.
        self.healthy.store(true, Ordering::Relaxed);
        let healthy = Arc::clone(&self.healthy);

        // Spawn background thread for file writes (avoids blocking tokio runtime)
        std::thread::spawn(move || {
            let strategy = rotation.strategy.clone();
            let max_files = rotation.max_files;

            // For time-based rotation
            let mut current_key = get_rotation_key(&strategy);
            let rotation_check_interval = Duration::from_secs(rotation.check_interval_secs);
            let mut last_rotation_check = Instant::now();

            // Active file always uses the base path (nginx-style).
            let mut current_path = base_path.clone();
            let mut initial_size: u64 = 0;
            if let RotationStrategy::Size(_) = &strategy {
                if base_path.exists() {
                    initial_size = fs::metadata(&base_path).map(|m| m.len()).unwrap_or(0);
                }
            }

            // Open initial file with buffered writer for better performance
            let file = match open_log_file(&current_path) {
                Ok(f) => BufWriter::new(f),
                Err(e) => {
                    tracing::error!(error = %e, path = %current_path.display(), "Failed to open log file");
                    // Writer thread is dead before it started — report unhealthy
                    // so first-healthy fallback skips this sink.
                    healthy.store(false, Ordering::Relaxed);
                    // Count any entries already queued before the open failed, so
                    // the loss is observable rather than silent (N2).
                    while rx.try_recv().is_ok() {
                        record_drop(log_type);
                    }
                    return;
                }
            };

            tracing::info!(path = %current_path.display(), "Log file opened");

            let mut sink = BatchSink::new(file, log_type, initial_size, healthy);
            // Seed the disk probe so an already-full disk degrades from batch one.
            sink.set_disk_ok(disk_ok(&base_path, disk_threshold));

            // Wake at least every `idle_tick` even with no traffic, so a shed
            // (unhealthy) sink can re-probe the disk and retry a flush and thus
            // recover — first-healthy routing sends it nothing while it is down.
            // Clamp to >=1s so a misconfigured 0 interval cannot busy-spin.
            let idle_tick = rotation_check_interval.max(Duration::from_secs(1));
            loop {
                let first_line = match rx.recv_timeout(idle_tick) {
                    Ok(line) => line,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // No traffic this interval: drive recovery without it.
                        sink.set_disk_ok(disk_ok(&base_path, disk_threshold));
                        sink.retry_flush_for_recovery();
                        continue;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
                // Periodic rotation/disk check (most batches skip this block); the
                // disk signal also refreshes on idle ticks above, so the signal
                // stays fresh whether or not traffic flows.
                if Instant::now().saturating_duration_since(last_rotation_check) >= rotation_check_interval {
                    last_rotation_check = Instant::now();

                    // Refresh the disk-usage health signal on the rotation cadence
                    // (piggybacked; no dedicated thread).
                    sink.set_disk_ok(disk_ok(&base_path, disk_threshold));

                    match &strategy {
                        // Size rotation runs per-batch after flush (see below).
                        RotationStrategy::Never | RotationStrategy::Size(_) => {}
                        RotationStrategy::Daily | RotationStrategy::Hourly => {
                            let new_key = get_rotation_key(&strategy);
                            if new_key != current_key {
                                let _ = sink.file.flush();
                                // Rename current file to date-suffixed archive
                                let archive_path = get_rotated_path(&base_path, &strategy);
                                if current_path.exists() && !archive_path.exists() {
                                    if let Err(e) = fs::rename(&current_path, &archive_path) {
                                        tracing::warn!(
                                            error = %e,
                                            from = %current_path.display(),
                                            to = %archive_path.display(),
                                            "Failed to rename log for rotation, continuing with base path"
                                        );
                                    } else {
                                        tracing::info!(
                                            archived = %archive_path.display(),
                                            "Log file archived on rotation"
                                        );
                                    }
                                }
                                current_key = new_key;
                                current_path = base_path.clone();

                                match open_log_file(&current_path) {
                                    Ok(f) => {
                                        sink.file = BufWriter::new(f);
                                        tracing::info!(path = %current_path.display(), "New log file opened after rotation");
                                        cleanup_old_files(&base_path, max_files);
                                    }
                                    Err(e) => {
                                        tracing::error!(error = %e, path = %current_path.display(), "Failed to open new log file after rotation");
                                    }
                                }
                            }
                        }
                    }
                }

                // Write first line, then drain up to 999 more (1000-line batch).
                sink.write_line(&first_line);
                for _ in 0..999 {
                    match rx.try_recv() {
                        Ok(line) => sink.write_line(&line),
                        Err(_) => break,
                    }
                }

                // Flush once after batch write (drives health from the real
                // flush outcome).
                sink.flush();

                // Size rotation runs per-batch so the active file overshoots
                // max_size by at most one batch.
                if let RotationStrategy::Size(max_size) = &strategy {
                    if sink.current_size >= *max_size {
                        let _ = sink.file.flush();
                        let size_index = find_next_size_index(&base_path);
                        current_path = get_size_rotated_path(&base_path, size_index);
                        match open_log_file(&current_path) {
                            Ok(f) => {
                                sink.file = BufWriter::new(f);
                                sink.current_size = 0;
                                tracing::info!(path = %current_path.display(), index = size_index, "Log file rotated (size)");
                                cleanup_old_files(&base_path, max_files);
                            }
                            Err(e) => {
                                tracing::error!(error = %e, path = %current_path.display(), "Failed to rotate log file");
                            }
                        }
                    }
                }
            }
        });

        self.sender = Some(tx);

        tracing::info!(
            path = %self.full_path().display(),
            rotation = ?self.rotation.strategy,
            max_files = self.rotation.max_files,
            "LocalFileWriter initialized"
        );

        Ok(())
    }

    fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed) && self.sender.is_some()
    }

    async fn send(&self, data: String) -> Result<()> {
        if let Some(sender) = &self.sender {
            // Non-blocking; fire-and-forget (I5).
            match sender.try_send(data) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    // Queue saturation is backpressure (task 06), NOT a health
                    // fault: count the drop but keep the sink healthy.
                    record_drop(self.log_type);
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    // Writer thread has exited (panic / shutdown); it cannot
                    // recover, so report unhealthy for first-healthy fallback.
                    self.healthy.store(false, Ordering::Relaxed);
                    record_drop(self.log_type);
                }
            }
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "local_file"
    }
}

#[cfg(test)]
mod tests {
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
    }

    impl Write for ToggleFailWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.fail.load(Ordering::Relaxed) {
                Err(std::io::Error::other("write failed"))
            } else {
                Ok(buf.len())
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            if self.fail.load(Ordering::Relaxed) {
                Err(std::io::Error::other("flush failed"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn batch_sink_idle_retry_recovers_after_failures_clear() {
        let healthy = Arc::new(AtomicBool::new(true));
        let fail = Arc::new(AtomicBool::new(true));
        let mut sink = BatchSink::new(
            BufWriter::new(ToggleFailWriter {
                fail: Arc::clone(&fail),
            }),
            LogType::Access,
            0,
            Arc::clone(&healthy),
        );

        // Three failed flushes drive the sink unhealthy.
        for _ in 0..3 {
            sink.write_line("line");
            sink.flush();
        }
        assert!(!healthy.load(Ordering::Relaxed), "3 flush failures -> unhealthy");

        // Disk clears; an idle retry (no new write_line, i.e. no traffic)
        // re-attempts the retained bytes and recovers the shed sink.
        fail.store(false, Ordering::Relaxed);
        sink.retry_flush_for_recovery();
        assert!(
            healthy.load(Ordering::Relaxed),
            "idle flush retry recovers a sink that receives no traffic"
        );
        assert_eq!(sink.consecutive_flush_errors, 0);
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
        );
        sink.retry_flush_for_recovery();
        assert!(healthy.load(Ordering::Relaxed));
        assert_eq!(sink.consecutive_flush_errors, 0);
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
        // A full bounded queue is backpressure (task 06), NOT a health fault:
        // the entry is dropped but the sink stays healthy (I5).
        let mut writer = LocalFileWriter::with_path("unused.log");
        // Capacity 1, receiver kept alive but never drained -> next send is Full.
        let (tx, _rx) = std::sync::mpsc::sync_channel::<String>(1);
        tx.try_send("first".to_string()).unwrap();
        writer.sender = Some(tx);
        writer.healthy.store(true, Ordering::Relaxed);

        writer.send("second".to_string()).await.unwrap();

        assert!(writer.healthy(), "a full queue is backpressure, not a health fault");
    }
}
