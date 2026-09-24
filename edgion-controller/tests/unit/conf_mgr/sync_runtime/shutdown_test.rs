use super::*;

#[tokio::test]
async fn test_shutdown_signal() {
    let handle = ShutdownHandle::new();
    let mut signal = handle.signal();

    assert!(!signal.is_shutdown());
    assert!(!handle.is_shutdown());

    handle.shutdown();

    assert!(signal.is_shutdown());
    assert!(handle.is_shutdown());

    // wait should return immediately
    signal.wait().await;
}
