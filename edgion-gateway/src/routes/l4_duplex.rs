//! Shared bidirectional relay loop for L4 (TCP/TLS) proxies.
//!
//! TCP, TLS-terminated, and TLS-passthrough proxies all need the same
//! independently polled duplex relay between a downstream and an upstream
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
//!   read or an advancing partial write). A blocked write or flush does not
//!   prevent the other direction from progressing. If no progress occurs within the window the loop calls
//!   [`DuplexHooks::on_error`] with [`DuplexError::IdleTimeout`] and breaks —
//!   an idle-timeout closure also routes through `on_error`, like an I/O error.
//! - `idle_timeout == None` leaves every await unbounded (the explicit opt-out).

use pingora_core::protocols::Stream;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{sleep, Instant, Sleep};

const BUFFER_SIZE: usize = 8192;

/// Which step in the relay loop failed. Callers map this to their own
/// error-reporting model (typed status enum, free-form log string, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DuplexError {
    /// Writing upstream failed while forwarding client→upstream.
    UpstreamWrite,
    /// `downstream.read(...)` failed.
    DownstreamRead,
    /// Writing downstream failed while forwarding upstream→client.
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

    /// Called once when the loop terminates due to an I/O error or idle timeout.
    fn on_error(&mut self, err: DuplexError);
}

enum RelayStep {
    Pending,
    Read(usize),
    Written,
    Flushed,
    Eof,
}

/// One buffer and its retained write cursor. A pending operation never resets
/// the cursor, so polling the reverse direction cannot replay partial writes.
struct RelayDirection {
    buf: Vec<u8>,
    len: usize,
    written: usize,
    flushing: bool,
}

impl RelayDirection {
    fn new() -> Self {
        Self {
            buf: vec![0; BUFFER_SIZE],
            len: 0,
            written: 0,
            flushing: false,
        }
    }

    /// Perform at most one I/O poll; the owner gives both directions a turn.
    fn poll_step(
        &mut self,
        cx: &mut Context<'_>,
        reader: &mut Stream,
        writer: &mut Stream,
        to_upstream: bool,
    ) -> Result<RelayStep, DuplexError> {
        if self.flushing {
            return match Pin::new(writer).poll_flush(cx) {
                Poll::Pending => Ok(RelayStep::Pending),
                Poll::Ready(Err(_)) => Err(DuplexError::DownstreamFlush),
                Poll::Ready(Ok(())) => {
                    self.flushing = false;
                    Ok(RelayStep::Flushed)
                }
            };
        }
        if self.written < self.len {
            return match Pin::new(writer).poll_write(cx, &self.buf[self.written..self.len]) {
                Poll::Pending => Ok(RelayStep::Pending),
                Poll::Ready(Err(_)) | Poll::Ready(Ok(0)) => Err(if to_upstream {
                    DuplexError::UpstreamWrite
                } else {
                    DuplexError::DownstreamWrite
                }),
                Poll::Ready(Ok(n)) => {
                    self.written += n;
                    if self.written == self.len {
                        self.len = 0;
                        self.written = 0;
                        self.flushing = !to_upstream;
                    }
                    Ok(RelayStep::Written)
                }
            };
        }
        let mut buf = ReadBuf::new(&mut self.buf);
        match Pin::new(reader).poll_read(cx, &mut buf) {
            Poll::Pending => Ok(RelayStep::Pending),
            Poll::Ready(Err(_)) => Err(if to_upstream {
                DuplexError::DownstreamRead
            } else {
                DuplexError::UpstreamRead
            }),
            Poll::Ready(Ok(())) => {
                self.len = buf.filled().len();
                if self.len == 0 {
                    Ok(RelayStep::Eof)
                } else {
                    Ok(RelayStep::Read(self.len))
                }
            }
        }
    }
}

/// Reset the shared idle deadline to `now + dur` when bounded; no-op when the
/// timeout is disabled (`None`).
#[inline]
fn reset_idle(idle: Pin<&mut Sleep>, idle_timeout: Option<Duration>) {
    if let Some(dur) = idle_timeout {
        idle.reset(Instant::now() + dur);
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
    let mut upload = RelayDirection::new();
    let mut download = RelayDirection::new();
    let mut upload_first = true;
    // A single timer follows byte progress in either direction. When unbounded
    // it is never polled or reset.
    let idle = sleep(idle_timeout.unwrap_or(Duration::from_secs(0)));
    tokio::pin!(idle);

    let error = poll_fn(|cx| {
        // Bound ready-I/O work even for streams without Tokio's cooperative
        // budget. Every sweep polls both directions, alternating the first.
        for _ in 0..32 {
            let mut progressed = false;
            for to_upstream in [upload_first, !upload_first] {
                let step = if to_upstream {
                    upload.poll_step(cx, &mut downstream, &mut upstream, true)
                } else {
                    download.poll_step(cx, &mut upstream, &mut downstream, false)
                };
                match step {
                    Err(err) => return Poll::Ready(Some(err)),
                    Ok(RelayStep::Eof) => return Poll::Ready(None),
                    Ok(RelayStep::Pending) => {}
                    Ok(RelayStep::Read(n)) => {
                        if to_upstream {
                            hooks.record_sent(n);
                        } else {
                            hooks.record_received(n);
                        }
                        reset_idle(idle.as_mut(), idle_timeout);
                        progressed = true;
                    }
                    Ok(RelayStep::Written) => {
                        reset_idle(idle.as_mut(), idle_timeout);
                        progressed = true;
                    }
                    Ok(RelayStep::Flushed) => progressed = true,
                }
            }
            upload_first = !upload_first;
            if idle_timeout.is_some() && idle.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Some(DuplexError::IdleTimeout));
            }
            if !progressed {
                return Poll::Pending;
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
    if let Some(err) = error {
        hooks.on_error(err);
    }
}

#[cfg(test)]
#[path = "../../tests/unit/routes/l4_duplex_test.rs"]
mod tests;
