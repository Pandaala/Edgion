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

impl LogType {
    /// Increment the canonical dropped-log counter for this log type.
    pub(crate) fn record_drop(self) {
        match self {
            LogType::Access => global_metrics().access_log_dropped(),
            LogType::Ssl => global_metrics().ssl_log_dropped(),
            LogType::Tcp => global_metrics().tcp_log_dropped(),
            LogType::Tls => global_metrics().tls_log_dropped(),
            LogType::Udp => global_metrics().udp_log_dropped(),
        }
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
    /// Bytes buffered in the current batch; promoted on flush Ok and cleared
    /// from accounting on Err even though `BufWriter` may retain unwritten data.
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
    /// Whether the writer is still attached to the canonical active path.
    /// A failed reopen plus failed rollback clears this until a later external-
    /// rename recovery successfully reclaims the base path.
    path_ok: bool,
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
            path_ok: true,
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
                self.log_type.record_drop();
            }
        }
    }

    /// Flush the batch — where disk-full surfaces. Promote pending bytes only
    /// on success, update the flush-error counter, and republish health.
    fn flush(&mut self) -> bool {
        let flushed = match self.file.flush() {
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
                true
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
                    self.log_type.record_drop();
                }
                false
            }
        };
        self.pending_bytes = 0;
        self.pending_lines = 0;
        self.publish_health();
        flushed
    }

    /// Update the disk-probe signal and republish health.
    fn set_disk_ok(&mut self, ok: bool) {
        self.disk_ok = ok;
        self.publish_health();
    }

    fn set_path_ok(&mut self, ok: bool) {
        self.path_ok = ok;
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
        let now = compute_healthy(self.consecutive_flush_errors, self.disk_ok) && self.path_ok;
        if now != self.last_published_healthy {
            if now {
                tracing::warn!("LocalFile log sink recovered to healthy");
            } else {
                tracing::warn!(
                    consecutive_flush_errors = self.consecutive_flush_errors,
                    disk_ok = self.disk_ok,
                    path_ok = self.path_ok,
                    "LocalFile log sink degraded to unhealthy"
                );
            }
            self.healthy.store(now, Ordering::Relaxed);
            self.last_published_healthy = now;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RotationOutcome {
    Rotated,
    ArchiveExists,
    Failed,
}

/// Create an archive without replacing an existing path, then unlink the base
/// name. Both names are in the same directory, so the hard link is atomic and
/// closes the exists-then-rename overwrite race.
fn archive_base_no_replace(base_path: &Path, archive_path: &Path) -> std::io::Result<()> {
    fs::hard_link(base_path, archive_path)?;
    if let Err(e) = fs::remove_file(base_path) {
        let _ = fs::remove_file(archive_path);
        return Err(e);
    }
    Ok(())
}

fn restore_archived_base(archive_path: &Path, base_path: &Path) -> std::io::Result<()> {
    fs::hard_link(archive_path, base_path)?;
    if let Err(e) = fs::remove_file(archive_path) {
        tracing::warn!(
            error = %e,
            path = %archive_path.display(),
            "Active log path was restored but the rotation archive could not be removed"
        );
    }
    Ok(())
}

/// Replace an externally renamed writer only after every retained byte has
/// flushed successfully. `wrap` keeps the gate testable with a failing writer.
fn reopen_sink_if_renamed<W, F>(sink: &mut BatchSink<W>, base_path: &Path, wrap: F) -> bool
where
    W: Write,
    F: FnOnce(fs::File) -> W,
{
    reopen_sink_if_renamed_with(sink, base_path, open_log_file, wrap)
}

fn reopen_sink_if_renamed_with<W, F, O>(sink: &mut BatchSink<W>, base_path: &Path, open: O, wrap: F) -> bool
where
    W: Write,
    F: FnOnce(fs::File) -> W,
    O: FnOnce(&Path) -> std::io::Result<fs::File>,
{
    // `path_ok=false` means the current writer is known to target a renamed
    // inode. An externally recreated base path does not repair that attachment;
    // it must still be opened and installed below.
    if sink.path_ok && base_path.exists() {
        return false;
    }

    if !sink.flush() {
        sink.set_path_ok(false);
        return false;
    }

    let file = match open(base_path) {
        Ok(file) => file,
        Err(e) => {
            sink.set_path_ok(false);
            tracing::error!(
                error = %e,
                path = %base_path.display(),
                "Failed to reopen canonical active log file"
            );
            return false;
        }
    };
    let current_size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    sink.file = BufWriter::new(wrap(file));
    sink.current_size = current_size;
    sink.set_path_ok(true);
    true
}

/// Archive the active base file without overwrite and reopen the base path.
/// The old writer is replaced only after its retained buffer is confirmed on
/// disk. A reopen failure restores the original base link; if that restoration
/// also fails, the still-open archive writer is marked unhealthy until the base
/// path can be reclaimed.
fn rotate_active_file<W, F>(
    sink: &mut BatchSink<W>,
    base_path: &Path,
    archive_path: &Path,
    max_files: usize,
    log_archive_failure: bool,
    wrap: F,
) -> RotationOutcome
where
    W: Write,
    F: FnOnce(fs::File) -> W,
{
    if !sink.flush() {
        return RotationOutcome::Failed;
    }

    match archive_base_no_replace(base_path, archive_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return RotationOutcome::ArchiveExists;
        }
        Err(e) => {
            if log_archive_failure {
                tracing::warn!(
                    error = %e,
                    from = %base_path.display(),
                    to = %archive_path.display(),
                    "Failed to archive log for rotation, continuing with base path"
                );
            }
            return RotationOutcome::Failed;
        }
    }

    let file = match open_log_file(base_path) {
        Ok(file) => file,
        Err(e) => {
            if let Err(rollback_error) = restore_archived_base(archive_path, base_path) {
                sink.set_path_ok(false);
                tracing::error!(
                    error = %e,
                    rollback_error = %rollback_error,
                    archived = %archive_path.display(),
                    active = %base_path.display(),
                    "Failed to reopen active log file after rotation and restore its path"
                );
            } else {
                tracing::error!(
                    error = %e,
                    path = %base_path.display(),
                    "Failed to reopen active log file after rotation; archive was restored"
                );
            }
            return RotationOutcome::Failed;
        }
    };

    sink.file = BufWriter::new(wrap(file));
    sink.current_size = 0;
    sink.set_path_ok(true);
    cleanup_old_files(base_path, max_files);
    RotationOutcome::Rotated
}

fn rotate_for_size<W, F>(
    sink: &mut BatchSink<W>,
    base_path: &Path,
    max_files: usize,
    log_failure: bool,
    wrap: F,
) -> bool
where
    W: Write,
    F: Fn(fs::File) -> W,
{
    for _ in 0..3 {
        let size_index = find_next_size_index(base_path);
        let archive_path = get_size_rotated_path(base_path, size_index);
        match rotate_active_file(sink, base_path, &archive_path, max_files, log_failure, &wrap) {
            RotationOutcome::Rotated => {
                tracing::info!(
                    path = %base_path.display(),
                    archived = %archive_path.display(),
                    index = size_index,
                    "Log file rotated (size)"
                );
                return true;
            }
            RotationOutcome::ArchiveExists => continue,
            RotationOutcome::Failed => return false,
        }
    }
    if log_failure {
        tracing::warn!(path = %base_path.display(), "Size rotation archive index raced repeatedly");
    }
    false
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

        // Warn on an unbounded configuration: Never rotation with the disk-usage
        // guard disabled lets the file grow without limit during a sustained
        // outage (the access-log fallback is the prime example). Bound it with a
        // size/time rotation + maxFiles, or a diskUsageThresholdPercent.
        if matches!(rotation.strategy, RotationStrategy::Never) && disk_threshold.is_none_or(|t| t == 0) {
            tracing::warn!(
                path = %base_path.display(),
                ?log_type,
                "LocalFile sink has rotation=Never and no disk-usage threshold; the file can grow unbounded. Set rotation (size/daily/hourly) + maxFiles, or diskUsageThresholdPercent."
            );
        }

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
            let mut size_rotation_failure_logged = false;
            let rotation_check_interval = Duration::from_secs(rotation.check_interval_secs);
            let mut last_rotation_check = Instant::now();

            // Active file always uses the base path (nginx-style).
            let current_path = base_path.clone();
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
                    // Count any entries already queued before the open failed so
                    // the loss is observable.
                    while rx.try_recv().is_ok() {
                        log_type.record_drop();
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
                        // No traffic this interval: drive recovery without it, and
                        // reclaim the active file if it was renamed away (e.g. by a
                        // backfill) so a future degrade writes to a fresh file.
                        if reopen_sink_if_renamed(&mut sink, &base_path, |file| file) {
                            tracing::info!(path = %base_path.display(), "Active log file reopened after external rename");
                        }
                        sink.set_disk_ok(disk_ok(&base_path, disk_threshold));
                        sink.retry_flush_for_recovery();
                        continue;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };

                // Before writing this batch, reclaim the active file if it was
                // renamed out from under us (backfill / logrotate `create`). The
                // previous batch was already flushed, so the buffer is empty and
                // nothing is lost; the first post-rename entry lands in a fresh file.
                if reopen_sink_if_renamed(&mut sink, &base_path, |file| file) {
                    tracing::info!(path = %base_path.display(), "Active log file reopened after external rename");
                }
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
                                let archive_path = get_rotated_path(&base_path, &strategy);
                                match rotate_active_file(
                                    &mut sink,
                                    &base_path,
                                    &archive_path,
                                    max_files,
                                    true,
                                    |file| file,
                                ) {
                                    RotationOutcome::Rotated => {
                                        tracing::info!(
                                            archived = %archive_path.display(),
                                            "Log file archived on rotation"
                                        );
                                        tracing::info!(path = %base_path.display(), "New log file opened after rotation");
                                        current_key = new_key;
                                    }
                                    RotationOutcome::ArchiveExists => {
                                        // A prior process already archived this
                                        // period. Keep the current base active
                                        // and avoid retry/log churn until the
                                        // next wall-clock key.
                                        current_key = new_key;
                                    }
                                    RotationOutcome::Failed => {}
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
                let flushed = sink.flush();

                // Size rotation runs per-batch so the active file overshoots
                // max_size by at most one batch.
                if let RotationStrategy::Size(max_size) = &strategy {
                    if flushed && sink.current_size >= *max_size {
                        let rotated = rotate_for_size(
                            &mut sink,
                            &base_path,
                            max_files,
                            !size_rotation_failure_logged,
                            |file| file,
                        );
                        size_rotation_failure_logged = !rotated;
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
            // Non-blocking; fire-and-forget.
            match sender.try_send(data) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    // Queue saturation is backpressure, NOT a health fault:
                    // count the drop but keep the sink healthy.
                    self.log_type.record_drop();
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    // Writer thread has exited (panic / shutdown); it cannot
                    // recover, so report unhealthy for first-healthy fallback.
                    self.healthy.store(false, Ordering::Relaxed);
                    self.log_type.record_drop();
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
#[path = "../../../../tests/unit/link_sys/providers/local_file/data_sender_impl_test.rs"]
mod tests;
