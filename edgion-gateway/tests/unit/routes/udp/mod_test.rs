use super::*;
use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinSet;

fn key(port: u16) -> ListenerKey {
    ListenerKey::new(([127, 0, 0, 1], port).into(), "UDP")
}

struct DropCount(Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn shutdown_cancels_every_listener_and_waits_for_cleanup() {
    let registry = UdpListenerRegistry::new();
    let mut tokens = Vec::new();
    let completed = Arc::new(AtomicUsize::new(0));
    for port in [9000, 9001, 9002] {
        let cancel = CancellationToken::new();
        tokens.push(cancel.clone());
        let done = DropCount(completed.clone());
        registry
            .spawn(&key(port), cancel.clone(), TaskTracker::new(), async move {
                let _done = done;
                cancel.cancelled().await;
                tokio::task::yield_now().await;
                Ok(())
            })
            .unwrap();
    }
    let outcome = registry.shutdown(Instant::now() + Duration::from_secs(1)).await;
    assert_eq!(outcome, UdpShutdownOutcome::default());
    assert!(tokens.iter().all(CancellationToken::is_cancelled));
    assert_eq!(completed.load(Ordering::SeqCst), 3);
    assert!(registry.state.lock().listeners.is_empty());
    assert_eq!(registry.shutdown(Instant::now()).await, outcome);
}

#[tokio::test]
async fn shutdown_waits_for_descendants_after_parent_abort() {
    let registry = UdpListenerRegistry::new();
    let tracker = TaskTracker::new();
    let children = tracker.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let dropped = Arc::new(AtomicUsize::new(0));
    let child_drop = DropCount(dropped.clone());
    let parent_drop = DropCount(dropped.clone());
    registry
        .spawn(&key(9000), CancellationToken::new(), tracker.clone(), async move {
            let _parent_drop = parent_drop;
            let mut owned_children = JoinSet::new();
            owned_children.spawn(children.track_future(async move {
                let _child_drop = child_drop;
                let _ = ready_tx.send(());
                pending::<()>().await;
            }));
            pending::<()>().await;
            drop(owned_children);
            Ok(())
        })
        .unwrap();
    ready_rx.await.unwrap();

    let outcome = registry.shutdown(Instant::now()).await;
    assert!(outcome.timed_out);
    assert_eq!(outcome.failed_listeners, 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    assert!(tracker.is_closed());
    assert!(tracker.is_empty());
    assert_eq!(registry.shutdown(Instant::now()).await, outcome);
}

#[tokio::test]
async fn duplicate_and_late_registration_never_spawn() {
    let registry = UdpListenerRegistry::new();
    let original = CancellationToken::new();
    let cancel = original.clone();
    registry
        .spawn(&key(9000), original.clone(), TaskTracker::new(), async move {
            cancel.cancelled().await;
            Ok(())
        })
        .unwrap();
    for stopping in [false, true] {
        if stopping {
            registry.cancel_all();
        }
        let token = CancellationToken::new();
        let (tx, mut rx) = oneshot::channel();
        let rejected = registry.spawn(
            &key(if stopping { 9001 } else { 9000 }),
            token.clone(),
            TaskTracker::new(),
            async move {
                let _ = tx.send(());
                Ok(())
            },
        );
        assert!(rejected.is_err());
        assert!(token.is_cancelled());
        assert_eq!(rx.try_recv(), Err(oneshot::error::TryRecvError::Closed));
        assert_eq!(original.is_cancelled(), stopping);
    }
    assert_eq!(
        registry.shutdown(Instant::now() + Duration::from_secs(1)).await,
        UdpShutdownOutcome::default()
    );
}

#[tokio::test]
async fn concurrent_shutdown_callers_observe_the_same_completed_result() {
    let registry = Arc::new(UdpListenerRegistry::new());
    let cancel = CancellationToken::new();
    let (release_tx, release_rx) = oneshot::channel();
    registry
        .spawn(&key(9000), cancel.clone(), TaskTracker::new(), async move {
            release_rx.await.unwrap();
            bail!("simulated listener failure")
        })
        .unwrap();
    let first_registry = registry.clone();
    let first = tokio::spawn(async move { first_registry.shutdown(Instant::now() + Duration::from_secs(1)).await });
    cancel.cancelled().await;
    let second_registry = registry.clone();
    let second = tokio::spawn(async move { second_registry.shutdown(Instant::now() + Duration::from_secs(1)).await });
    release_tx.send(()).unwrap();
    let outcome = first.await.unwrap();
    assert!(!outcome.timed_out);
    assert_eq!(outcome.failed_listeners, 1);
    assert_eq!(second.await.unwrap(), outcome);
}
