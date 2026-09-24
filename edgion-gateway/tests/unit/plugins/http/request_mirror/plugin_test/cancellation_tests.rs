use super::*;

#[tokio::test]
async fn explicit_cancellation_wins_when_result_is_also_ready() {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    cancel_tx.send(()).unwrap();

    assert_eq!(wait_for_send_or_cancel(async { 7_u8 }, cancel_rx).await, None);
}

#[tokio::test]
async fn clean_completion_waits_for_send_result() {
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    drop(cancel_tx);

    assert_eq!(wait_for_send_or_cancel(async { 7_u8 }, cancel_rx).await, Some(7));
}

#[test]
fn cancellation_reason_is_recorded_before_notifying_task() {
    let (cancellation, mut cancel_rx) = MirrorCancellation::new();
    let reason = cancellation.reason_handle();
    cancellation.cancel(MirrorAbandonReason::ChannelFull);

    assert_eq!(
        MirrorCancellation::load_reason(&reason),
        Some(MirrorAbandonReason::ChannelFull)
    );
    assert!(cancel_rx.try_recv().is_ok());
}

#[test]
fn dropping_armed_guard_fails_closed_as_body_abandoned() {
    let (cancellation, mut cancel_rx) = MirrorCancellation::new();
    let reason = cancellation.reason_handle();
    drop(cancellation);

    assert_eq!(
        MirrorCancellation::load_reason(&reason),
        Some(MirrorAbandonReason::BodyAbandoned)
    );
    assert_eq!(mirror_abandon_status(&reason), Some("body_abandoned"));
    assert!(cancel_rx.try_recv().is_ok());
}
