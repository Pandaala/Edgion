//! Process-global local rate gate (token bucket), per plugin entry and listener port.
//!
//! This is Stage 1 of the connection-rate plugin: a dependency-free backstop
//! that survives config hot-reload (keyed by entry scope + listener port in a
//! process-global registry) so a reload does not reset the bucket. No tracing on
//! this path.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

/// Token-bucket gate. `capacity == rate`, refilled `rate` tokens per `interval_ms`.
/// Tokens are stored scaled by 1000 (milli-tokens) in an atomic to keep the hot
/// path lock-light; the refill clock uses a short mutex only on refill.
pub struct LocalRateGate {
    origin: Instant,
    rate: AtomicU64,         // tokens per interval (== capacity)
    interval_ms: AtomicU64,  // refill interval
    milli_tokens: AtomicU64, // current tokens * 1000
    last_refill_ms: AtomicU64,
    refill_lock: Mutex<()>,
}

impl LocalRateGate {
    pub fn new(rate: u64, interval_ms: u64) -> Self {
        Self {
            origin: Instant::now(),
            rate: AtomicU64::new(rate.max(1)),
            interval_ms: AtomicU64::new(interval_ms.max(1)),
            milli_tokens: AtomicU64::new(rate.max(1) * 1000),
            last_refill_ms: AtomicU64::new(0),
            refill_lock: Mutex::new(()),
        }
    }

    /// Update rate/interval in place (hot reload).
    ///
    /// - Rate raised: immediately credit the capacity-delta so the bucket
    ///   reflects the new limit without waiting for the next refill tick.
    /// - Rate lowered: clamp current tokens to the new capacity.
    /// Current tokens are never reset to zero; the registry slot is preserved.
    pub fn update_params(&self, rate: u64, interval_ms: u64) {
        let new_rate = rate.max(1);
        let old_rate = self.rate.swap(new_rate, Ordering::Relaxed);
        self.interval_ms.store(interval_ms.max(1), Ordering::Relaxed);
        let new_cap = new_rate * 1000;
        let old_cap = old_rate * 1000;
        let cur = self.milli_tokens.load(Ordering::Relaxed);
        if new_cap > old_cap {
            // Rate raised: add the capacity difference so callers benefit immediately.
            let next = (cur + (new_cap - old_cap)).min(new_cap);
            self.milli_tokens.store(next, Ordering::Relaxed);
        } else if cur > new_cap {
            // Rate lowered: clamp to the new capacity.
            self.milli_tokens.store(new_cap, Ordering::Relaxed);
        }
    }

