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
#[path = "../../tests/unit/services/supervision_test.rs"]
mod tests;
