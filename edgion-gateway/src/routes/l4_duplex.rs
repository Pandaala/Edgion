//! Shared bidirectional relay loop for L4 (TCP/TLS) proxies.
//!
//! TCP, TLS-terminated, and TLS-passthrough proxies all need the same
//! `select!`-driven duplex relay between a downstream and an upstream
//! [`Stream`]. The skeleton — buffer sizes, EOF handling, write-then-flush
//! ordering on the client-facing direction — is identical; only how each
//! caller records byte counters and surfaces errors differs. This module
//! captures the skeleton; callers provide a [`DuplexHooks`] impl for the
//! per-context bits.
//!
//! Behavior contract (callers can rely on it):
//!
//! - Buffers: two independently-owned 8 KiB heap buffers, one per direction.
//! - On either side reading `Ok(0)` (clean EOF) the loop breaks normally
//!   without invoking [`DuplexHooks::on_error`].
//! - On any I/O error (read, write, flush) the loop calls
//!   [`DuplexHooks::on_error`] with the matching [`DuplexError`] variant and
//!   then breaks.
//! - The client-facing (downstream) side is explicitly flushed after every
//!   upstream-to-client write — listener streams may buffer otherwise. The
//!   upstream side is *not* flushed (matches prior behavior across all three
//!   call sites).
//! - Byte counters are recorded *before* the corresponding write, so they
//!   reflect bytes successfully read from the source, regardless of whether
//!   the subsequent write succeeds. This matches prior behavior.
//! - The relay is bounded by a single idle clock (when `idle_timeout` is
//!   `Some`): it resets on any byte progress in either direction (a completed
//!   read or an advancing partial write, including inside `write_all_idle` /
//!   `flush_idle`). If no progress occurs within the window the loop calls
//!   [`DuplexHooks::on_error`] with [`DuplexError::IdleTimeout`] and breaks —
//!   an idle-timeout closure also routes through `on_error`, like an I/O error.
//! - `idle_timeout == None` leaves every await unbounded (the explicit opt-out).

use pingora_core::protocols::Stream;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::select;
use tokio::time::{sleep, Instant, Sleep};

const BUFFER_SIZE: usize = 8192;

/// Which step in the relay loop failed. Callers map this to their own
/// error-reporting model (typed status enum, free-form log string, ...).
#[derive(Debug, Clone, Copy)]
pub(crate) enum DuplexError {
    /// `upstream.write_all(...)` failed while forwarding client→upstream.
    UpstreamWrite,
    /// `downstream.read(...)` failed.
    DownstreamRead,
    /// `downstream.write_all(...)` failed while forwarding upstream→client.
    DownstreamWrite,
    /// `downstream.flush()` failed after an upstream→client write. Callers
    /// that do not distinguish flush from write should map this to the same
    /// status as [`DuplexError::DownstreamWrite`].
    DownstreamFlush,
    /// `upstream.read(...)` failed.
    UpstreamRead,
    /// The relay idle deadline elapsed with no byte progress in either
    /// direction (a completed read or an advancing partial write) within the
    /// configured idle window. A deliberate closure, not an I/O error.
    IdleTimeout,
}

/// Per-call-site behavior plugged into [`bidirectional_relay`].
///
/// Implementations typically hold a `&mut SomeContext` and translate the
/// generic hook calls into mutations on that context (incrementing byte
/// counters, setting a status enum or an error log string).
pub(crate) trait DuplexHooks {
    /// Called after each successful client→upstream read, with the byte
    /// count just read. Invoked *before* the matching upstream write is
    /// attempted.
    fn record_sent(&mut self, n: usize);

    /// Called after each successful upstream→client read, with the byte
    /// count just read. Invoked *before* the matching downstream write is
    /// attempted.
    fn record_received(&mut self, n: usize);

    /// Called once when the loop is about to break due to an I/O error.
    fn on_error(&mut self, err: DuplexError);
}

/// Outcome of an idle-bounded write or flush.
enum WriteOutcome {
    /// The whole buffer was written / the flush completed.
    Done,
    /// A read/write/flush I/O error occurred (including a `write` returning 0).
    Io,
    /// The idle deadline elapsed before completion.
    IdleExpired,
}

