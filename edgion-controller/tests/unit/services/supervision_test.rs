use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn panic_with_current_generation_invokes_closure() {
    let invoked = Arc::new(AtomicBool::new(false));
    let flag = invoked.clone();
    let worker = tokio::spawn(async { panic!("boom") });
    observe_worker("test_job", worker, move || {
        flag.store(true, Ordering::SeqCst);
        true
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(invoked.load(Ordering::SeqCst), "closure must run on panic");
}

#[tokio::test]
async fn clean_return_invokes_closure_for_classification() {
    // A worker returning Ok(()) may be an orderly drain-based stop OR an
    // unexpected exit; the CLOSURE decides via generation. The helper must
    // still invoke it.
    let invoked = Arc::new(AtomicBool::new(false));
    let flag = invoked.clone();
    let worker = tokio::spawn(async {});
    observe_worker("test_job", worker, move || {
        flag.store(true, Ordering::SeqCst);
        false // classified stale → helper must not treat as death (no way to assert the log; the return path is enough)
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(invoked.load(Ordering::SeqCst));
}

#[tokio::test]
async fn aborted_worker_never_invokes_closure() {
    let invoked = Arc::new(AtomicBool::new(false));
    let flag = invoked.clone();
    let worker = tokio::spawn(async {
        std::future::pending::<()>().await;
    });
    worker.abort();
    observe_worker("test_job", worker, move || {
        flag.store(true, Ordering::SeqCst);
        true
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!invoked.load(Ordering::SeqCst), "orderly abort must not classify");
}
