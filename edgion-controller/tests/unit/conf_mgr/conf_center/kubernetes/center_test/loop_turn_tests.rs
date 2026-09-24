use super::*;
use std::pin::pin;
use tokio_util::sync::CancellationToken;

fn new_set() -> WatcherSet {
    WatcherSet::new(CancellationToken::new())
}

#[tokio::test(start_paused = true)]
async fn watcher_panic_is_attributed_by_name() {
    let (_tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
    let mut watchers = new_set();
    watchers.spawn("boomer", async { panic!("boom") });
    let mut deadline = pin!(tokio::time::sleep(Duration::from_secs(90)));
    let turn = next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), true).await;
    assert!(matches!(turn, LoopTurn::WatcherPanicked("boomer")));
}

#[tokio::test(start_paused = true)]
async fn event_sent_before_return_is_not_lost() {
    let (tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
    let mut watchers = new_set();
    watchers.spawn("oneshot", async move {
        let _ = tx.send(LifecycleEvent::CachesReady).await;
    });
    // Drive turns until the event arrives; a WatcherFinished in between is
    // fine (either order is legal), but the event must never be dropped.
    let mut saw_event = false;
    for _ in 0..3 {
        let mut deadline = pin!(tokio::time::sleep(Duration::from_secs(90)));
        match next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), true).await {
            LoopTurn::Event(LifecycleEvent::CachesReady) => {
                saw_event = true;
                break;
            }
            LoopTurn::WatcherFinished("oneshot") => continue,
            other => panic!("unexpected turn: {:?}", std::mem::discriminant(&other)),
        }
    }
    assert!(saw_event);
}

#[tokio::test(start_paused = true)]
async fn armed_deadline_fires_when_nothing_happens() {
    let (_tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
    let mut watchers = new_set();
    watchers.spawn("idle", async { std::future::pending::<()>().await });
    let mut deadline = pin!(tokio::time::sleep(Duration::from_secs(90)));
    let turn = next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), true).await;
    assert!(matches!(turn, LoopTurn::DeadlineExpired));
}

#[tokio::test(start_paused = true)]
async fn disarmed_deadline_never_fires() {
    let (tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
    let mut watchers = new_set();
    watchers.spawn("idle", async { std::future::pending::<()>().await });
    // Deadline already elapsed but disarmed — the turn must wait for the event.
    let mut deadline = pin!(tokio::time::sleep(Duration::from_millis(1)));
    tokio::time::sleep(Duration::from_millis(5)).await;
    let sender = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(600)).await;
        let _ = sender.send(LifecycleEvent::ReloadRequested).await;
    });
    let turn = next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), false).await;
    assert!(matches!(turn, LoopTurn::Event(LifecycleEvent::ReloadRequested)));
}

#[tokio::test(start_paused = true)]
async fn closed_channel_is_reported() {
    let (tx, mut rx) = mpsc::channel::<LifecycleEvent>(4);
    drop(tx);
    let mut watchers = new_set();
    let mut deadline = pin!(tokio::time::sleep(Duration::from_secs(90)));
    let turn = next_loop_turn(&mut rx, &mut watchers, deadline.as_mut(), true).await;
    assert!(matches!(turn, LoopTurn::ChannelClosed));
}
