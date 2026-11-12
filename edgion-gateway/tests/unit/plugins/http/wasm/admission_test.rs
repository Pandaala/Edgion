use super::*;
use crate::plugins::http::wasm::host_abi::HostState;
use wasmtime::{Engine, Instance, Module, ResourceLimiter, Store};

/// Process defaults for the lib test harness, which runs many independent
/// loads in one process: the production preparation bound (4, non-waiting)
/// would make them reject each other. Tests that exercise exhaustion read
/// `limits()` instead of assuming 4.
pub(super) fn default_process_limits() -> WasmLimits {
    WasmLimits {
        max_preparations: 32,
        ..WasmLimits::default()
    }
}

impl GuestMemory {
    fn for_test(budget: Arc<GuestBudget>) -> Arc<Self> {
        Arc::new(Self {
            budget,
            bytes: AtomicUsize::new(0),
        })
    }
}

#[test]
fn concurrent_reservations_never_over_admit_and_release_once() {
    let budget = GuestBudget::new(16).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(5));
    let mut threads = Vec::new();
    for _ in 0..4 {
        let charge = GuestMemory::for_test(budget.clone());
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let admitted = charge.reserve(8);
            barrier.wait();
            barrier.wait();
            admitted
        }));
    }
    barrier.wait();
    assert_eq!(budget.used.load(Ordering::Acquire), 16);
    barrier.wait();
    assert_eq!(
        threads.into_iter().map(|t| t.join().unwrap() as usize).sum::<usize>(),
        2
    );
    assert_eq!(budget.used.load(Ordering::Acquire), 0);
    assert!(GuestBudget::new(0).is_err());
    assert!(GuestBudget::new(usize::MAX).is_err());
}

#[test]
fn failed_growth_rolls_back_only_the_current_reservation() {
    let budget = GuestBudget::new(4 * 65536).unwrap();
    let charge = GuestMemory::for_test(budget.clone());
    let mut host = HostState::default();
    host.guest_memory = charge.clone();
    assert!(host.memory_growing(0, 65536, None).unwrap());
    assert!(host.memory_growing(65536, 2 * 65536, None).unwrap());
    host.memory_grow_failed(wasmtime::Error::msg("allocation failed"))
        .unwrap();
    assert_eq!(budget.used.load(Ordering::Acquire), 65536);
    assert!(!host.memory_growing(65536, 3 * 65536, Some(2 * 65536)).unwrap());
    host.memory_grow_failed(wasmtime::Error::msg("rejected")).unwrap();
    assert_eq!(budget.used.load(Ordering::Acquire), 65536);
    assert!(host.table_growing(0, 10, None).unwrap());
    host.table_grow_failed(wasmtime::Error::msg("table failed")).unwrap();
    assert_eq!(budget.used.load(Ordering::Acquire), 65536);
    drop(host);
    assert_eq!(
        budget.used.load(Ordering::Acquire),
        65536,
        "external destruction guard owns charge"
    );
    drop(charge);
    assert_eq!(budget.used.load(Ordering::Acquire), 0);
}

