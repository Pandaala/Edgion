use super::*;

#[test]
fn instances_and_generations_share_global_task_and_body_limits() {
    let global = MirrorBudget::new(1, 8).unwrap();
    let first = global.body_budget(8);
    let second = global.body_budget(8);
    let task = global.try_acquire_task().unwrap();
    assert!(global.try_acquire_task().is_none());
    let data = first.try_copy(b"12345678").unwrap();
    assert!(second.try_copy(b"x").is_none());
    drop(task);
    assert!(global.try_acquire_task().is_some());
    drop(data);
    assert!(second.try_copy(b"12345678").is_some());
}

#[test]
fn last_transport_clone_owns_the_full_allocation_charge() {
    let budget = MirrorBodyBudget::for_test(8);
    let data = budget.try_copy(b"12345678").unwrap();
    let slice = data.slice(..1);
    drop(data);
    assert!(budget.try_copy(b"x").is_none());
    drop(slice);
    assert!(budget.try_copy(b"12345678").is_some());
}

#[test]
fn failed_local_admission_returns_global_permits() {
    let global = MirrorBudget::new(1, 8).unwrap();
    let small = global.body_budget(1);
    assert!(small.try_copy(b"12").is_none());
    assert_eq!(global.bytes.available_permits(), 8);
    assert!(small.try_copy(b"1").is_some());
}

#[test]
fn unsafe_direct_process_capacities_return_errors() {
    assert!(MirrorBudget::new(0, 1).is_err());
    assert!(MirrorBudget::new(1, 0).is_err());
    assert!(MirrorBudget::new(usize::MAX, 1).is_err());
    assert!(MirrorBudget::new(1, usize::MAX).is_err());
}
