//! Per-attempt LB commit state.
//!
//! The guard owns **outstanding-load accounting only**. The latency sample is
//! recorded separately, at response headers, by
//! `crate::routes::http::proxy_http::pg_upstream_response_filter` — see
//! `skills/12-algorithms/lb-peak-ewma.md` (defect 2 and defect 4) for why it
//! moved and why a retried attempt now keeps its sample.

use std::sync::Arc;

use super::edgion_backend::EdgionBackend;

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
    backend: Arc<EdgionBackend>,
    /// Cleared by the first finalize; a second is a no-op.
    live: bool,
}

impl LbCommitGuard {
    fn new(backend: Arc<EdgionBackend>) -> Self {
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
/// Reads `upstream.lb_policy` and `upstream.lb_edgion_backend`, writes
/// `upstream.lb_committed`. Idempotent: a second call is a no-op.
#[inline]
pub fn commit_attempt(upstream: &mut crate::UpstreamInfo) {
    if upstream.lb_committed.is_some() {
        return;
    }
    let Some(backend) = upstream.lb_edgion_backend.as_ref().cloned() else {
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
mod tests {
    use super::*;
    use crate::lb::edgion_backend::EdgionBackend;
    use crate::UpstreamInfo;
    use edgion_resources::resources::lb_policy::ConsistentHashOn;
    use edgion_resources::resources::lb_policy::ParsedLBPolicy;
    use pingora_core::protocols::l4::socket::SocketAddr;
    use pingora_load_balancing::Backend;
    use std::net::SocketAddr as StdSocketAddr;
    use std::sync::Arc;
    use std::time::Instant;

    fn make_backend(port: u16) -> Arc<EdgionBackend> {
        Arc::new(EdgionBackend::new(Backend {
            addr: SocketAddr::Inet(StdSocketAddr::from(([127, 0, 0, 1], port))),
            weight: 1,
            ext: Default::default(),
        }))
    }

    fn make_upstream(backend: Arc<EdgionBackend>, policy: Option<ParsedLBPolicy>) -> UpstreamInfo {
        UpstreamInfo {
            ip: None,
            port: None,
            status: None,
            ct: None,
            ht: None,
            bt: None,
            et: None,
            wpt: None,
            response_body_size: None,
            start_time: Instant::now(),
            err: Vec::new(),
            lb_backend_addr: None,
            lb_edgion_backend: Some(backend),
            lb_outlier_completion: None,
            service_key: None,
            lb_policy: policy,
            tls: None,
            lb_committed: None,
        }
    }

    #[test]
    fn both_scoring_policies_increment_conn() {
        // Ewma now needs the count too: its score is `latency * (conn + 1)`.
        for policy in [ParsedLBPolicy::LeastConn, ParsedLBPolicy::Ewma] {
            let b = make_backend(10001);
            let mut up = make_upstream(b.clone(), Some(policy.clone()));
            commit_attempt(&mut up);
            assert_eq!(b.conn(), 1, "{policy:?} must inc_conn at connect");
            assert!(up.lb_committed.is_some());
        }
    }

    #[test]
    fn non_scoring_policies_do_not_commit() {
        for policy in [None, Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::SourceIp))] {
            let b = make_backend(10002);
            let mut up = make_upstream(b.clone(), policy.clone());
            commit_attempt(&mut up);
            assert_eq!(b.conn(), 0, "{policy:?} needs no outstanding-load accounting");
            assert!(up.lb_committed.is_none());
        }
    }

    #[test]
    fn finalize_balances_the_increment() {
        let b = make_backend(10003);
        let mut up = make_upstream(b.clone(), Some(ParsedLBPolicy::Ewma));
        commit_attempt(&mut up);
        assert_eq!(b.conn(), 1);
        finalize_attempt(&mut up);
        assert_eq!(b.conn(), 0);
        assert!(up.lb_committed.is_none());
    }

    #[test]
    fn finalize_is_idempotent() {
        let b = make_backend(10004);
        let mut up = make_upstream(b.clone(), Some(ParsedLBPolicy::LeastConn));
        commit_attempt(&mut up);
        finalize_attempt(&mut up);
        finalize_attempt(&mut up);
        assert_eq!(b.conn(), 0, "a second finalize must not double-decrement");
    }

    #[test]
    fn commit_is_idempotent() {
        let b = make_backend(10005);
        let mut up = make_upstream(b.clone(), Some(ParsedLBPolicy::LeastConn));
        commit_attempt(&mut up);
        commit_attempt(&mut up);
        assert_eq!(b.conn(), 1, "a second commit must not double-increment");
    }

    #[test]
    fn dropping_an_unresolved_guard_balances() {
        // The third discard path: neither `begin_upstream_attempt` nor
        // `lb_runtime_cleanup` ran, e.g. a cancelled request.
        let b = make_backend(10006);
        {
            let mut up = make_upstream(b.clone(), Some(ParsedLBPolicy::Ewma));
            commit_attempt(&mut up);
            assert_eq!(b.conn(), 1);
        }
        assert_eq!(b.conn(), 0, "Drop must balance a guard nobody finalized");
    }

    #[test]
    fn commit_records_no_latency_sample() {
        // The sample now lands in `pg_upstream_response_filter`, at response
        // headers. The guard must not touch the score at all.
        let b = make_backend(10007);
        let now = crate::lb::ewma::score::now_units();
        let before = b.latency_estimate(now);
        let mut up = make_upstream(b.clone(), Some(ParsedLBPolicy::Ewma));
        commit_attempt(&mut up);
        finalize_attempt(&mut up);
        assert_eq!(b.latency_estimate(now), before, "commit/finalize must not score");
    }
}
