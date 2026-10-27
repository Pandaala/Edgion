use super::*;

#[test]
fn counters_refuse_without_waiting_and_reuse_exact_credits() {
    let budget = StorageBudget::new(8, 16, 1, 1);
    let lease = budget.capture().unwrap();
    assert!(matches!(budget.capture(), Err(BodyReadError::CapacityExceeded)));
    let body = MemoryBuffer::copy(b"12345678", Arc::clone(&lease)).unwrap();
    assert!(matches!(
        MemoryBuffer::new(1, Arc::clone(&lease)),
        Err(BodyReadError::CapacityExceeded)
    ));
    let slice = body.slice(0..1);
    drop(body);
    drop(lease);
    assert_eq!(budget.usage(), [8, 0, 1, 0]);
    drop(slice);
    assert_eq!(budget.usage(), [0; 4]);
    assert!(budget.capture().is_ok());
}

#[test]
fn replacement_capacity_is_reserved_while_previous_allocation_exists() {
    let budget = StorageBudget::new(7, 16, 1, 1);
    let lease = budget.capture().unwrap();
    let mut buffer = MemoryBuffer::new(4, lease).unwrap();
    buffer.append(b"abc");
    assert_eq!(buffer.grow(5).err(), Some(BodyReadError::CapacityExceeded));
    assert_eq!(buffer.as_slice(), b"abc");
    assert_eq!(budget.usage(), [4, 0, 1, 0]);
    drop(buffer);
    assert_eq!(budget.usage(), [0; 4]);
}

#[test]
fn concurrent_reservations_cannot_over_admit() {
    let budget = StorageBudget::new(4, 16, 8, 8);
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let mut threads = Vec::new();
    for _ in 0..8 {
        let budget = Arc::clone(&budget);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            let reservation = budget.memory.reserve(1);
            barrier.wait();
            assert!(budget.usage()[0] <= 4);
            barrier.wait();
            reservation.is_ok()
        }));
    }
    assert_eq!(
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|admitted| *admitted)
            .count(),
        4
    );
    assert_eq!(budget.usage(), [0; 4]);
}

#[test]
fn startup_configuration_rejects_zero_invalid_and_unknown_values() {
    assert!(RequestBodyBudgetConfig::default().resolve().is_ok());
    for value in ["0", "invalid", "18446744073709551616GiB"] {
        let config = RequestBodyBudgetConfig {
            memory_limit: value.into(),
            ..Default::default()
        };
        assert!(config.resolve().is_err());
    }
    let config = RequestBodyBudgetConfig {
        max_files: 0,
        ..Default::default()
    };
    assert!(config.resolve().is_err());
    assert!(serde_yaml::from_str::<RequestBodyBudgetConfig>("unknown: 1").is_err());
}

#[test]
fn process_publication_cannot_replace_an_existing_allowance() {
    let original = process_budget();
    assert!(initialize(&RequestBodyBudgetConfig::default()).is_err());
    assert!(Arc::ptr_eq(&original, &process_budget()));
}
