//! DNS-01 propagation poll loop (Phase 3 of `execute_dns01_flow`).
//!
//! After the TXT records are created, poll the zone's authoritative
//! nameservers at `propagationCheckInterval` cadence and activate the ACME
//! challenges as soon as every record is visible; `propagationTimeout` bounds
//! the wait. The deadline is evaluated between rounds and one final round
//! deliberately runs at expiry, so a round of hanging checks (per-record
//! budget [`CHECK_ROUND_BUDGET`]) may run past the deadline — a loose bound,
//! accepted on this cold path in exchange for a definitive final
//! observation. The loop core is checker-injectable so unit tests drive it
//! without any network.

use std::future::Future;
use std::time::Duration;

use edgion_common::dns::txt_probe::TxtCheckOutcome;
use tokio::time::Instant;

/// Budget for one check of one record (authoritative discovery plus the
/// per-nameserver TXT queries), so a single hung round can never eat the
/// whole propagation deadline.
pub(crate) const CHECK_ROUND_BUDGET: Duration = Duration::from_secs(20);

/// Final outcome of the propagation wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PropagationOutcome {
    /// Every challenge TXT record was observed on the zone's authoritative
    /// nameservers; challenges may activate immediately.
    Propagated,
    /// The deadline expired and at least one record was definitively observed
    /// missing on a responding authoritative nameserver. Activating now would
    /// burn an ACME validation attempt, so the caller fails into the retry
    /// path.
    TimedOutConfirmedMissing,
    /// The deadline expired but the check never produced a definitive
    /// observation for the remaining records (e.g. firewalled DNS egress or
    /// discovery failure). With no evidence either way this is equivalent to
    /// the legacy fixed wait, so the caller proceeds optimistically.
    TimedOutUnverified,
}

/// TXT record name for a challenge domain. The ACME authorization identifier
/// already carries the wildcard label stripped; strip defensively anyway to
/// mirror the DNS providers' record construction.
pub(crate) fn challenge_txt_fqdn(domain: &str) -> String {
    format!("_acme-challenge.{}", domain.strip_prefix("*.").unwrap_or(domain))
}

