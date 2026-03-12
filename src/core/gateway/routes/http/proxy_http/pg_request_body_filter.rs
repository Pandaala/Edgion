use std::sync::atomic::Ordering;
use std::time::Duration;

use super::EdgionHttpProxy;
use crate::types::{EdgionHttpContext, MirrorState};
use bytes::Bytes;
use pingora_proxy::Session;
use tokio::sync::mpsc;

/// CRITICAL SAFETY INVARIANT: this function MUST ALWAYS return Ok(()).
/// In pingora H1 mode, returning Err terminates the main request.
/// Mirror is sidecar logic and must NEVER affect the main request path.
#[inline]
pub async fn request_body_filter(
    _edgion_http: &EdgionHttpProxy,
    _session: &mut Session,
    body: &mut Option<Bytes>,
    end_of_stream: bool,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Fast path: no mirror active, skip all mirror logic.
    if ctx.mirror_state.is_none() {
        return Ok(());
    }

    // All mirror logic is isolated — failures result in MirrorState::Abandoned,
    // never propagated back to the caller.
    mirror_body_tee(body, end_of_stream, ctx).await;

    Ok(()) // ALWAYS Ok — mirror errors never bubble up.
}

/// Copy body chunks to the mirror channel.
///
/// This function never returns an error — all failures result in mirror abandonment
/// via state transition to MirrorState::Abandoned.
///
/// When `channel_full_timeout_ms > 0` and the channel is full, this function will
/// await channel drain for at most that many milliseconds before abandoning. This
/// adds a bounded latency to the request body filter in exchange for fewer discarded
/// mirror requests when the mirror backend is occasionally slow.
async fn mirror_body_tee(body: &mut Option<Bytes>, end_of_stream: bool, ctx: &mut EdgionHttpContext) {
    // Take ownership to avoid borrow conflicts when mutating ctx.mirror_state below.
    let Some(state) = ctx.mirror_state.take() else {
        return;
    };

    match state {
        MirrorState::Streaming {
            body_tx,
            writer_handle,
            channel_full_flag,
            channel_full_timeout_ms,
        } => {
            if let Some(data) = body.as_ref() {
                match body_tx.try_send(Ok(data.clone())) {
                    Ok(()) => {
                        // Chunk sent successfully — fall through to end_of_stream handling.
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // Channel is full. Decide: immediately abandon or briefly wait.
                        let sent = if channel_full_timeout_ms > 0 {
                            // Give the mirror task a short window to drain the channel.
                            // body_tx.send() takes &self, so body_tx remains owned after await.
                            let wait = Duration::from_millis(channel_full_timeout_ms);
                            tokio::time::timeout(wait, body_tx.send(Ok(data.clone())))
                                .await
                                .is_ok_and(|r| r.is_ok())
                        } else {
                            false // Immediate abandon, no waiting.
                        };

                        if !sent {
                            // Mirror is too slow and the wait window (if any) has expired.
                            // Set the shared flag so the mirror task can correctly report
                            // "channel_full" instead of guessing from the error kind.
                            channel_full_flag.store(true, Ordering::Relaxed);
                            tracing::warn!(channel_full_timeout_ms, "Mirror channel full, abandoning mirror",);
                            // Drop body_tx to close the channel → mirror task's ReceiverStream
                            // yields None → reqwest body ends → send() completes (possibly ok).
                            // Drop writer_handle to detach (NOT abort) the task; it will finish
                            // on its own, read channel_full_flag=true, and emit "channel_full" log.
                            drop(writer_handle);
                            ctx.mirror_state = Some(MirrorState::Abandoned);
                            return; // body_tx is dropped here (end of match arm)
                        }

                        // Successfully sent after waiting — continue streaming below.
                        // Fall through to end_of_stream handling.
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        // Mirror task already exited (timeout, network error, etc.).
                        drop(writer_handle);
                        ctx.mirror_state = Some(MirrorState::Abandoned);
                        return;
                    }
                }
            }

            // Handle end of stream (body == None or final chunk with end_of_stream=true).
            // None + end_of_stream=true means a pure EOF signal with no data — still need
            // to close body_tx so the mirror task's ReceiverStream terminates cleanly.
            if end_of_stream {
                // Dropping body_tx closes the sender side → ReceiverStream yields None
                // → mirror task finishes sending, emits its log, then exits.
                // Dropping writer_handle detaches (does NOT abort) the task.
                drop(body_tx);
                drop(writer_handle);
                ctx.mirror_state = Some(MirrorState::Abandoned);
            } else {
                ctx.mirror_state = Some(MirrorState::Streaming {
                    body_tx,
                    writer_handle,
                    channel_full_flag,
                    channel_full_timeout_ms,
                });
            }
        }
        MirrorState::Abandoned => {
            ctx.mirror_state = Some(MirrorState::Abandoned);
        }
    }
}