    /// Try to consume one connection token. Returns true if admitted.
    pub fn try_acquire(&self) -> bool {
        let elapsed = Instant::now().saturating_duration_since(self.origin);
        let now_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX).max(1);
        self.try_acquire_elapsed_ms(now_ms)
    }

    #[cfg(test)]
    fn try_acquire_at(&self, now_ms: u64) -> bool {
        self.try_acquire_elapsed_ms(now_ms)
    }

    fn try_acquire_elapsed_ms(&self, now_ms: u64) -> bool {
        self.refill(now_ms);
        loop {
            let cur = self.milli_tokens.load(Ordering::Acquire);
            if cur < 1000 {
                return false;
            }
            if self
                .milli_tokens
                .compare_exchange(cur, cur - 1000, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    fn refill(&self, now_ms: u64) {
        let _guard = self.refill_lock.lock().unwrap();
        let last = self.last_refill_ms.load(Ordering::Relaxed);
        if last == 0 {
            self.last_refill_ms.store(now_ms, Ordering::Relaxed);
            return;
        }
        if now_ms <= last {
            return;
        }
        let elapsed = now_ms - last;
        let rate = self.rate.load(Ordering::Relaxed);
        let interval = self.interval_ms.load(Ordering::Relaxed).max(1);
        let refill = (elapsed as u128 * rate as u128 * 1000 / interval as u128) as u64;
        if refill == 0 {
            return; // not enough time elapsed for a whole milli-token; keep last unchanged
        }
        let cap = rate * 1000;
        let cur = self.milli_tokens.load(Ordering::Relaxed);
        let next = (cur + refill).min(cap);
        self.milli_tokens.store(next, Ordering::Relaxed);
        self.last_refill_ms.store(now_ms, Ordering::Relaxed);
    }
}

/// Gates by owning plugin-entry scope, then by listener port.
///
/// The scope level is what keeps two `ConnectionRateLimit` entries active on one
/// port from sharing a token bucket: they would otherwise not merely share the
/// budget but rewrite each other's rate/interval through `update_params` on
/// every connection. Keying by scope first also keeps the lookup allocation-free
/// (`&str` borrows against the `String` key).
///
/// The registry has no eviction: a renamed resource, a reordered entry, or a
/// delete/recreate leaves its old scope behind for the process lifetime. Each
/// orphan is a handful of atomics plus a small port map, and the churn rate is
/// operator config changes, so this is accepted rather than plumbing a removal
/// hook from the stream plugin store.
static LOCAL_GATES: LazyLock<Mutex<HashMap<String, HashMap<u16, Arc<LocalRateGate>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Look up (or create) the process-global gate for `scope` + `listener_port`,
/// updating its rate/interval to the latest config so hot-reload changes apply
/// without losing the registry slot or the current token state. The scope is
/// stable across a reload of the same entry, so the token state survives it.
pub fn get_or_update_local_gate(scope: &str, listener_port: u16, rate: u64, interval_ms: u64) -> Arc<LocalRateGate> {
    let mut map = LOCAL_GATES.lock().unwrap();
    if let Some(g) = map.get(scope).and_then(|ports| ports.get(&listener_port)) {
        g.update_params(rate, interval_ms);
        return g.clone();
    }
    let g = Arc::new(LocalRateGate::new(rate, interval_ms));
    map.entry(scope.to_string())
        .or_default()
        .insert(listener_port, g.clone());
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_capacity_then_denies() {
        let gate = LocalRateGate::new(3, 1000);
        let t0 = 1_000_000u64;
        assert!(gate.try_acquire_at(t0));
        assert!(gate.try_acquire_at(t0));
        assert!(gate.try_acquire_at(t0));
        assert!(!gate.try_acquire_at(t0)); // 4th in same instant denied
    }

    #[test]
    fn production_method_allows_capacity_then_denies_without_sleep() {
        let gate = LocalRateGate::new(3, u64::MAX);
        assert!(gate.try_acquire());
        assert!(gate.try_acquire());
        assert!(gate.try_acquire());
        assert!(!gate.try_acquire());
    }

    #[test]
    fn refills_over_time() {
        let gate = LocalRateGate::new(2, 1000); // 2 per 1000ms
        let t0 = 1_000_000u64;
        assert!(gate.try_acquire_at(t0));
        assert!(gate.try_acquire_at(t0));
        assert!(!gate.try_acquire_at(t0));
        // after a full interval, fully refilled
        assert!(gate.try_acquire_at(t0 + 1000));
        assert!(gate.try_acquire_at(t0 + 1000));
        assert!(!gate.try_acquire_at(t0 + 1000));
    }

    #[test]
    fn update_params_changes_rate_without_losing_registry_entry() {
        let port = 65000u16;
        let scope = "default/esp-a/#0:";
        let g1 = get_or_update_local_gate(scope, port, 1, 1000);
        let t0 = 2_000_000u64;
        assert!(g1.try_acquire_at(t0));
        assert!(!g1.try_acquire_at(t0));
        // hot reload raises the rate; same registry slot, new capacity applies
        let g2 = get_or_update_local_gate(scope, port, 5, 1000);
        assert!(Arc::ptr_eq(&g1, &g2));
        assert!(g2.try_acquire_at(t0)); // capacity raised to 5, more tokens available
    }

    #[test]
    fn gates_are_isolated_per_entry_scope_on_one_port() {
        // Two ConnectionRateLimit entries can be active on one listener port
        // (two TLSRoutes with different SNIs, or a listener-level plus a
        // route-level attachment). They must not share a bucket, and must not
        // rewrite each other's rate through update_params.
        let port = 65001u16;
        let a = get_or_update_local_gate("default/esp-a/#0:", port, 1, 1000);
        let b = get_or_update_local_gate("default/esp-b/#0:", port, 9, 1000);
        assert!(!Arc::ptr_eq(&a, &b));

        let t0 = 3_000_000u64;
        assert!(a.try_acquire_at(t0));
        assert!(!a.try_acquire_at(t0), "entry A must keep its own rate of 1");
        // B's budget is untouched by A having exhausted its own.
        assert!(b.try_acquire_at(t0));
        assert!(b.try_acquire_at(t0));
    }

    #[test]
    fn one_scope_keeps_separate_gates_per_port() {
        let scope = "default/esp-c/#0:";
        let p1 = get_or_update_local_gate(scope, 65002, 1, 1000);
        let p2 = get_or_update_local_gate(scope, 65003, 1, 1000);
        assert!(!Arc::ptr_eq(&p1, &p2));
    }
}
