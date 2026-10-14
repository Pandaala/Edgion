use super::*;
#[test]
fn atomic_classic_matches_upstream_first_match_and_finite_sums() {
    for bounds in [vec![1., 10.], vec![10., 1.], vec![1., 1., 10.], vec![f64::NAN, 10., 1.]] {
        let actual = AtomicHistogram::new(bounds.clone());
        let mut expected = metrics_util::storage::Histogram::new(&bounds).unwrap();
        let mut seed = 17u64;
        for _ in 0..10_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let sample = (seed >> 32) as i32 as f64 / 1000000.;
            actual.record(sample);
            expected.record_many(std::iter::once(&sample));
        }
        let result = actual.snapshot();
        for ((actual_bound, actual_count), (expected_bound, expected_count)) in
            result.buckets.iter().zip(expected.buckets())
        {
            assert_eq!(actual_bound.to_bits(), expected_bound.to_bits());
            assert_eq!(*actual_count, expected_count);
        }
        assert_eq!(result.count, expected.count());
        assert!((result.sum - expected.sum()).abs() < 1e-8 * expected.sum().abs().max(1.));
    }
}
#[test]
fn empty_store_reports_zero() {
    let store = DynamicStore::new("test", 1, &[]);
    let stats = store.stats();
    assert_eq!(stats.admission_count, 0);
    assert_eq!(stats.live_epochs, 0);
    assert_eq!(stats.overdue_epochs, 0);
    assert_eq!(stats.gauge_owners, 0);
}

#[test]
fn classic_nonfinite_samples_and_bounds_preserve_upstream() {
    for bounds in [vec![f64::NEG_INFINITY, 1., f64::INFINITY], vec![f64::NAN, 10., 1.]] {
        let actual = AtomicHistogram::new(bounds.clone());
        let mut expected = metrics_util::storage::Histogram::new(&bounds).unwrap();
        let samples = [f64::NEG_INFINITY, -1., 0., 1., f64::INFINITY, f64::NAN];
        for value in samples {
            actual.record(value);
            expected.record_many(std::iter::once(&value));
        }
        let result = actual.snapshot();
        for ((bound, count), (expected_bound, expected_count)) in result.buckets.iter().zip(expected.buckets()) {
            assert_eq!(bound.to_bits(), expected_bound.to_bits());
            assert_eq!(*count, expected_count);
        }
        assert_eq!(result.count, expected.count());
        assert!(result.sum.is_nan());
        assert!(expected.sum().is_nan());
    }
}
fn counter_store() -> (Arc<DynamicStore>, Arc<Slot>, Arc<AtomicU64>) {
    let guard = crate::observe::cardinality_guard::CardinalityGuard::new(
        metrics_util::debugging::DebuggingRecorder::new(),
        100,
        crate::observe::cardinality_guard::CardinalityMode::Enforce,
    );
    let clock = Arc::new(AtomicU64::new(0));
    let mut store = DynamicStore::new("test", 1, &[]);
    store.set_clock(Some(clock.clone()));
    store.attach(guard.admission.clone());
    let store = Arc::new(store);
    let key = Key::from_parts(
        "edgion_lb_degrade_selections_total",
        vec![metrics::Label::new("service", "race")],
    );
    let slot = store.resolve(&key).unwrap();
    (store, slot, clock)
}
fn increment(store: &DynamicStore, slot: &Slot) {
    store.mutate(slot, |value| {
        if let Value::Counter(counter) = value {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    });
}
#[test]
fn update_wins_expiry_rechecks_and_captures_timestamp_inside_gate() {
    let (store, slot, clock) = counter_store();
    increment(&store, &slot);
    clock.store(2_000_000_000, Ordering::Relaxed);
    let entered = Arc::new(std::sync::Barrier::new(2));
    let resume = Arc::new(std::sync::Barrier::new(2));
    let s = store.clone();
    let key = slot.clone();
    let e = entered.clone();
    let r = resume.clone();
    let operation = std::thread::spawn(move || {
        s.mutate(&key, |value| {
            e.wait();
            r.wait();
            if let Value::Counter(counter) = value {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        })
    });
    entered.wait();
    let expiry_checked = Arc::new(std::sync::Barrier::new(2));
    let s = store.clone();
    let key = slot.clone();
    let checked = expiry_checked.clone();
    let eviction = std::thread::spawn(move || {
        assert!(s.expiry_candidate(&key, 2_000_000_000));
        checked.wait();
        s.retire_expired(&key, 2_000_000_000);
    });
    expiry_checked.wait();
    resume.wait();
    operation.join().unwrap();
    eviction.join().unwrap();
    assert!(matches!(store.snapshot(&slot, store.now()), Some(Snapshot::Counter(2))));
}
#[test]
fn expiry_wins_and_concurrent_first_renewal_has_one_epoch() {
    let (store, slot, clock) = counter_store();
    increment(&store, &slot);
    clock.store(2_000_000_000, Ordering::Relaxed);
    store.expire(&slot, store.now());
    let start = Arc::new(std::sync::Barrier::new(9));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let s = store.clone();
        let key = slot.clone();
        let b = start.clone();
        workers.push(std::thread::spawn(move || {
            b.wait();
            increment(&s, &key);
        }));
    }
    start.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    assert!(matches!(store.snapshot(&slot, store.now()), Some(Snapshot::Counter(8))));
    assert_eq!(store.stats().admission_count, 3);
}

impl DynamicStore {
    pub(crate) fn test_cold_permission(&self, key: &Key) -> bool {
        self.admission.get().unwrap().test_cold_permission(key)
    }
    pub(crate) fn test_commit_prior_permission(&self, key: &Key, permitted: bool) {
        self.admission
            .get()
            .unwrap()
            .test_commit_prior_permission(key, permitted);
    }
}