/// Reset the shared idle deadline to `now + dur` when bounded; no-op when the
/// timeout is disabled (`None`).
#[inline]
fn reset_idle(idle: Pin<&mut Sleep>, idle_timeout: Option<Duration>) {
    if let Some(dur) = idle_timeout {
        idle.reset(Instant::now() + dur);
    }
}

/// Write the whole buffer, resetting the shared idle clock on every byte of
/// progress so a slow-but-draining peer is not severed. Returns `IdleExpired`
/// if the idle deadline fires before the write completes. When `idle_timeout`
/// is `None` the write is unbounded (the explicit opt-out path).
///
/// Uses single `write` calls (not `write_all`) because `write`'s future is
/// cancel-safe: if the idle arm fires before the write future resolves, the
/// future is dropped having written zero bytes (the Tokio `AsyncWrite`
/// contract), so `written` stays accurate and no partial write is lost.
async fn write_all_idle<W>(
    stream: &mut W,
    buf: &[u8],
    mut idle: Pin<&mut Sleep>,
    idle_timeout: Option<Duration>,
) -> WriteOutcome
where
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut written = 0;
    while written < buf.len() {
        if let Some(dur) = idle_timeout {
            select! {
                w = stream.write(&buf[written..]) => match w {
                    Ok(0) => return WriteOutcome::Io,
                    Ok(n) => {
                        written += n;
                        idle.as_mut().reset(Instant::now() + dur);
                    }
                    Err(_) => return WriteOutcome::Io,
                },
                _ = idle.as_mut() => return WriteOutcome::IdleExpired,
            }
        } else {
            match stream.write(&buf[written..]).await {
                Ok(0) => return WriteOutcome::Io,
                Ok(n) => written += n,
                Err(_) => return WriteOutcome::Io,
            }
        }
    }
    WriteOutcome::Done
}

/// Flush the client-facing stream, bounded by the shared idle clock. `flush`
/// has no partial-progress signal, so it must complete within one idle window
/// of the preceding write progress. Unbounded when `idle_timeout` is `None`.
async fn flush_idle<W>(stream: &mut W, mut idle: Pin<&mut Sleep>, idle_timeout: Option<Duration>) -> WriteOutcome
where
    W: AsyncWrite + Unpin + ?Sized,
{
    if idle_timeout.is_some() {
        select! {
            f = stream.flush() => match f {
                Ok(()) => WriteOutcome::Done,
                Err(_) => WriteOutcome::Io,
            },
            _ = idle.as_mut() => WriteOutcome::IdleExpired,
        }
    } else {
        match stream.flush().await {
            Ok(()) => WriteOutcome::Done,
            Err(_) => WriteOutcome::Io,
        }
    }
}

