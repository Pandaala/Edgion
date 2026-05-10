//! Per-attempt LB commit state and finalize helpers.

use std::time::Instant;

/// One LB attempt's committed accounting record.
/// Stored on `UpstreamInfo` (per-attempt entry); the backend Arc is NOT stored
/// here — it is already on the same entry as `lb_edgion_backend`.
#[derive(Debug, Clone)]
pub enum LbCommit {
    /// `inc_conn()` was called at connect; finalize must call `dec_conn()`.
    LeastConn,
    /// Attempt start recorded; `Settle` finalize updates EWMA,
    /// `Discard` drops the sample (retry must not pollute prior backend).
    Ewma { started: Instant },
}

/// How a per-attempt LB commit is finalized.
#[derive(Debug, Clone, Copy)]
pub enum FinalizeMode {
    /// Attempt abandoned (e.g. retry). EWMA sample dropped; LeastConn still
    /// decrements to balance the earlier `inc_conn()`.
    Discard,
    /// Attempt is the request's final outcome. EWMA sample folded in.
    Settle,
}

/// Record LB commit state after a successful upstream connect.
/// Reads `upstream.lb_policy` and `upstream.lb_edgion_backend`, writes
/// `upstream.lb_committed`. Idempotent: a second call is a no-op.
#[inline]
pub fn commit_attempt(upstream: &mut crate::types::UpstreamInfo) {
    if upstream.lb_committed.is_some() {
        return;
    }
    let Some(backend) = upstream.lb_edgion_backend.as_ref() else {
        return;
    };
    upstream.lb_committed = match &upstream.lb_policy {
        Some(crate::types::ParsedLBPolicy::LeastConn) => {
            backend.inc_conn();
            Some(LbCommit::LeastConn)
        }
        Some(crate::types::ParsedLBPolicy::Ewma) => Some(LbCommit::Ewma {
            started: upstream.start_time,
        }),
        _ => None,
    };
}

/// Finalize and consume the commit on this upstream entry.
/// Idempotent: a second call sees `None` from `take()` and does nothing.
#[inline]
pub fn finalize_attempt(upstream: &mut crate::types::UpstreamInfo, mode: FinalizeMode) {
    let Some(commit) = upstream.lb_committed.take() else {
        return;
    };
    let Some(backend) = upstream.lb_edgion_backend.as_ref() else {
        return;
    };
    match commit {
        LbCommit::LeastConn => backend.dec_conn(),
        LbCommit::Ewma { started } => {
            if let FinalizeMode::Settle = mode {
                let latency_us = started.elapsed().as_micros() as u64;
                backend.update_ewma(latency_us);
            }
            // FinalizeMode::Discard: drop the sample — retry elapsed time must
            // not pollute the previous backend's EWMA.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::lb::edgion_backend::EdgionBackend;
    use crate::types::{ParsedLBPolicy, UpstreamInfo};
    use pingora_core::protocols::l4::socket::SocketAddr;
    use pingora_load_balancing::Backend;
    use std::net::SocketAddr as StdSocketAddr;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn make_backend(port: u16) -> Arc<EdgionBackend> {
        Arc::new(EdgionBackend::new(Backend {
            addr: SocketAddr::Inet(StdSocketAddr::from(([127, 0, 0, 1], port))),
            weight: 1,
            ext: Default::default(),
        }))
    }

    fn make_upstream(backend: Arc<EdgionBackend>, policy: ParsedLBPolicy) -> UpstreamInfo {
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
            service_key: None,
            lb_policy: Some(policy),
            tls: None,
            lb_committed: None,
        }
    }

    #[test]
    fn commit_leastconn_increments_and_records() {
        let b = make_backend(10001);
        let mut up = make_upstream(b.clone(), ParsedLBPolicy::LeastConn);
        commit_attempt(&mut up);
        assert_eq!(b.conn(), 1);
        assert!(matches!(up.lb_committed, Some(LbCommit::LeastConn)));
    }

    #[test]
    fn commit_ewma_records_start_without_inc_conn() {
        let b = make_backend(10002);
        let mut up = make_upstream(b.clone(), ParsedLBPolicy::Ewma);
        commit_attempt(&mut up);
        assert_eq!(b.conn(), 0, "EWMA commit must not call inc_conn");
        assert!(matches!(up.lb_committed, Some(LbCommit::Ewma { .. })));
    }

    #[test]
    fn commit_is_idempotent() {
        let b = make_backend(10003);
        let mut up = make_upstream(b.clone(), ParsedLBPolicy::LeastConn);
        commit_attempt(&mut up);
        commit_attempt(&mut up);
        assert_eq!(b.conn(), 1, "second commit must be a no-op");
    }

    #[test]
    fn commit_no_op_without_backend() {
        let mut up = UpstreamInfo {
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
            lb_edgion_backend: None,
            service_key: None,
            lb_policy: Some(ParsedLBPolicy::LeastConn),
            tls: None,
            lb_committed: None,
        };
        commit_attempt(&mut up); // must not panic
        assert!(up.lb_committed.is_none());
    }

    #[test]
    fn finalize_settle_leastconn_decrements() {
        let b = make_backend(10004);
        let mut up = make_upstream(b.clone(), ParsedLBPolicy::LeastConn);
        commit_attempt(&mut up);
        assert_eq!(b.conn(), 1);
        finalize_attempt(&mut up, FinalizeMode::Settle);
        assert_eq!(b.conn(), 0);
        assert!(up.lb_committed.is_none());
    }

    #[test]
    fn finalize_discard_leastconn_still_decrements() {
        let b = make_backend(10005);
        let mut up = make_upstream(b.clone(), ParsedLBPolicy::LeastConn);
        commit_attempt(&mut up);
        finalize_attempt(&mut up, FinalizeMode::Discard);
        assert_eq!(b.conn(), 0, "Discard must still decrement to balance inc_conn");
    }

    #[test]
    fn finalize_settle_ewma_updates_sample() {
        let b = make_backend(10006);
        let initial = b.ewma();
        // Use a past start_time so elapsed() is measurable.
        let mut up = UpstreamInfo {
            start_time: Instant::now() - Duration::from_millis(50),
            ..make_upstream(b.clone(), ParsedLBPolicy::Ewma)
        };
        commit_attempt(&mut up);
        finalize_attempt(&mut up, FinalizeMode::Settle);
        assert_ne!(b.ewma(), initial, "Settle must fold the EWMA sample");
    }

    #[test]
    fn finalize_discard_ewma_drops_sample() {
        let b = make_backend(10007);
        let initial = b.ewma();
        let mut up = make_upstream(b.clone(), ParsedLBPolicy::Ewma);
        commit_attempt(&mut up);
        finalize_attempt(&mut up, FinalizeMode::Discard);
        assert_eq!(b.ewma(), initial, "Discard must not call update_ewma");
    }

    #[test]
    fn finalize_is_idempotent() {
        let b = make_backend(10008);
        let mut up = make_upstream(b.clone(), ParsedLBPolicy::LeastConn);
        commit_attempt(&mut up);
        finalize_attempt(&mut up, FinalizeMode::Settle);
        assert_eq!(b.conn(), 0);
        b.inc_conn(); // separate in-flight on same backend
        finalize_attempt(&mut up, FinalizeMode::Settle); // lb_committed=None — must be no-op
        assert_eq!(b.conn(), 1, "second finalize must not decrement again");
    }
}
