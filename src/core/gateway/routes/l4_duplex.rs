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

use pingora_core::protocols::Stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::select;

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

/// Run the shared bidirectional relay loop until either side closes cleanly
/// or an I/O error occurs.
///
/// Consumes both streams. Returns when:
/// - Either side returns `Ok(0)` from `read` (clean EOF), or
/// - Any read/write/flush errors, after notifying [`DuplexHooks::on_error`].
pub(crate) async fn bidirectional_relay(mut downstream: Stream, mut upstream: Stream, mut hooks: impl DuplexHooks) {
    let mut client_to_upstream_buf = vec![0u8; BUFFER_SIZE];
    let mut upstream_to_client_buf = vec![0u8; BUFFER_SIZE];

    loop {
        select! {
            // Client → Upstream
            result = downstream.read(&mut client_to_upstream_buf) => {
                match result {
                    Ok(0) => break,
                    Ok(n) => {
                        hooks.record_sent(n);
                        if (upstream.write_all(&client_to_upstream_buf[..n]).await).is_err() {
                            hooks.on_error(DuplexError::UpstreamWrite);
                            break;
                        }
                    }
                    Err(_) => {
                        hooks.on_error(DuplexError::DownstreamRead);
                        break;
                    }
                }
            }
            // Upstream → Client
            result = upstream.read(&mut upstream_to_client_buf) => {
                match result {
                    Ok(0) => break,
                    Ok(n) => {
                        hooks.record_received(n);
                        if (downstream.write_all(&upstream_to_client_buf[..n]).await).is_err() {
                            hooks.on_error(DuplexError::DownstreamWrite);
                            break;
                        }
                        // Listener streams buffer writes; flush client-facing bytes promptly.
                        if (downstream.flush().await).is_err() {
                            hooks.on_error(DuplexError::DownstreamFlush);
                            break;
                        }
                    }
                    Err(_) => {
                        hooks.on_error(DuplexError::UpstreamRead);
                        break;
                    }
                }
            }
        }
    }
}
