use super::*;
use edgion_common::observe::{
    cardinality_guard::CardinalityMode,
    metrics_api::{RecorderConfig, TestRecorder},
};

fn isolated_metrics() -> (ControllerMetrics, TestRecorder) {
    let recorder = TestRecorder::new(RecorderConfig {
        service: "test",
        idle_timeout_secs: 0,
        histogram_buckets: &[],
        cardinality_cap: 10000,
        cardinality_mode: CardinalityMode::Enforce,
    })
    .unwrap();
    let owner = recorder.with(ControllerMetrics::new);
    (owner, recorder)
}

fn active(handle: &TestRecorder) -> f64 {
    let output = handle.render();
    output
        .lines()
        .find(|line| line.starts_with("edgion_controller_active{"))
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("active controller gauge must be registered")
        .parse()
        .unwrap()
}

#[test]
fn active_scopes_restore_their_own_contribution() {
    let (owner, handle) = isolated_metrics();
    let baseline = owner.active_controller();
    assert_eq!(active(&handle), 1.0);
    let first = owner.active_controller();
    let second = owner.active_controller();
    assert_eq!(active(&handle), 3.0);
    drop(first);
    assert_eq!(active(&handle), 2.0);
    drop(second);
    assert_eq!(active(&handle), 1.0);
    drop(baseline);
    assert_eq!(active(&handle), 0.0);
}

#[tokio::test]
async fn unpolled_and_started_future_destruction_preserve_baseline() {
    let (owner, handle) = isolated_metrics();
    let _baseline = owner.active_controller();
    let pending_run = || async {
        let _active = owner.active_controller();
        std::future::pending::<()>().await;
    };
    let unpolled = pending_run();
    assert_eq!(active(&handle), 1.0);
    drop(unpolled);
    assert_eq!(active(&handle), 1.0);

    let mut started = Box::pin(pending_run());
    assert!(futures::poll!(&mut started).is_pending());
    assert_eq!(active(&handle), 2.0);
    drop(started);
    assert_eq!(active(&handle), 1.0);
}

#[tokio::test]
async fn task_abort_restores_baseline_after_future_destruction() {
    let (owner, handle) = isolated_metrics();
    let owner = std::sync::Arc::new(owner);
    let _baseline = owner.active_controller();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let task_owner = owner.clone();
    let task = tokio::spawn(async move {
        let _active = task_owner.active_controller();
        started_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), started_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active(&handle), 2.0);
    task.abort();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .unwrap();
    assert!(result.unwrap_err().is_cancelled());
    assert_eq!(active(&handle), 1.0);
}

#[test]
#[cfg(panic = "unwind")]
fn unwind_restores_baseline() {
    let (owner, handle) = isolated_metrics();
    let _baseline = owner.active_controller();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _active = owner.active_controller();
        assert_eq!(active(&handle), 2.0);
        panic!("simulated controller unwind");
    }));
    assert!(result.is_err());
    assert_eq!(active(&handle), 1.0);
}
