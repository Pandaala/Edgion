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
    let binding = bind(&registry, "a", 3, 1_000);
    for _ in 0..3 {
        assert!(binding.try_acquire(80));
    }
    assert!(!binding.try_acquire(80));
}

#[test]
fn binding_enforces_the_shared_numeric_domain() {
    let registry = LocalGateRegistry::default();
    assert!(registry.bind("max", 1_000_000, i32::MAX as u64).is_some());
    assert!(registry.bind("rate-over", 1_000_001, 1_000).is_none());
    assert!(registry.bind("window-over", 1, i32::MAX as u64 + 1).is_none());
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
    LOCAL_GATES.bind_validated(scope, 1_000_000_000, u64::MAX).unwrap()
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
                                    admitted += usize::from(std::hint::black_box(measurement_acquire(binding, port)));
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