/// Poll `records` (`(fqdn, expected-value)` pairs) until all are observed or
/// `timeout` expires. A record that ever produced a definitive
/// [`TxtCheckOutcome::NotFound`] marks the timeout as confirmed-missing; a
/// record only ever [`TxtCheckOutcome::Unreachable`] does not.
pub(crate) async fn poll_propagation<C, Fut>(
    records: &[(String, String)],
    interval: Duration,
    timeout: Duration,
    check: C,
) -> PropagationOutcome
where
    C: Fn(String, String) -> Fut,
    Fut: Future<Output = TxtCheckOutcome>,
{
    struct Pending {
        fqdn: String,
        expected: String,
        saw_definitive: bool,
    }

    let deadline = Instant::now() + timeout;
    let mut pending: Vec<Pending> = records
        .iter()
        .map(|(fqdn, expected)| Pending {
            fqdn: fqdn.clone(),
            expected: expected.clone(),
            saw_definitive: false,
        })
        .collect();

    loop {
        let mut still_pending = Vec::with_capacity(pending.len());
        for mut rec in pending {
            match check(rec.fqdn.clone(), rec.expected.clone()).await {
                TxtCheckOutcome::Found => {}
                TxtCheckOutcome::NotFound => {
                    rec.saw_definitive = true;
                    still_pending.push(rec);
                }
                TxtCheckOutcome::Unreachable => still_pending.push(rec),
            }
        }
        pending = still_pending;

        if pending.is_empty() {
            return PropagationOutcome::Propagated;
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        // Truncate the last sleep to the deadline so a final check round runs
        // at (not before) expiry.
        tokio::time::sleep(interval.min(deadline - now)).await;
    }

    if pending.iter().all(|rec| !rec.saw_definitive) {
        PropagationOutcome::TimedOutUnverified
    } else {
        PropagationOutcome::TimedOutConfirmedMissing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn records(names: &[&str]) -> Vec<(String, String)> {
        names
            .iter()
            .map(|n| (format!("_acme-challenge.{n}"), format!("digest-{n}")))
            .collect()
    }

    const INTERVAL: Duration = Duration::from_secs(5);
    const TIMEOUT: Duration = Duration::from_secs(120);

    #[test]
    fn challenge_txt_fqdn_strips_wildcard_label() {
        assert_eq!(challenge_txt_fqdn("example.com"), "_acme-challenge.example.com");
        assert_eq!(challenge_txt_fqdn("*.example.com"), "_acme-challenge.example.com");
        assert_eq!(challenge_txt_fqdn("sub.example.com"), "_acme-challenge.sub.example.com");
    }

    #[tokio::test(start_paused = true)]
    async fn found_immediately_propagates_without_sleeping() {
        let start = Instant::now();
        let outcome = poll_propagation(&records(&["a.test"]), INTERVAL, TIMEOUT, |_, _| async {
            TxtCheckOutcome::Found
        })
        .await;
        assert_eq!(outcome, PropagationOutcome::Propagated);
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn found_after_a_few_rounds_exits_early() {
        let start = Instant::now();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = calls.clone();
        let outcome = poll_propagation(&records(&["a.test"]), INTERVAL, TIMEOUT, move |_, _| {
            let calls = calls_in.clone();
            async move {
                if calls.fetch_add(1, Ordering::SeqCst) < 3 {
                    TxtCheckOutcome::NotFound
                } else {
                    TxtCheckOutcome::Found
                }
            }
        })
        .await;
        assert_eq!(outcome, PropagationOutcome::Propagated);
        // Three NotFound rounds → three interval sleeps, then Found.
        assert_eq!(start.elapsed(), INTERVAL * 3);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn confirmed_missing_at_deadline_fails_closed() {
        let start = Instant::now();
        let outcome = poll_propagation(&records(&["a.test"]), INTERVAL, TIMEOUT, |_, _| async {
            TxtCheckOutcome::NotFound
        })
        .await;
        assert_eq!(outcome, PropagationOutcome::TimedOutConfirmedMissing);
        assert_eq!(start.elapsed(), TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn empty_record_set_propagates_immediately() {
        let start = Instant::now();
        let outcome = poll_propagation(&[], INTERVAL, TIMEOUT, |_, _| async {
            unreachable!("checker must not be called for an empty record set")
        })
        .await;
        assert_eq!(outcome, PropagationOutcome::Propagated);
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn interval_longer_than_timeout_truncates_to_deadline() {
        // Interval 50s > timeout 20s: the single sleep must be truncated to
        // the deadline, yielding exactly two check rounds (t=0 and t=20s) and
        // an elapsed time of exactly the timeout.
        let start = Instant::now();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = calls.clone();
        let outcome = poll_propagation(
            &records(&["a.test"]),
            Duration::from_secs(50),
            Duration::from_secs(20),
            move |_, _| {
                let calls = calls_in.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    TxtCheckOutcome::NotFound
                }
            },
        )
        .await;
        assert_eq!(outcome, PropagationOutcome::TimedOutConfirmedMissing);
        assert_eq!(start.elapsed(), Duration::from_secs(20));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn never_verifiable_times_out_unverified() {
        let outcome = poll_propagation(&records(&["a.test"]), INTERVAL, TIMEOUT, |_, _| async {
            TxtCheckOutcome::Unreachable
        })
        .await;
        assert_eq!(outcome, PropagationOutcome::TimedOutUnverified);
    }

    #[tokio::test(start_paused = true)]
    async fn one_definitive_miss_among_unreachable_is_confirmed_missing() {
        // Record a: found in round one. Record b: definitively missing once,
        // then only unreachable — the early NotFound must stick.
        let calls_b = Arc::new(AtomicUsize::new(0));
        let calls_b_in = calls_b.clone();
        let outcome = poll_propagation(&records(&["a.test", "b.test"]), INTERVAL, TIMEOUT, move |fqdn, _| {
            let calls_b = calls_b_in.clone();
            async move {
                if fqdn.contains("a.test") {
                    TxtCheckOutcome::Found
                } else if calls_b.fetch_add(1, Ordering::SeqCst) == 0 {
                    TxtCheckOutcome::NotFound
                } else {
                    TxtCheckOutcome::Unreachable
                }
            }
        })
        .await;
        assert_eq!(outcome, PropagationOutcome::TimedOutConfirmedMissing);
    }

    #[tokio::test(start_paused = true)]
    async fn resolved_record_is_not_rechecked() {
        let calls_a = Arc::new(AtomicUsize::new(0));
        let calls_a_in = calls_a.clone();
        let outcome = poll_propagation(&records(&["a.test", "b.test"]), INTERVAL, TIMEOUT, move |fqdn, _| {
            let calls_a = calls_a_in.clone();
            async move {
                if fqdn.contains("a.test") {
                    calls_a.fetch_add(1, Ordering::SeqCst);
                    TxtCheckOutcome::Found
                } else {
                    TxtCheckOutcome::Unreachable
                }
            }
        })
        .await;
        assert_eq!(outcome, PropagationOutcome::TimedOutUnverified);
        assert_eq!(
            calls_a.load(Ordering::SeqCst),
            1,
            "record a must be checked exactly once"
        );
    }
}
