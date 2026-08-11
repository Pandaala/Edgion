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
    if ctx.mirror_state.is_none() {
        return Ok(());
    }

    // All mirror logic is isolated — failures result in MirrorState::Abandoned,
    // never propagated back to the caller.
    mirror_body_tee(body, event, ctx).await;

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
async fn mirror_body_tee(body: &mut Option<Bytes>, event: RequestBodyEvent, ctx: &mut EdgionHttpContext) {
    if event == RequestBodyEvent::Abandoned {
        abandon_request_mirror(ctx, MirrorAbandonReason::BodyAbandoned);
        return;
    }

    // Take ownership to avoid borrow conflicts when mutating ctx.mirror_state below.
    let Some(state) = ctx.mirror_state.take() else {
        return;
    };

    match state {
        MirrorState::Streaming {
            body_tx,
            writer_handle,
            cancellation,
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
                            // Cancel before dropping body_tx so the mirror backend cannot
                            // mistake the delivered prefix for a complete chunked body.
                            cancellation.cancel(MirrorAbandonReason::ChannelFull);
                            // Drop writer_handle to detach (NOT abort) the task; it will finish
                            // on its own and emit the "channel_full" outcome.
                            drop(writer_handle);
                            ctx.mirror_state = Some(MirrorState::Abandoned);
                            return; // body_tx is dropped here (end of match arm)
                        }

                        // Successfully sent after waiting — continue streaming below.
                        // Fall through to end_of_stream handling.
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        // Mirror task already exited (timeout, network error, etc.).
                        // Disarm the guard so it cannot overwrite that task's
                        // already-determined outcome with `body_abandoned`.
                        cancellation.complete();
                        drop(writer_handle);
                        ctx.mirror_state = Some(MirrorState::Abandoned);
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
                ctx.mirror_state = Some(MirrorState::Abandoned);
            } else {
                ctx.mirror_state = Some(MirrorState::Streaming {
                    body_tx,
                    writer_handle,
                    cancellation,
                    channel_full_timeout_ms,
                });
            }
        }
        MirrorState::Abandoned => {
            ctx.mirror_state = Some(MirrorState::Abandoned);
        }
    }
}

pub(super) fn abandon_request_mirror(ctx: &mut EdgionHttpContext, reason: MirrorAbandonReason) {
    let Some(state) = ctx.mirror_state.take() else {
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
    ctx.mirror_state = Some(MirrorState::Abandoned);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::MirrorCancellation;
    use std::sync::atomic::AtomicU8;
    use std::sync::Arc;
    use tokio::sync::oneshot;

    /// Note: the body receiver is intentionally dropped here, so the returned
    /// state's `body_tx` is already closed. Tests that pass `body = None` never
    /// touch it; a test that sends real data exercises `TrySendError::Closed`.
    fn streaming_state() -> (MirrorState, oneshot::Receiver<()>, Arc<AtomicU8>) {
        let (body_tx, _body_rx) = mpsc::channel(1);
        let (cancellation, cancel_rx) = MirrorCancellation::new();
        let reason = cancellation.reason_handle();
        let writer_handle = tokio::spawn(async {});
        (
            MirrorState::Streaming {
                body_tx,
                writer_handle,
                cancellation,
                channel_full_timeout_ms: 0,
            },
            cancel_rx,
            reason,
        )
    }

    #[tokio::test]
    async fn abandoned_body_cancels_mirror_request() {
        let (state, cancel_rx, _reason) = streaming_state();
        let mut ctx = EdgionHttpContext {
            mirror_state: Some(state),
            ..Default::default()
        };
        let mut body = None;

        mirror_body_tee(&mut body, RequestBodyEvent::Abandoned, &mut ctx).await;

        assert!(matches!(ctx.mirror_state, Some(MirrorState::Abandoned)));
        assert!(cancel_rx.await.is_ok(), "abandonment must cancel the HTTP send future");
    }

    #[tokio::test]
    async fn complete_body_closes_mirror_without_cancelling() {
        let (state, cancel_rx, _reason) = streaming_state();
        let mut ctx = EdgionHttpContext {
            mirror_state: Some(state),
            ..Default::default()
        };
        let mut body = None;

        mirror_body_tee(&mut body, RequestBodyEvent::Complete, &mut ctx).await;

        assert!(matches!(ctx.mirror_state, Some(MirrorState::Abandoned)));
        assert!(
            cancel_rx.await.is_err(),
            "a complete body must close the stream normally, not cancel it"
        );
    }

    #[tokio::test]
    async fn dropping_streaming_state_cancels_by_default() {
        let (state, cancel_rx, _reason) = streaming_state();

        drop(state);

        assert!(
            cancel_rx.await.is_ok(),
            "implicit state destruction must cancel before treating the body as complete"
        );
    }

    /// A closed body channel means the writer task already exited and already
    /// classified its own failure (timeout / connect_err / write_err). That
    /// outcome must survive, so this arm DISARMS the guard instead of firing it.
    #[tokio::test]
    async fn closed_body_channel_disarms_guard_instead_of_overwriting_writer_outcome() {
        // The helper's body receiver is already dropped, so `try_send` below
        // takes the `TrySendError::Closed` arm.
        let (state, cancel_rx, reason) = streaming_state();
        let mut ctx = EdgionHttpContext {
            mirror_state: Some(state),
            ..Default::default()
        };
        let mut body = Some(Bytes::from_static(b"partial chunk"));

        mirror_body_tee(&mut body, RequestBodyEvent::Data, &mut ctx).await;

        assert!(matches!(ctx.mirror_state, Some(MirrorState::Abandoned)));
        assert!(
            cancel_rx.await.is_err(),
            "a closed body channel must disarm the guard (sender dropped, no signal), not cancel it: \
             turning this arm into cancel(BodyAbandoned) would mask the writer task's real failure \
             classification in edgion_mirror_requests_total"
        );
        assert!(
            MirrorCancellation::load_reason(&reason).is_none(),
            "disarming must leave the abandon reason unset so the writer task's already-determined \
             outcome is not overwritten with body_abandoned"
        );
    }

    #[test]
    fn abandoning_without_a_mirror_preserves_absence() {
        let mut ctx = EdgionHttpContext::default();

        abandon_request_mirror(&mut ctx, MirrorAbandonReason::BodyAbandoned);

        assert!(
            ctx.mirror_state.is_none(),
            "a request with no mirror must not acquire an abandoned mirror sentinel"
        );
    }
}
