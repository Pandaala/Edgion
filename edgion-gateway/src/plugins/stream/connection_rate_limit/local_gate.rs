//! Persistent local connection budgets, identified by plugin entry and listener port.
//!
//! Construction binds an entry once. Admission never takes the process registry
//! lock: it resolves a port under that entry's read lock, then performs one
//! coherent refill/generation/consumption transition under the bucket mutex.
//! No lock spans Redis work. State survives reload and is never evicted.

use edgion_resources::redis_rate_limit::validate_rate_limit_policy;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalGateParams {
    rate: u64,
    interval_ms: u64,
}

impl LocalGateParams {
    fn capacity(self) -> u128 {
        u128::from(self.rate) * 1000
    }
}

struct GateState {
    active_generation: u64,
    params: LocalGateParams,
    milli_tokens: u128,
    last_refill_ms: u64,
}

impl GateState {
    fn new(generation: u64, params: LocalGateParams, now_ms: u64) -> Self {
        Self {
            active_generation: generation,
            params,
            milli_tokens: params.capacity(),
            last_refill_ms: now_ms,
        }
    }

    fn refill(&mut self, now_ms: u64) {
        let elapsed = now_ms - self.last_refill_ms;
        if elapsed == 0 {
            return;
        }
        let interval = self.params.interval_ms;
        if elapsed >= interval {
            self.milli_tokens = self.params.capacity();
            self.last_refill_ms = now_ms;
            return;
        }

        // Two u64 factors fit u128. Dividing before scaling avoids a third
        // overflowing factor: quotient < rate, remainder < interval.
        let product = u128::from(elapsed) * u128::from(self.params.rate);
        let interval = u128::from(interval);
        let added = (product / interval) * 1000 + ((product % interval) * 1000) / interval;
        if added != 0 {
            self.milli_tokens = (self.milli_tokens + added).min(self.params.capacity());
            self.last_refill_ms = now_ms;
        }
    }

    fn admit(&mut self, generation: u64, params: LocalGateParams, sample_ms: u64) -> bool {
        let now_ms = sample_ms.max(self.last_refill_ms);
        self.refill(now_ms);
        if generation > self.active_generation {
            if params != self.params {
                let old_capacity = self.params.capacity();
                let new_capacity = params.capacity();
                self.milli_tokens = if new_capacity > old_capacity {
                    self.milli_tokens + (new_capacity - old_capacity)
                } else {
                    self.milli_tokens.min(new_capacity)
                };
                self.params = params;
                // Old elapsed time was settled at the old rate. Start the new
                // rate here, discarding at most an uncredited milli-token fraction.
                self.last_refill_ms = now_ms;
            }
            self.active_generation = generation;
        }
        if self.milli_tokens < 1000 {
            return false;
        }
        self.milli_tokens -= 1000;
        true
    }
}

struct LocalRateGate {
    origin: Instant,
    // Port-map insertion is not activation. The first locked admission chooses
    // the initial generation, even if a different generation created the slot.
    state: Mutex<Option<GateState>>,
}

impl LocalRateGate {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            state: Mutex::new(None),
        }
    }

    fn try_acquire(&self, generation: u64, params: LocalGateParams) -> bool {
        self.try_acquire_with_clock(generation, params, || {
            let elapsed = Instant::now().saturating_duration_since(self.origin);
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
    }

    // Statically dispatched: production samples Instant inside the state lock;
    // tests supply deterministic samples/interleavings without a runtime clock object.
    fn try_acquire_with_clock(&self, generation: u64, params: LocalGateParams, clock: impl FnOnce() -> u64) -> bool {
        let mut state = self.state.lock();
        let now_ms = clock();
        state
            .get_or_insert_with(|| GateState::new(generation, params, now_ms))
            .admit(generation, params, now_ms)
    }
}

#[derive(Default)]
struct EntryGates {
    next_generation: AtomicU64,
    ports: RwLock<HashMap<u16, Arc<LocalRateGate>>>,
}

impl EntryGates {
    fn gate(&self, port: u16) -> Arc<LocalRateGate> {
        {
            let ports = self.ports.read();
            if let Some(gate) = ports.get(&port) {
                return Arc::clone(gate);
            }
        }
        let mut ports = self.ports.write();
        Arc::clone(ports.entry(port).or_insert_with(|| Arc::new(LocalRateGate::new())))
    }
}

/// The existing no-eviction budget registry, partitioned by entry for admission.
/// This is not a cache: eviction would recreate full budgets. Each sparse port
/// map is bounded by u16, but retained entry scopes still grow with config churn.
#[derive(Default)]
struct LocalGateRegistry {
    entries: Mutex<HashMap<String, Arc<EntryGates>>>,
}

impl LocalGateRegistry {
    fn bind(&self, scope: &str, rate: u64, interval_ms: u64) -> Option<LocalGateBinding> {
        if validate_rate_limit_policy(rate, Duration::from_millis(interval_ms)).is_err() {
            return None;
        }
        self.bind_validated(scope, rate, interval_ms)
    }

    fn bind_validated(&self, scope: &str, rate: u64, interval_ms: u64) -> Option<LocalGateBinding> {
        let entry = {
            let mut entries = self.entries.lock();
            Arc::clone(entries.entry(scope.to_owned()).or_default())
        };
        let previous = entry
            .next_generation
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
            .ok()?;
        Some(LocalGateBinding {
            entry,
            generation: previous + 1,
            params: LocalGateParams { rate, interval_ms },
        })
    }
}

static LOCAL_GATES: LazyLock<LocalGateRegistry> = LazyLock::new(LocalGateRegistry::default);

/// One immutable runtime generation bound to a persistent entry budget.
/// Generations follow the production serial conf-sync construction order.
/// Only a call reaching a port can activate this generation there; stale callers
/// consume the latest activated local budget, retaining their own Redis config.
pub(super) struct LocalGateBinding {
    entry: Arc<EntryGates>,
    generation: u64,
    params: LocalGateParams,
}

impl LocalGateBinding {
    pub(super) fn new(scope: &str, rate: u64, interval_ms: u64) -> Option<Self> {
        LOCAL_GATES.bind(scope, rate, interval_ms)
    }

    pub(super) fn try_acquire(&self, port: u16) -> bool {
        self.entry.gate(port).try_acquire(self.generation, self.params)
    }

    #[cfg(test)]
    pub(super) fn exhaust_generations_for_test(&self) {
        self.entry.next_generation.store(u64::MAX, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/connection_rate_limit/local_gate_test.rs"]
mod tests;
