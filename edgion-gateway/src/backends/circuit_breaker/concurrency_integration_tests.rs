//! Concurrency correctness tests for the per-endpoint circuit-breaker permit.
//!
//! The integration harness (`run_integration.sh`) is a sequential curl-based
//! status checker with no concurrent-request driver and no holding upstream, so
//! "N concurrent requests, excess shed" cannot be asserted there. These
//! multi-threaded tests are the real proof of the admit-time cap at the
//! `CbPermit` / `EdgionBackend` layer.

use super::CbPermit;
use crate::lb::edgion_backend::EdgionBackend;
use pingora_load_balancing::Backend;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

fn backend() -> Arc<EdgionBackend> {
    Arc::new(EdgionBackend::new(Backend::new("127.0.0.1:1").unwrap()))
}

/// Under heavy contention, at most `max` acquires may succeed — the atomic
/// check-and-increment must never admit more than the cap.
#[test]
fn exactly_max_permits_win_under_contention() {
    const THREADS: usize = 50;
    const MAX: u32 = 10;
    let b = backend();
    let barrier = Arc::new(Barrier::new(THREADS));
    let mut handles = Vec::new();
    for _ in 0..THREADS {
        let b = b.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            CbPermit::try_acquire(b, MAX)
        }));
    }
    let winners: Vec<_> = handles.into_iter().filter_map(|h| h.join().unwrap()).collect();
    assert_eq!(winners.len(), MAX as usize);
    assert_eq!(b.cb_inflight(), MAX as u64);
    drop(winners);
    assert_eq!(b.cb_inflight(), 0);
}

/// The cap is per endpoint: saturating backend A must not affect backend B.
#[test]
fn per_endpoint_independent() {
    let a = backend();
    let b = backend();
    let _pa = CbPermit::try_acquire(a.clone(), 1).unwrap();
    assert!(CbPermit::try_acquire(a.clone(), 1).is_none()); // A is full
                                                            // B is a different endpoint — its own slot is free.
    let _pb = CbPermit::try_acquire(b.clone(), 1).unwrap();
    assert_eq!(a.cb_inflight(), 1);
    assert_eq!(b.cb_inflight(), 1);
}

/// After in-flight requests drain (permits dropped), the endpoint readmits —
/// mirrors "shed under load, succeed once concurrency drops".
#[test]
fn drain_then_readmit() {
    const MAX: u32 = 3;
    let b = backend();
    let mut held: Vec<CbPermit> = (0..MAX)
        .map(|_| CbPermit::try_acquire(b.clone(), MAX).unwrap())
        .collect();
    assert!(CbPermit::try_acquire(b.clone(), MAX).is_none()); // saturated
    held.pop(); // drain one
    assert_eq!(b.cb_inflight(), (MAX - 1) as u64);
    let _re = CbPermit::try_acquire(b.clone(), MAX).unwrap(); // readmitted
    drop(held);
}

/// Concurrent acquire+drop churn must never leak or exceed the cap, and must
/// return to zero once all guards are dropped.
#[test]
fn churn_never_exceeds_cap_and_settles_to_zero() {
    const THREADS: usize = 32;
    const MAX: u32 = 4;
    let b = backend();
    let barrier = Arc::new(Barrier::new(THREADS));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..THREADS {
        let b = b.clone();
        let barrier = barrier.clone();
        let peak = peak.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            for _ in 0..200 {
                if let Some(p) = CbPermit::try_acquire(b.clone(), MAX) {
                    let cur = b.cb_inflight() as usize;
                    peak.fetch_max(cur, Ordering::SeqCst);
                    drop(p);
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert!(
        peak.load(Ordering::SeqCst) <= MAX as usize,
        "cap exceeded: {}",
        peak.load(Ordering::SeqCst)
    );
    assert_eq!(b.cb_inflight(), 0, "permits leaked");
}
