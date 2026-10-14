use super::*;
use crate::plugins::http::wasm::host_abi::HostState;
use wasmtime::{Engine, Instance, Module, ResourceLimiter, Store};

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
