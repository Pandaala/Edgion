//! Persistent local connection budgets, identified by plugin entry and listener port.
//!
//! Construction binds an entry once. Admission never takes the process registry
//! lock: it resolves a port under that entry's read lock, then performs one
//! coherent refill/generation/consumption transition under the bucket mutex.
//! No lock spans Redis work. State survives reload and is never evicted.

use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

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
        if rate == 0 || interval_ms == 0 {
            return None;
        }
        let entry = {
            let mut entries = self.entries.lock();
            Arc::clone(entries.entry(scope.to_owned()).or_default())
        };
        let previous = entry
            .next_generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
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
mod tests {
    use super::*;
    use std::sync::{mpsc, Barrier};
    use std::thread;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);

    fn bind(registry: &LocalGateRegistry, scope: &str, rate: u64, interval: u64) -> LocalGateBinding {
        registry.bind(scope, rate, interval).unwrap()
    }

    fn at(binding: &LocalGateBinding, port: u16, now: u64) -> bool {
        binding
            .entry
            .gate(port)
            .try_acquire_with_clock(binding.generation, binding.params, || now)
    }

    fn snapshot(binding: &LocalGateBinding, port: u16) -> (u64, LocalGateParams, u128, u64) {
        let gate = binding.entry.gate(port);
        let guard = gate.state.lock();
        let state = guard.as_ref().unwrap();
        (
            state.active_generation,
            state.params,
            state.milli_tokens,
            state.last_refill_ms,
        )
    }

    #[test]
    fn capacity_then_refill_with_zero_as_a_valid_timestamp() {
        let registry = LocalGateRegistry::default();
        let binding = bind(&registry, "a", 2, 1000);
        assert!(at(&binding, 80, 0));
        assert!(at(&binding, 80, 0));
        assert!(!at(&binding, 80, 0));
        assert!(!at(&binding, 80, 499));
        assert!(at(&binding, 80, 500));
        assert!(!at(&binding, 80, 500));
        assert!(at(&binding, 80, 1000));
        assert!(!at(&binding, 80, 1000));
        assert_eq!(snapshot(&binding, 80).2, 0);
    }

    #[test]
    fn production_clock_allows_exact_capacity_without_sleep() {
        let registry = LocalGateRegistry::default();
        let binding = bind(&registry, "a", 3, u64::MAX);
        for _ in 0..3 {
            assert!(binding.try_acquire(80));
        }
        assert!(!binding.try_acquire(80));
    }

    #[test]
    fn concurrent_fixed_time_admits_exactly_capacity() {
        let registry = LocalGateRegistry::default();
        let binding = bind(&registry, "a", 37, 1000);
        let start = Barrier::new(16);
        let admitted = thread::scope(|threads| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    threads.spawn(|| {
                        start.wait();
                        (0..8).filter(|_| at(&binding, 80, 0)).count()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).sum::<usize>()
        });
        assert_eq!(admitted, 37);
        assert_eq!(snapshot(&binding, 80).2, 0);
        assert!(!at(&binding, 80, 0));
    }

    #[test]
    fn refill_and_overlapping_decrement_have_one_serial_balance() {
        let registry = LocalGateRegistry::default();
        let binding = bind(&registry, "a", 2, 1000);
        assert!(at(&binding, 80, 0));
        let gate = binding.entry.gate(80);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        thread::scope(|threads| {
            let gate = &gate;
            let binding = &binding;
            let first = threads.spawn(move || {
                gate.try_acquire_with_clock(binding.generation, binding.params, || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(WAIT).unwrap();
                    500
                })
            });
            entered_rx.recv_timeout(WAIT).unwrap();
            let second = threads.spawn(move || {
                attempt_tx.send(()).unwrap();
                at(binding, 80, 500)
            });
            attempt_rx.recv_timeout(WAIT).unwrap();
            release_tx.send(()).unwrap();
            assert!(first.join().unwrap());
            assert!(second.join().unwrap());
        });
        assert_eq!(snapshot(&binding, 80).2, 0);
        assert!(!at(&binding, 80, 500));
    }

    #[test]
    fn concurrent_port_creation_returns_one_unactivated_bucket() {
        let registry = LocalGateRegistry::default();
        let binding = bind(&registry, "a", 1, 1000);
        let start = Barrier::new(16);
        let gates = thread::scope(|threads| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    threads.spawn(|| {
                        start.wait();
                        binding.entry.gate(80)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>()
        });
        assert!(gates.iter().all(|gate| Arc::ptr_eq(gate, &gates[0])));
        assert!(gates[0].state.lock().is_none());
        assert!(at(&binding, 80, 0));
        assert!(!at(&binding, 80, 0));
    }

    #[test]
    fn delayed_new_creator_does_not_activate_before_old_admission() {
        let registry = LocalGateRegistry::default();
        let old = bind(&registry, "a", 2, 1000);
        let new = bind(&registry, "a", 5, 1000);
        // Stop the newer operation after it publishes the port slot, before its
        // admission. The older operation is first to obtain the bucket mutex.
        let created = new.entry.gate(80);
        assert!(created.state.lock().is_none());
        assert!(at(&old, 80, 0));
        assert_eq!(snapshot(&old, 80), (old.generation, old.params, 1000, 0));
        assert!(created.try_acquire_with_clock(new.generation, new.params, || 0));
        assert_eq!(snapshot(&old, 80), (new.generation, new.params, 3000, 0));
    }

    #[test]
    fn concurrent_expansion_credits_the_delta_once() {
        let registry = LocalGateRegistry::default();
        let old = bind(&registry, "a", 1, 1000);
        assert!(at(&old, 80, 0));
        let new = bind(&registry, "a", 17, 1000);
        let start = Barrier::new(32);
        let admitted = thread::scope(|threads| {
            let handles: Vec<_> = (0..32)
                .map(|_| {
                    threads.spawn(|| {
                        start.wait();
                        at(&new, 80, 0)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .filter(|allowed| *allowed)
                .count()
        });
        assert_eq!(admitted, 16);
        for _ in 0..10 {
            assert!(!at(&old, 80, 0));
            assert!(!at(&new, 80, 0));
        }
        assert_eq!(snapshot(&old, 80), (new.generation, new.params, 0, 0));
    }

    #[test]
    fn shrink_and_interval_change_settle_old_rate_before_overlapping_old_call() {
        let registry = LocalGateRegistry::default();
        let old = bind(&registry, "a", 4, 1000);
        for _ in 0..4 {
            assert!(at(&old, 80, 0));
        }
        let new = bind(&registry, "a", 2, 2000);
        let gate = old.entry.gate(80);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        thread::scope(|threads| {
            let gate = &gate;
            let new = &new;
            let updating = threads.spawn(move || {
                gate.try_acquire_with_clock(new.generation, new.params, || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(WAIT).unwrap();
                    500
                })
            });
            entered_rx.recv_timeout(WAIT).unwrap();
            let stale = threads.spawn(|| at(&old, 80, 500));
            release_tx.send(()).unwrap();
            // At the old rate 500ms earns two tokens. Both calls consume them;
            // refilling elapsed time at the new rate would incorrectly deny.
            assert!(updating.join().unwrap());
            assert!(stale.join().unwrap());
        });
        assert_eq!(snapshot(&new, 80), (new.generation, new.params, 0, 500));
        assert!(!at(&old, 80, 1499));
        assert!(at(&old, 80, 1500));
    }

    #[test]
    fn shrinking_clamps_existing_balance_before_consumption() {
        let registry = LocalGateRegistry::default();
        let old = bind(&registry, "a", 10, 1000);
        assert!(at(&old, 80, 0));
        let new = bind(&registry, "a", 2, 1000);
        assert!(at(&new, 80, 0));
        assert_eq!(snapshot(&old, 80).2, 1000);
        assert!(at(&old, 80, 0));
        assert!(!at(&new, 80, 0));
    }

    #[test]
    fn unchanged_generation_parameters_preserve_uncredited_time() {
        let registry = LocalGateRegistry::default();
        let old = bind(&registry, "a", 1, 10_000);
        assert!(at(&old, 80, 0));
        let new = bind(&registry, "a", 1, 10_000);
        assert!(!at(&new, 80, 9));
        assert_eq!(snapshot(&new, 80), (new.generation, new.params, 0, 0));
        assert!(!at(&old, 80, 10));
        assert_eq!(snapshot(&new, 80).2, 1);
        assert_eq!(snapshot(&new, 80).3, 10);
    }

    #[test]
    fn real_parameter_change_does_not_reinterpret_old_fraction() {
        let registry = LocalGateRegistry::default();
        let old = bind(&registry, "a", 1, 10_000);
        assert!(at(&old, 80, 0));
        let new = bind(&registry, "a", 1, 1000);
        assert!(!at(&new, 80, 9));
        assert_eq!(snapshot(&new, 80), (new.generation, new.params, 0, 9));
        assert!(!at(&new, 80, 1008));
        assert!(at(&new, 80, 1009));
    }

    #[test]
    fn reversed_sample_during_update_never_rewinds_the_baseline() {
        let registry = LocalGateRegistry::default();
        let old = bind(&registry, "a", 1, 1000);
        assert!(at(&old, 80, 100));
        assert!(!at(&old, 80, 600));
        let new = bind(&registry, "a", 1, 2000);
        assert!(!at(&new, 80, 200));
        assert_eq!(snapshot(&new, 80), (new.generation, new.params, 500, 600));
        assert!(!at(&old, 80, 600));
        assert!(!at(&new, 80, 1599));
        // Preserve existing milli-token quantization: the preceding refill
        // discarded half a milli-token, so the next full milli-token takes 2ms.
        assert!(!at(&old, 80, 1600));
        assert!(at(&old, 80, 1601));
    }

    #[test]
    fn unused_and_skipped_generations_are_inert_per_port() {
        let registry = LocalGateRegistry::default();
        let old = bind(&registry, "a", 2, 1000);
        assert!(at(&old, 80, 0));
        assert!(at(&old, 81, 0));
        let skipped = bind(&registry, "a", 100, 1000);
        let latest = bind(&registry, "a", 3, 1000);
        assert_eq!(snapshot(&old, 80).0, old.generation);
        assert!(at(&latest, 80, 0));
        assert_eq!(snapshot(&latest, 80).2, 1000);
        assert!(at(&skipped, 80, 0));
        assert!(!at(&old, 80, 0));
        assert_eq!(snapshot(&old, 80).0, latest.generation);
        // Port 81 never encountered either new generation.
        assert_eq!(snapshot(&old, 81), (old.generation, old.params, 1000, 0));
        assert!(at(&old, 81, 0));
    }

    #[test]
    fn same_identity_reuses_state_after_all_bindings_are_dropped() {
        let registry = LocalGateRegistry::default();
        {
            let old = bind(&registry, "a", 1, 1000);
            assert!(at(&old, 80, 0));
        }
        let new = bind(&registry, "a", 1, 1000);
        assert!(!at(&new, 80, 0));
        assert!(at(&new, 80, 1000));
    }

    #[test]
    fn entry_and_port_budgets_are_independent() {
        let registry = LocalGateRegistry::default();
        let a = bind(&registry, "a", 1, 1000);
        let b = bind(&registry, "b", 9, 1000);
        assert!(at(&a, 80, 0));
        assert!(!at(&a, 80, 0));
        assert!(at(&a, 81, 0));
        for _ in 0..9 {
            assert!(at(&b, 80, 0));
        }
        assert!(!at(&b, 80, 0));
    }

    #[test]
    fn blocked_bucket_does_not_hold_port_map_or_global_registry() {
        let registry = LocalGateRegistry::default();
        let a = bind(&registry, "a", 1, 1000);
        let blocked = a.entry.gate(80);
        let guard = blocked.state.lock();
        let (tx, rx) = mpsc::channel();
        thread::scope(|threads| {
            let waiting = threads.spawn(|| at(&a, 80, 0));
            let independent = threads.spawn(|| {
                let b = bind(&registry, "b", 1, 1000);
                tx.send((at(&a, 81, 0), at(&b, 80, 0))).unwrap();
            });
            let result = rx.recv_timeout(WAIT);
            drop(guard);
            assert_eq!(result.unwrap(), (true, true));
            assert!(waiting.join().unwrap());
            independent.join().unwrap();
        });
    }

    #[test]
    fn full_u64_domain_refill_and_capacity_fit_the_local_representation() {
        let params = LocalGateParams {
            rate: u64::MAX,
            interval_ms: u64::MAX,
        };
        let mut state = GateState::new(1, params, 0);
        state.milli_tokens = 0;
        state.refill(u64::MAX - 1);
        assert_eq!(state.milli_tokens, u128::from(u64::MAX - 1) * 1000);
        state.refill(u64::MAX);
        assert_eq!(state.milli_tokens, params.capacity());

        let mut low = GateState::new(
            1,
            LocalGateParams {
                rate: 1,
                interval_ms: 1,
            },
            0,
        );
        low.milli_tokens = 0;
        assert!(low.admit(2, params, 0));
        assert_eq!(low.milli_tokens, params.capacity() - 2000);
        low.milli_tokens = 0;
        assert!(low.admit(2, params, u64::MAX));
        assert_eq!(low.milli_tokens, params.capacity() - 1000);
    }

    #[test]
    fn integer_refill_matches_direct_formula_on_representable_products() {
        for rate in [1u64, 7, 1001, 1_000_003] {
            for interval in [1u64, 999, 10_000] {
                for elapsed in [0, interval / 2, interval - 1, interval, u64::MAX] {
                    let params = LocalGateParams {
                        rate,
                        interval_ms: interval,
                    };
                    let mut state = GateState::new(1, params, 0);
                    state.milli_tokens = 0;
                    state.refill(elapsed);
                    let expected =
                        (u128::from(elapsed) * u128::from(rate) * 1000 / u128::from(interval)).min(params.capacity());
                    assert_eq!(state.milli_tokens, expected);
                }
            }
        }
    }

    #[test]
    fn generation_exhaustion_fails_reservation_without_changing_live_state() {
        let registry = LocalGateRegistry::default();
        let old = bind(&registry, "a", 1, 1000);
        assert!(at(&old, 80, 0));
        old.entry.next_generation.store(u64::MAX - 1, Ordering::Relaxed);
        let last = bind(&registry, "a", 2, 1000);
        assert_eq!(last.generation, u64::MAX);
        assert!(registry.bind("a", 100, 1000).is_none());
        assert_eq!(snapshot(&old, 80).0, old.generation);
        assert!(at(&last, 80, 0));
        assert!(!at(&old, 80, 0));
        assert_eq!(old.entry.next_generation.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn zero_parameters_do_not_register_an_entry() {
        let registry = LocalGateRegistry::default();
        assert!(registry.bind("a", 0, 1000).is_none());
        assert!(registry.bind("a", 1, 0).is_none());
        assert!(registry.entries.lock().is_empty());
    }

    // Keep workload constants identical when comparing revisions. The two helpers
    // adapt construction/admission only; timing excludes runtime construction.
    fn measurement_binding(scope: &str) -> LocalGateBinding {
        LocalGateBinding::new(scope, 1_000_000_000, u64::MAX).unwrap()
    }

    fn measurement_acquire(binding: &LocalGateBinding, port: u16) -> bool {
        binding.try_acquire(port)
    }

    #[test]
    #[ignore = "optimized contention measurement; run explicitly in isolation"]
    fn local_gate_contention_measurement() {
        use std::sync::Barrier;
        use std::thread;

        println!("mode,workers,round,operations,admitted,elapsed_ns,operations_per_second");
        for mode in ["one_bucket", "many_entries", "many_ports", "port_misses"] {
            for workers in [1usize, 2, 4, 8] {
                for round in 0..3 {
                    let bindings: Vec<_> = (0..workers)
                        .map(|worker| {
                            let entry = if mode == "many_entries" { worker } else { 0 };
                            measurement_binding(&format!("measurement/{mode}-{workers}-{round}-{entry}/#0"))
                        })
                        .collect();
                    let iterations = if mode == "port_misses" { 1000 } else { 100_000 };
                    if mode != "port_misses" {
                        for (worker, binding) in bindings.iter().enumerate() {
                            let port = if mode == "many_ports" {
                                8080 + worker as u16
                            } else {
                                8080
                            };
                            assert!(measurement_acquire(binding, port));
                        }
                    }
                    let start = Barrier::new(workers + 1);
                    let (admitted, elapsed) = thread::scope(|threads| {
                        let handles: Vec<_> = bindings
                            .iter()
                            .enumerate()
                            .map(|(worker, binding)| {
                                let start = &start;
                                threads.spawn(move || {
                                    start.wait();
                                    let mut admitted = 0;
                                    for iteration in 0..iterations {
                                        let port = match mode {
                                            "many_ports" => 8080 + worker as u16,
                                            "port_misses" => 10_000 + (worker * iterations + iteration) as u16,
                                            _ => 8080,
                                        };
                                        admitted +=
                                            usize::from(std::hint::black_box(measurement_acquire(binding, port)));
                                    }
                                    admitted
                                })
                            })
                            .collect();
                        let started = Instant::now();
                        start.wait();
                        let admitted: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
                        (admitted, Instant::now().saturating_duration_since(started))
                    });
                    let operations = workers * iterations;
                    assert_eq!(admitted, operations);
                    println!(
                        "{mode},{workers},{round},{operations},{admitted},{},{:.0}",
                        elapsed.as_nanos(),
                        operations as f64 / elapsed.as_secs_f64()
                    );
                }
            }
        }
    }
}
