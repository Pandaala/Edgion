//! Per-attempt LB commit state.
//!
//! The guard owns **outstanding-load accounting only**. The latency sample is
//! recorded separately, at response headers, by
//! `crate::routes::http::proxy_http::pg_upstream_response_filter` — see
//! `skills/12-algorithms/lb-peak-ewma.md` (defect 2 and defect 4) for why it
//! moved and why a retried attempt now keeps its sample.

use std::sync::Arc;

use super::backend::LbBackend;

/// Move-only owner for an LB attempt's outstanding-connection accounting.
///
/// `inc_conn()` ran at connect; exactly one `dec_conn()` must follow. Dropping
/// an unresolved guard balances it, which makes the accounting
/// cancellation-safe.
///
/// Both scoring policies need this: LeastConn compares the count directly, and
/// Peak-EWMA multiplies its latency estimate by `conn + 1`.
#[derive(Debug)]
pub struct LbCommitGuard {
    backend: Arc<LbBackend>,
    /// Cleared by the first finalize; a second is a no-op.
    live: bool,
}

impl LbCommitGuard {
    fn new(backend: Arc<LbBackend>) -> Self {
        Self { backend, live: true }
    }

    fn finalize(&mut self) {
        if std::mem::take(&mut self.live) {
            self.backend.dec_conn();
        }
    }
}

impl Drop for LbCommitGuard {
    fn drop(&mut self) {
        self.finalize();
    }
}

/// Record LB commit state after a successful upstream connect.
///
/// Reads `upstream.lb_policy` and `upstream.lb_backend`, writes
/// `upstream.lb_committed`. Idempotent: a second call is a no-op.
#[inline]
pub fn commit_attempt(upstream: &mut crate::UpstreamInfo) {
    if upstream.lb_committed.is_some() {
        return;
    }
    let Some(backend) = upstream.lb_backend.as_ref() else {
        return;
    };
    // Both scoring policies maintain the count; the non-scoring ones do not
    // need it and are skipped so their backends pay nothing.
    let scoring = matches!(
        &upstream.lb_policy,
        Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn)
            | Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::Ewma)
    );
    if scoring {
        let backend = Arc::clone(backend);
        backend.inc_conn();
        upstream.lb_committed = Some(LbCommitGuard::new(backend));
    }
}

/// Finalize and consume the commit on this upstream entry.
/// Idempotent: a second call sees `None` and does nothing.
#[inline]
pub fn finalize_attempt(upstream: &mut crate::UpstreamInfo) {
    if let Some(mut commit) = upstream.lb_committed.take() {
        commit.finalize();
    }
}

#[cfg(test)]
#[path = "../../tests/unit/lb/attempt_test.rs"]
mod tests;
