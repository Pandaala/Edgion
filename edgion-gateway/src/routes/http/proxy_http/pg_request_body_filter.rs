use std::time::Duration;

use super::EdgionHttpProxy;
use crate::ctx::MirrorAbandonReason;
use crate::{EdgionHttpContext, MirrorState};
use bytes::Bytes;
use pingora_proxy::{RequestBodyEvent, Session};
use tokio::sync::mpsc;

/// CRITICAL SAFETY INVARIANT: this function MUST ALWAYS return Ok(()).
/// In pingora H1 mode, returning Err terminates the main request.
/// Mirror is sidecar logic and must NEVER affect the main request path.
#[inline]
pub async fn request_body_filter(
    _edgion_http: &EdgionHttpProxy,
    _session: &mut Session,
    body: &mut Option<Bytes>,
    event: RequestBodyEvent,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Fast path: no mirror active, skip all mirror logic.
    if !ctx.has_mirror_state() {
        return Ok(());
    }

    // All mirror logic is isolated: failures retire the mirror without ever
    // propagating to the caller.
    mirror_body_tee(body, event, ctx).await;

    Ok(()) // ALWAYS Ok — mirror errors never bubble up.
}

/// Copy body chunks to the mirror channel.
///
/// This function never returns an error. Every terminal outcome retires mirror
/// ownership from the request context.
///
/// When `channel_full_timeout_ms > 0` and the channel is full, this function will
/// await channel drain for at most that many milliseconds before abandoning. This
/// adds a bounded latency to the request body filter in exchange for fewer discarded
/// mirror requests when the mirror backend is occasionally slow.
async fn mirror_body_tee(body: &mut Option<Bytes>, event: RequestBodyEvent, ctx: &mut EdgionHttpContext) {
    if event == RequestBodyEvent::Abandoned {
        abandon_request_mirror(ctx, MirrorAbandonReason::BodyAbandoned);
        return;
    }

    // Take ownership to avoid borrow conflicts when mutating ctx.mirror_state below.
    let Some(state) = ctx.take_mirror_state() else {
        return;
    };

    match state {
        MirrorState::Streaming {
            body_tx,
            body_budget,
            writer_handle,
            cancellation,
            channel_full_timeout_ms,
        } => {
            if let Some(data) = body.as_ref() {
                match send_mirror_chunk(&body_tx, &body_budget, data, channel_full_timeout_ms).await {
                    Ok(()) => {}
                    Err(MirrorSendError::Abandon(reason)) => {
                        // Signal cancellation before closing the channel: the delivered
                        // prefix must never become a successful partial upload.
                        cancellation.cancel(reason);
                        drop(body_tx);
                        drop(writer_handle);
                        ctx.retire_request_mirror();
                        return;
                    }
                    Err(MirrorSendError::Closed) => {
                        // Mirror task already exited (timeout, network error, etc.).
                        // Disarm the guard so it cannot overwrite that task's
                        // already-determined outcome with `body_abandoned`.
                        cancellation.complete();
                        drop(body_tx);
                        drop(writer_handle);
                        ctx.retire_request_mirror();
                        return;
                    }
                }
            }

            // A complete terminal event may carry no data — still need
            // to close body_tx so the mirror task's ReceiverStream terminates cleanly.
            if event.is_complete() {
                // Dropping body_tx closes the sender side → ReceiverStream yields None
                // → mirror task finishes sending, emits its log, then exits.
                // Dropping writer_handle detaches (does NOT abort) the task.
                drop(body_tx);
                cancellation.complete();
                drop(writer_handle);
                ctx.retire_request_mirror();
            } else {
                ctx.set_mirror_state(MirrorState::Streaming {
                    body_tx,
                    body_budget,
                    writer_handle,
                    cancellation,
                    channel_full_timeout_ms,
                });
            }
        }
        MirrorState::Abandoned => {
            ctx.retire_request_mirror();
        }
    }
}

enum MirrorSendError {
    Closed,
    Abandon(MirrorAbandonReason),
}

async fn send_mirror_chunk(
    sender: &mpsc::Sender<Result<Bytes, std::io::Error>>,
    budget: &crate::plugins::http::request_mirror::MirrorBodyBudget,
    data: &[u8],
    timeout_ms: u64,
) -> Result<(), MirrorSendError> {
    // Reserve the queue slot first so a full/closed queue never allocates a copy.
    let slot = match sender.try_reserve() {
        Ok(slot) => slot,
        Err(mpsc::error::TrySendError::Closed(_)) => return Err(MirrorSendError::Closed),
        Err(mpsc::error::TrySendError::Full(_)) if timeout_ms > 0 => {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), sender.reserve()).await {
                Ok(Ok(slot)) => slot,
                Ok(Err(_)) => return Err(MirrorSendError::Closed),
                Err(_) => return Err(MirrorSendError::Abandon(MirrorAbandonReason::ChannelFull)),
            }
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            return Err(MirrorSendError::Abandon(MirrorAbandonReason::ChannelFull));
        }
    };
    // Byte-budget admission never waits, even if queue waiting was enabled.
    let owned = budget
        .try_copy(data)
        .ok_or(MirrorSendError::Abandon(MirrorAbandonReason::BufferLimit))?;
    slot.send(Ok(owned));
    Ok(())
}

pub(super) fn abandon_request_mirror(ctx: &mut EdgionHttpContext, reason: MirrorAbandonReason) {
    let Some(state) = ctx.take_mirror_state() else {
        return;
    };
    if let MirrorState::Streaming {
        body_tx,
        writer_handle,
        cancellation,
        ..
    } = state
    {
        cancellation.cancel(reason);
        drop(body_tx);
        drop(writer_handle);
    }
    ctx.retire_request_mirror();
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_request_body_filter_test.rs"]
mod tests;