/// Run the shared bidirectional relay loop until either side closes cleanly
/// or an I/O error occurs.
///
/// Consumes both streams. Returns when:
/// - Either side returns `Ok(0)` from `read` (clean EOF), or
/// - Any read/write/flush errors, after notifying [`DuplexHooks::on_error`], or
/// - The idle deadline fires (when `idle_timeout` is `Some`).
pub(crate) async fn bidirectional_relay(
    mut downstream: Stream,
    mut upstream: Stream,
    mut hooks: impl DuplexHooks,
    idle_timeout: Option<Duration>,
) {
    let mut client_to_upstream_buf = vec![0u8; BUFFER_SIZE];
    let mut upstream_to_client_buf = vec![0u8; BUFFER_SIZE];

    // One shared idle clock for both the read select and the write helpers.
    // When unbounded it is constructed (a select! arm needs a concrete future)
    // but disabled via the `if idle_timeout.is_some()` precondition guard.
    let idle = sleep(idle_timeout.unwrap_or(Duration::from_secs(0)));
    tokio::pin!(idle);

    loop {
        select! {
            // Client -> Upstream
            result = downstream.read(&mut client_to_upstream_buf) => {
                match result {
                    Ok(0) => break,
                    Ok(n) => {
                        reset_idle(idle.as_mut(), idle_timeout); // read = progress
                        hooks.record_sent(n);
                        match write_all_idle(&mut upstream, &client_to_upstream_buf[..n], idle.as_mut(), idle_timeout).await {
                            WriteOutcome::Done => {}
                            WriteOutcome::Io => { hooks.on_error(DuplexError::UpstreamWrite); break; }
                            WriteOutcome::IdleExpired => { hooks.on_error(DuplexError::IdleTimeout); break; }
                        }
                    }
                    Err(_) => {
                        hooks.on_error(DuplexError::DownstreamRead);
                        break;
                    }
                }
            }
            // Upstream -> Client
            result = upstream.read(&mut upstream_to_client_buf) => {
                match result {
                    Ok(0) => break,
                    Ok(n) => {
                        reset_idle(idle.as_mut(), idle_timeout);
                        hooks.record_received(n);
                        match write_all_idle(&mut downstream, &upstream_to_client_buf[..n], idle.as_mut(), idle_timeout).await {
                            WriteOutcome::Done => {}
                            WriteOutcome::Io => { hooks.on_error(DuplexError::DownstreamWrite); break; }
                            WriteOutcome::IdleExpired => { hooks.on_error(DuplexError::IdleTimeout); break; }
                        }
                        // Listener streams buffer writes; flush client-facing bytes promptly.
                        match flush_idle(&mut downstream, idle.as_mut(), idle_timeout).await {
                            WriteOutcome::Done => {}
                            WriteOutcome::Io => { hooks.on_error(DuplexError::DownstreamFlush); break; }
                            WriteOutcome::IdleExpired => { hooks.on_error(DuplexError::IdleTimeout); break; }
                        }
                    }
                    Err(_) => {
                        hooks.on_error(DuplexError::UpstreamRead);
                        break;
                    }
                }
            }
            // Idle deadline (disabled when unbounded). Non-biased select is fine:
            // an idle connection has only this arm ready, so it always fires.
            _ = idle.as_mut(), if idle_timeout.is_some() => {
                hooks.on_error(DuplexError::IdleTimeout);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::time::{sleep as tokio_sleep, Instant};

    fn pinned_idle(dur: Duration) -> std::pin::Pin<Box<tokio::time::Sleep>> {
        Box::pin(tokio::time::sleep(dur))
    }

    #[tokio::test]
    async fn write_all_idle_completes_when_reader_drains() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let idle_dur = Duration::from_millis(200);
        let reader = tokio::spawn(async move {
            let mut buf = vec![0u8; 16];
            let mut total = 0;
            while total < 16 {
                let n = b.read(&mut buf[total..]).await.unwrap();
                if n == 0 {
                    break;
                }
                total += n;
            }
            total
        });
        let mut idle = pinned_idle(idle_dur);
        let out = write_all_idle(&mut a, &[7u8; 16], idle.as_mut(), Some(idle_dur)).await;
        assert!(matches!(out, WriteOutcome::Done));
        drop(a);
        assert_eq!(reader.await.unwrap(), 16);
    }

    #[tokio::test]
    async fn write_all_idle_expires_when_peer_never_reads() {
        let (mut a, _b) = tokio::io::duplex(8);
        let idle_dur = Duration::from_millis(150);
        let mut idle = pinned_idle(idle_dur);
        let started = Instant::now();
        let out = write_all_idle(&mut a, &[1u8; 4096], idle.as_mut(), Some(idle_dur)).await;
        assert!(matches!(out, WriteOutcome::IdleExpired));
        assert!(started.elapsed() >= idle_dur, "should not fire before the window");
    }

    #[tokio::test]
    async fn write_all_idle_survives_slow_but_progressing_peer() {
        let (mut a, mut b) = tokio::io::duplex(8);
        let idle_dur = Duration::from_millis(200);
        let reader = tokio::spawn(async move {
            let mut buf = vec![0u8; 8];
            let mut total = 0;
            loop {
                let n = b.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                total += n;
                tokio_sleep(Duration::from_millis(80)).await;
            }
            total
        });
        let mut idle = pinned_idle(idle_dur);
        let out = write_all_idle(&mut a, &[9u8; 40], idle.as_mut(), Some(idle_dur)).await;
        assert!(matches!(out, WriteOutcome::Done));
        drop(a);
        assert_eq!(reader.await.unwrap(), 40);
    }
}