#[test]
fn real_guests_share_growth_budget_and_failed_initialization_releases() {
    let engine = Engine::default();
    let module = Module::new(
        &engine,
        wat::parse_str(
            r#"(module
        (memory (export "memory") 1 4)
        (table 2 funcref)
        (func (export "grow") (result i32) (memory.grow (i32.const 1))))"#,
        )
        .unwrap(),
    )
    .unwrap();
    let budget = GuestBudget::new(3 * 65536 + 4 * size_of::<usize>()).unwrap();
    let mut stores = Vec::new();
    for _ in 0..2 {
        let charge = GuestMemory::for_test(budget.clone());
        let mut host = HostState::default();
        host.guest_memory = charge.clone();
        let mut store = Store::new(&engine, host);
        store.limiter(|s| s);
        let instance = Instance::new(&mut store, &module, &[]).unwrap();
        stores.push((store, instance, charge));
    }
    let (store, instance, _) = &mut stores[0];
    let grow = instance.get_typed_func::<(), i32>(&mut *store, "grow").unwrap();
    assert_eq!(grow.call(&mut *store, ()).unwrap(), 1);
    let (store, instance, _) = &mut stores[1];
    let grow = instance.get_typed_func::<(), i32>(&mut *store, "grow").unwrap();
    assert_eq!(grow.call(&mut *store, ()).unwrap(), -1);
    assert_eq!(budget.used.load(Ordering::Acquire), budget.limit);
    drop(stores);
    assert_eq!(budget.used.load(Ordering::Acquire), 0);

    let bad = Module::new(
        &engine,
        wat::parse_str("(module (memory 1) (func $start unreachable) (start $start))").unwrap(),
    )
    .unwrap();
    let charge = GuestMemory::for_test(budget.clone());
    let mut host = HostState::default();
    host.guest_memory = charge.clone();
    let mut store = Store::new(&engine, host);
    store.limiter(|s| s);
    assert!(Instance::new(&mut store, &bad, &[]).is_err());
    drop(store);
    assert_eq!(budget.used.load(Ordering::Acquire), 65536);
    drop(charge);
    assert_eq!(budget.used.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn cancelled_blocking_work_retains_preparation_until_payload_drop() {
    let permits = Arc::new(Semaphore::new(1));
    let permit = Arc::new(permits.clone().try_acquire_owned().unwrap());
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (destroyed, gone) = tokio::sync::oneshot::channel();
    #[derive(Debug)]
    struct Payload {
        destroyed: Option<tokio::sync::oneshot::Sender<()>>,
        permits: Arc<Semaphore>,
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            assert_eq!(self.permits.available_permits(), 0, "result outlives admission");
            self.destroyed.take().unwrap().send(()).unwrap();
        }
    }
    let observed = permits.clone();
    let task = tokio::spawn(async move {
        prepare_blocking(&permit, move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            Ok(Payload {
                destroyed: Some(destroyed),
                permits: observed,
            })
        })
        .await
    });
    ready.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(permits.available_permits(), 0);
    release.send(()).unwrap();
    gone.await
        .expect("payload destructor must verify admission before signaling");
    // The payload signal precedes its following permit's drop; yield until the
    // blocking worker has completed that destructor sequence.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while permits.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(permits.available_permits(), 1);
}

#[test]
fn configurable_limits_reject_invalid_bounds() {
    let defaults = WasmLimits::default();
    defaults.validate().unwrap();
    for (field, values) in [
        ("guest_memory_bytes", vec![0, usize::MAX]),
        ("max_instances", vec![0, 1025]),
        ("max_preparations", vec![0, 33]),
        ("max_instance_memory_bytes", vec![0, MAX_GUEST_MEMORY_BYTES + 1]),
        ("max_instance_table_elements", vec![0, MAX_GUEST_TABLE_ELEMENTS + 1]),
        ("max_idle_instances", vec![defaults.max_instances + 1]),
        ("max_idle_guest_memory_bytes", vec![defaults.guest_memory_bytes + 1]),
        ("max_host_call_bytes", vec![0, MAX_GUEST_MEMORY_BYTES + 1]),
        ("max_host_abi_bytes", vec![0, MAX_RETAINED_ABI_BYTES + 1]),
        ("max_host_abi_ops", vec![0, MAX_RETAINED_ABI_OPS + 1]),
    ] {
        for value in values {
            let mut json = serde_json::to_value(defaults).unwrap();
            json[field] = value.into();
            let limits: WasmLimits = serde_json::from_value(json).unwrap();
            assert!(limits.validate().unwrap_err().contains(field), "{field}={value}");
            assert!(Admission::new(limits).is_err());
        }
    }
}

#[test]
fn idle_reservations_enforce_both_bounds_and_rollback_refused_slots() {
    for limits in [
        WasmLimits {
            max_idle_instances: 2,
            max_idle_guest_memory_bytes: 64,
            ..Default::default()
        },
        WasmLimits {
            max_idle_instances: 4,
            max_idle_guest_memory_bytes: 16,
            ..Default::default()
        },
    ] {
        let admission = Arc::new(Admission::new(limits).unwrap());
        assert!(admission.try_idle(65).is_none());
        assert_eq!(admission.stats().idle_instances, 0, "byte refusal returns slot");
        let barrier = Arc::new(std::sync::Barrier::new(5));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let admission = admission.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let reservation = admission.try_idle(8);
                    barrier.wait();
                    barrier.wait();
                    reservation.is_some()
                })
            })
            .collect();
        barrier.wait();
        assert_eq!(admission.stats().idle_instances, 2);
        assert_eq!(admission.stats().idle_guest_bytes, 16);
        barrier.wait();
        assert_eq!(
            workers
                .into_iter()
                .filter_map(|w| w.join().unwrap().then_some(()))
                .count(),
            2
        );
        assert_eq!(admission.stats().idle_instances, 0);
        assert_eq!(admission.stats().idle_guest_bytes, 0);
    }
    for limits in [
        WasmLimits {
            max_idle_instances: 0,
            ..Default::default()
        },
        WasmLimits {
            max_idle_guest_memory_bytes: 0,
            ..Default::default()
        },
    ] {
        let admission = Admission::new(limits).unwrap();
        assert!(admission.try_idle(0).is_none(), "zero limit disables even empty guests");
    }
}

