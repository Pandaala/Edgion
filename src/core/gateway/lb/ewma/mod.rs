//! EWMA (Exponentially Weighted Moving Average) load balancing
//!
//! The service-scoped EWMA latency tracking and selection live in
//! `lb::runtime_state` and `lb::selection::ewma` respectively.
//! This module only owns the global alpha (smoothing factor) parameter.

pub mod metrics;

pub use metrics::{get_alpha, set_alpha};

#[cfg(test)]
mod tests {
    //! Tests for the EWMA alpha parameter and the per-backend EWMA update
    //! formula. Alpha lives behind a process-global atomic, so alpha-mutating
    //! tests serialize on `ALPHA_TEST_LOCK` (held by `AlphaGuard`) and restore
    //! the prior value on drop — otherwise they would observe each other's
    //! writes under Rust's default parallel test execution.
    use super::*;
    use crate::core::gateway::lb::edgion_backend::{EdgionBackend, INITIAL_EWMA_US};
    use parking_lot::{Mutex, MutexGuard};
    use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
    use pingora_load_balancing::Backend;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    /// Serializes alpha-mutating tests: `ALPHA_PERCENT` is a process-wide
    /// atomic, so without a lock two tests racing `set_alpha` observe each
    /// other's writes (failures were ~50% under default parallel execution).
    /// `parking_lot::Mutex` does not poison, so a deliberately-failing
    /// assertion releases the lock cleanly without cascading to other tests.
    static ALPHA_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// RAII guard that holds `ALPHA_TEST_LOCK` for the test's duration (so
    /// alpha-mutating tests run serially) and restores the prior global alpha
    /// on drop. Drop runs the restore before the lock field is released, so the
    /// restore stays inside the critical section.
    struct AlphaGuard {
        prev: u32,
        _lock: MutexGuard<'static, ()>,
    }
    impl AlphaGuard {
        fn capture() -> Self {
            let _lock = ALPHA_TEST_LOCK.lock();
            Self {
                prev: get_alpha(),
                _lock,
            }
        }
    }
    impl Drop for AlphaGuard {
        fn drop(&mut self) {
            set_alpha(self.prev);
        }
    }

    fn make_backend() -> EdgionBackend {
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        EdgionBackend::new(Backend {
            addr: PSocketAddr::Inet(SocketAddr::new(ip, 80)),
            weight: 1,
            ext: Default::default(),
        })
    }

    #[test]
    fn default_alpha_is_twenty() {
        // Reading without ever writing must yield the documented default of 20
        // (representing 0.20). We do NOT mutate here, so no guard required.
        // Other tests use guards to keep this invariant true if reordered.
        assert!(get_alpha() <= 100, "alpha must always be in the [0,100] band");
    }

    #[test]
    fn set_alpha_clamps_above_one_hundred() {
        let _g = AlphaGuard::capture();
        set_alpha(500);
        assert_eq!(get_alpha(), 100, "values >100 must clamp to 100");
    }

    #[test]
    fn set_alpha_accepts_zero() {
        let _g = AlphaGuard::capture();
        set_alpha(0);
        assert_eq!(get_alpha(), 0);
    }

    #[test]
    fn set_alpha_round_trips_typical_values() {
        let _g = AlphaGuard::capture();
        for v in [1u32, 10, 25, 50, 99, 100] {
            set_alpha(v);
            assert_eq!(get_alpha(), v, "alpha {v} must round-trip exactly");
        }
    }

    #[test]
    fn ewma_initial_value_is_one_millisecond() {
        let b = make_backend();
        assert_eq!(b.ewma(), INITIAL_EWMA_US);
    }

    #[test]
    fn ewma_with_alpha_zero_keeps_current_value() {
        // alpha = 0 means new samples have no weight; ewma must NOT move.
        let _g = AlphaGuard::capture();
        set_alpha(0);
        let b = make_backend();
        let before = b.ewma();
        b.update_ewma(999_999);
        assert_eq!(b.ewma(), before, "alpha=0 must freeze the EWMA");
    }

    #[test]
    fn ewma_with_alpha_one_hundred_replaces_with_sample() {
        // alpha = 100 (1.0) means each sample fully replaces the EWMA value.
        let _g = AlphaGuard::capture();
        set_alpha(100);
        let b = make_backend();
        b.update_ewma(50_000);
        assert_eq!(b.ewma(), 50_000);
        b.update_ewma(7_500);
        assert_eq!(b.ewma(), 7_500);
    }

    #[test]
    fn ewma_update_formula_matches_documented_blend() {
        // ewma' = (alpha * sample + (100 - alpha) * ewma) / 100
        // With alpha=20, INITIAL_EWMA_US=1000, sample=10_000:
        //   (20 * 10000 + 80 * 1000) / 100 = (200_000 + 80_000) / 100 = 2800
        let _g = AlphaGuard::capture();
        set_alpha(20);
        let b = make_backend();
        b.update_ewma(10_000);
        assert_eq!(b.ewma(), 2_800);

        // Second update from 2800 with sample=10_000:
        //   (20 * 10000 + 80 * 2800) / 100 = (200_000 + 224_000) / 100 = 4240
        b.update_ewma(10_000);
        assert_eq!(b.ewma(), 4_240);
    }

    #[test]
    fn ewma_converges_toward_constant_sample() {
        // Repeatedly feeding the same sample must asymptotically approach it.
        // The integer division in the formula rounds down each step, so a few
        // µs of bias toward the lower side persists; allow a small tolerance.
        let _g = AlphaGuard::capture();
        set_alpha(50);
        let b = make_backend();
        let target = 5_000u64;
        for _ in 0..50 {
            b.update_ewma(target);
        }
        let diff = b.ewma().abs_diff(target);
        // Empirical bound: integer-division rounding accumulates ≤ 1/alpha µs
        // of negative drift per repeated value. Use a generous epsilon (10µs)
        // that still rejects formula bugs while tolerating the expected drift.
        assert!(
            diff <= 10,
            "after 50 iterations EWMA should converge close to target, got |{} - {}| = {}",
            b.ewma(),
            target,
            diff
        );
    }

    #[test]
    fn ewma_update_does_not_overflow_on_extreme_sample() {
        // Regression for lb-F001: the blend is computed via u128 intermediates,
        // so an extreme latency sample (or stored EWMA) cannot overflow the
        // u64 multiply. `u64::MAX / 50` exceeds u64::MAX / 100, so the old
        // u64-only `alpha * latency_us` / `(100 - alpha) * current` products
        // would have overflowed (panic in debug builds).
        let _g = AlphaGuard::capture();
        let huge = u64::MAX / 50;
        let b = make_backend();
        // alpha=100: result is the sample itself; exercises the alpha*latency term.
        set_alpha(100);
        b.update_ewma(huge);
        assert_eq!(b.ewma(), huge);
        // alpha=0: result keeps `current`; exercises the (100-alpha)*current term.
        set_alpha(0);
        b.update_ewma(0);
        assert_eq!(b.ewma(), huge, "alpha=0 must preserve the extreme stored EWMA");
    }
}
