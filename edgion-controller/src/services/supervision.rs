//! Leader-job worker death observation (metric + log only — no restart).
//!
//! Each long-lived leader-job worker keeps its `JoinHandle` and hands it to
//! [`observe_worker`]. Death is surfaced via `edgion_controller_leader_job_alive`
//! going 0 while the replica is still leader; recovery is by alert + operator
//! action (`/api/v1/reload` or pod restart). A blind in-place restart was
//! deliberately rejected: a panicking worker usually means corrupted state and
//! risks a panic loop (see the wedge-recovery design spec).

use tokio::task::JoinHandle;

/// Observe a leader-job worker until it ends.
///
/// - `JoinError::is_cancelled()` (abort) ⇒ orderly stop: only the service's
///   own `stop_*` fn ever aborts a worker; nothing to report.
/// - Any other end (clean return or panic) ⇒ `confirm_dead_current` decides:
///   it must take the service's own global lock, compare its captured
///   generation against the currently registered one, perform the gauge-0
///   write INSIDE the lock when current (check-then-write outside the lock
///   would race a concurrent stop+start), and return whether it was current.
///   A stale observer (drain-based stop completing after a restart) returns
///   false and writes nothing.
pub(crate) fn observe_worker<C>(job: &'static str, handle: JoinHandle<()>, confirm_dead_current: C)
where
    C: FnOnce() -> bool + Send + 'static,
{
    tokio::spawn(async move {
        let result = handle.await;
        if matches!(&result, Err(e) if e.is_cancelled()) {
            return;
        }
        if confirm_dead_current() {
            tracing::error!(
                component = "leader_job_supervision",
                leader_job = job,
                panicked = result.is_err(),
                "Leader job worker died unexpectedly; the job stays down for the rest of the \
                 leadership term — recover via /api/v1/reload or pod restart"
            );
        }
    });
}

#[cfg(test)]
mod tests {
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
}
