use super::*;

#[tokio::test]
async fn owner_drop_cancels_tasks_and_rejects_late_publication() {
    let owner = PluginTaskOwner::<2>::new();
    let handle = owner.handle();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (finished, stopped) = tokio::sync::oneshot::channel::<()>();
    assert!(handle.spawn(0, async move {
        let _finished = finished;
        let _ = started.send(());
        std::future::pending::<()>().await;
    }));
    ready.await.unwrap();
    drop(owner);
    assert!(stopped.await.is_err());
    assert!(handle.with_active(|| panic!("closed scope published")).is_none());
    assert!(!handle.spawn(1, async {}));
}
