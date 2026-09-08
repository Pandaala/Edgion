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
                body_budget: crate::plugins::http::request_mirror::MirrorBodyBudget::for_test(1024),
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
        let mut ctx = EdgionHttpContext::default();
        ctx.set_mirror_state(state);
        assert!(ctx.has_request_body_plugins());
        assert!(
            !ctx.request_body_stream_active(),
            "mirror observation does not consume replayability"
        );
        let mut body = None;

        mirror_body_tee(&mut body, RequestBodyEvent::Abandoned, &mut ctx).await;

        assert!(!ctx.has_mirror_state());
        assert!(!ctx.has_request_body_plugins());
        assert!(cancel_rx.await.is_ok(), "abandonment must cancel the HTTP send future");
    }

    #[tokio::test]
    async fn complete_body_closes_mirror_without_cancelling() {
        let (state, cancel_rx, _reason) = streaming_state();
        let mut ctx = EdgionHttpContext::default();
        ctx.set_mirror_state(state);
        let mut body = None;

        mirror_body_tee(&mut body, RequestBodyEvent::Complete, &mut ctx).await;

        assert!(!ctx.has_mirror_state());
        assert!(!ctx.has_request_body_plugins());
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
        let mut ctx = EdgionHttpContext::default();
        ctx.set_mirror_state(state);
        let mut body = Some(Bytes::from_static(b"partial chunk"));

        mirror_body_tee(&mut body, RequestBodyEvent::Data, &mut ctx).await;

        assert!(!ctx.has_mirror_state());
        assert!(!ctx.has_request_body_plugins());
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
            !ctx.has_mirror_state(),
            "a request with no mirror must not acquire an abandoned mirror sentinel"
        );
    }

    #[tokio::test]
    async fn defensive_abandoned_state_is_retired() {
        let mut ctx = EdgionHttpContext::default();
        ctx.set_mirror_state(MirrorState::Abandoned);
        let mut body = Some(Bytes::from_static(b"main"));

        mirror_body_tee(&mut body, RequestBodyEvent::Data, &mut ctx).await;

        assert_eq!(body.as_deref(), Some(&b"main"[..]));
        assert!(!ctx.has_mirror_state());
        assert!(!ctx.has_request_body_plugins());
    }

    #[tokio::test]
    async fn channel_full_retires_mirror_and_preserves_main_chunk() {
        let (body_tx, _body_rx) = mpsc::channel(1);
        body_tx.try_send(Ok(Bytes::from_static(b"queued"))).unwrap();
        let (cancellation, cancel_rx) = MirrorCancellation::new();
        let reason = cancellation.reason_handle();
        let mut ctx = EdgionHttpContext::default();
        ctx.set_mirror_state(MirrorState::Streaming {
            body_tx,
            body_budget: crate::plugins::http::request_mirror::MirrorBodyBudget::for_test(1024),
            cancellation,
            writer_handle: tokio::spawn(async {}),
            channel_full_timeout_ms: 0,
        });
        let mut body = Some(Bytes::from_static(b"main"));

        mirror_body_tee(&mut body, RequestBodyEvent::Data, &mut ctx).await;

        assert_eq!(body.as_deref(), Some(&b"main"[..]));
        assert!(!ctx.has_mirror_state());
        assert!(!ctx.has_request_body_plugins());
        assert!(cancel_rx.await.is_ok());
        assert_eq!(
            MirrorCancellation::load_reason(&reason),
            Some(MirrorAbandonReason::ChannelFull)
        );
    }

    #[tokio::test]
    async fn mirror_byte_limit_cancels_only_the_mirror_and_preserves_main_chunk() {
        let (body_tx, mut body_rx) = mpsc::channel(5);
        let budget = crate::plugins::http::request_mirror::MirrorBodyBudget::for_test(4);
        let (cancellation, cancel_rx) = MirrorCancellation::new();
        let reason = cancellation.reason_handle();
        let mut ctx = EdgionHttpContext::default();
        ctx.set_mirror_state(MirrorState::Streaming {
            body_tx,
            body_budget: budget.clone(),
            cancellation,
            writer_handle: tokio::spawn(async {}),
            channel_full_timeout_ms: 1000,
        });
        let mut body = Some(Bytes::from_static(b"1234"));
        mirror_body_tee(&mut body, RequestBodyEvent::Data, &mut ctx).await;
        let in_transport = body_rx.recv().await.unwrap().unwrap();
        assert_eq!(body.as_deref(), Some(&b"1234"[..]));
        // The queue is empty, but the transport still owns all four bytes.
        body = Some(Bytes::from_static(b"5"));
        mirror_body_tee(&mut body, RequestBodyEvent::Complete, &mut ctx).await;
        assert_eq!(body.as_deref(), Some(&b"5"[..]));
        assert!(cancel_rx.await.is_ok());
        assert_eq!(
            MirrorCancellation::load_reason(&reason),
            Some(MirrorAbandonReason::BufferLimit)
        );
        assert!(!ctx.has_mirror_state());
        assert!(!ctx.has_request_body_plugins());
        assert!(budget.try_copy(b"x").is_none());

        body = Some(Bytes::from_static(b"later"));
        mirror_body_tee(&mut body, RequestBodyEvent::Data, &mut ctx).await;
        assert_eq!(body.as_deref(), Some(&b"later"[..]));
        assert_eq!(
            MirrorCancellation::load_reason(&reason),
            Some(MirrorAbandonReason::BufferLimit)
        );

        drop(in_transport);
        assert!(budget.try_copy(b"1234").is_some());
    }

    #[tokio::test]
    async fn mirror_queue_full_does_not_allocate_and_wait_can_drain() {
        let (tx, mut rx) = mpsc::channel(1);
        let budget = crate::plugins::http::request_mirror::MirrorBodyBudget::for_test(8);
        assert!(send_mirror_chunk(&tx, &budget, b"1234", 0).await.is_ok());
        assert!(matches!(
            send_mirror_chunk(&tx, &budget, b"5678", 0).await,
            Err(MirrorSendError::Abandon(MirrorAbandonReason::ChannelFull))
        ));
        assert!(budget.try_copy(b"5678").is_some());
        let send = send_mirror_chunk(&tx, &budget, b"5678", 1000);
        let drain = async {
            drop(rx.recv().await.unwrap());
        };
        let (sent, ()) = tokio::join!(send, drain);
        assert!(sent.is_ok());
        assert_eq!(rx.recv().await.unwrap().unwrap(), "5678");
    }
}