#[test]
fn startup_publication_is_atomic_and_stats_do_not_initialize_defaults() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::wasm::admission::tests",
        "startup_publication_is_atomic_and_stats_do_not_initialize_defaults",
        "EDGION_WASM_LIMITS_STARTUP_CHILD",
        "bounded",
        || {
            assert!(ADMISSION.get().is_none());
            let empty = stats();
            assert_eq!(empty.instances, 0);
            assert_eq!(empty.preparations, 0);
            assert_eq!(empty.guest_bytes, 0);
            assert_eq!(empty.idle_instances, 0);
            assert_eq!(empty.idle_guest_bytes, 0);
            assert!(ADMISSION.get().is_none());
            assert!(initialize(WasmLimits {
                max_preparations: 0,
                ..Default::default()
            })
            .is_err());
            assert!(ADMISSION.get().is_none());
            let limits = WasmLimits {
                max_instances: 2,
                max_idle_instances: 1,
                max_preparations: 1,
                ..Default::default()
            };
            initialize(limits).unwrap();
            assert!(initialize(WasmLimits::default()).is_err());
            assert_eq!(stats().limits, limits);
            let prep = try_prepare().unwrap();
            assert!(try_prepare().is_err());
            assert_eq!(stats().preparations, 1);
            drop(prep);
            assert_eq!(stats().preparations, 0);
            let instances = instance_permits().clone().try_acquire_many_owned(2).unwrap();
            assert!(instance_permits().clone().try_acquire_owned().is_err());
            assert_eq!(stats().instances, 2);
            drop(instances);
            assert_eq!(stats().instances, 0);
        },
    );
}

#[test]
fn configured_guest_limits_reject_initial_allocation_and_growth() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::wasm::admission::tests",
        "configured_guest_limits_reject_initial_allocation_and_growth",
        "EDGION_WASM_LIMITS_GUEST_CHILD",
        "bounded",
        || {
            initialize(WasmLimits {
                max_instance_memory_bytes: 2 * 65536,
                max_instance_table_elements: 2,
                ..Default::default()
            })
            .unwrap();
            let engine = Engine::default();
            let module = Module::new(
                &engine,
                wat::parse_str(
                    r#"(module
                (memory 1)
                (table 1 funcref)
                (func (export "memory_grow") (result i32) (memory.grow (i32.const 1)))
                (func (export "table_grow") (result i32) (table.grow (ref.null func) (i32.const 1))))"#,
                )
                .unwrap(),
            )
            .unwrap();
            let mut store = Store::new(&engine, HostState::default());
            store.limiter(|s| s);
            let instance = Instance::new(&mut store, &module, &[]).unwrap();
            for name in ["memory_grow", "table_grow"] {
                let grow = instance.get_typed_func::<(), i32>(&mut store, name).unwrap();
                assert_eq!(grow.call(&mut store, ()).unwrap(), 1);
                assert_eq!(grow.call(&mut store, ()).unwrap(), -1);
            }
            assert_eq!(stats().guest_bytes, 2 * 65536 + 2 * size_of::<usize>());
            drop(store);
            assert_eq!(stats().guest_bytes, 0);
            for guest in ["(module (memory 3))", "(module (table 3 funcref))"] {
                let module = Module::new(&engine, wat::parse_str(guest).unwrap()).unwrap();
                let mut store = Store::new(&engine, HostState::default());
                store.limiter(|s| s);
                assert!(Instance::new(&mut store, &module, &[]).is_err());
                drop(store);
                assert_eq!(stats().guest_bytes, 0);
            }
        },
    );
}
