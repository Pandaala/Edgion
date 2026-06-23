//! Process-local monotonic `resource_version` counter.
//!
//! The controller assigns a `resource_version` to every write that passes
//! through its event store. The counter is a plain in-process `AtomicU64`
//! seeded at boot by [`bump_resource_version_floor`] so that, after a restart,
//! the first value returned by [`next_resource_version`] is strictly greater
//! than any version already persisted on disk.
//!
//! Init ordering is enforced by `controller::api::require_ready_middleware`,
//! which returns 503 on every write route until every processor has finished
//! `on_init_done` — that's where the floor reconstruction happens. New write
//! routes that allocate `resource_version` MUST be mounted behind that
//! middleware; otherwise the floor reconstruction can race against external
//! allocation.

use std::sync::atomic::{AtomicU64, Ordering};

static LAST_VERSION: AtomicU64 = AtomicU64::new(0);

/// Allocate the next `resource_version` as a monotonically increasing `u64`.
///
/// Safe to call concurrently across threads.
pub fn next_resource_version() -> u64 {
    LAST_VERSION.fetch_add(1, Ordering::Relaxed) + 1
}

/// Raise the [`next_resource_version`] floor to at least `rv`.
///
/// Idempotent and concurrent-safe; `rv == 0` is a no-op. Intended to be called
/// once per persisted resource during init, before any external writer is
/// allowed to allocate a new `resource_version`.
pub fn bump_resource_version_floor(rv: u64) {
    if rv > 0 {
        // Relaxed is sufficient: `LAST_VERSION` is self-contained and the
        // init-before-external-write happens-before is provided by the
        // readiness gate, not by this counter.
        LAST_VERSION.fetch_max(rv, Ordering::Relaxed);
    }
}

/// Peek at the last-issued `resource_version` without incrementing it.
///
/// Returns 0 if [`next_resource_version`] has never been called. The value is
/// a process-local sequence number (not a wall-clock timestamp). Intended for
/// diagnostic/stats use only — not for assigning versions to resources.
pub fn current_resource_version() -> u64 {
    LAST_VERSION.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn generates_monotonic_versions() {
        let first = next_resource_version();
        let second = next_resource_version();
        assert!(second > first);
    }

    #[test]
    fn concurrent_calls_are_unique_and_ordered() {
        let handles: Vec<_> = (0..64).map(|_| thread::spawn(next_resource_version)).collect();

        let mut values: Vec<u64> = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread panicked"))
            .collect();

        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), 64);

        for window in values.windows(2) {
            assert!(window[1] > window[0]);
        }
    }

    #[test]
    fn bump_resource_version_floor_raises_floor() {
        let baseline = next_resource_version();
        let target = baseline + 1_000_000;
        bump_resource_version_floor(target);
        let next = next_resource_version();
        assert!(
            next > target,
            "next_resource_version() should be strictly greater than the bumped floor (next={}, target={})",
            next,
            target
        );
    }

    #[test]
    fn bump_resource_version_floor_no_regression() {
        let baseline = next_resource_version();
        let high = baseline + 2_000_000;
        let low = baseline + 1_000_000;
        bump_resource_version_floor(high);
        bump_resource_version_floor(low);
        let next = next_resource_version();
        assert!(
            next > high,
            "a lower bump must not roll the floor back (next={}, high={})",
            next,
            high
        );
    }
}
