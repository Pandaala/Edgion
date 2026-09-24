use super::BodyScanBudget;

#[test]
fn exact_reservation_charges_only_fetched_bytes_after_refund() {
    let mut state = BodyScanBudget::default();

    state.reserve_exact(5, 16, 1).unwrap();
    state.refund_exact(2);
    state.reserve_exact(13, 16, 1).unwrap();

    assert!(state.reserve_exact(1, 16, 1).is_err());
}

#[test]
fn exact_reservation_rejects_before_overdrawing_budget() {
    let mut state = BodyScanBudget::default();

    assert!(state.reserve_exact(17, 16, 1).is_err());
    state.reserve_exact(0, 16, 1).unwrap();
    state.reserve_exact(16, 16, 1).unwrap();
}

#[test]
fn scans_and_uploads_share_remaining_bytes_without_reinitializing() {
    let mut budget = BodyScanBudget::default();
    budget.reserve_exact(5, 16, 1).unwrap();
    budget.refund_exact(2);
    let reserved = budget.reserve(128, 4).unwrap();
    assert_eq!(reserved, 13);
    budget.settle(reserved, 4);
    budget.reserve_exact(9, 128, 4).unwrap();
    assert!(budget.reserve_exact(1, 128, 4).is_err());
}

#[test]
fn failed_or_unsettled_scan_does_not_refund_its_reservation() {
    let mut budget = BodyScanBudget::default();
    assert_eq!(budget.reserve(16, 2).unwrap(), 32);
    assert_eq!(budget.reserve(16, 2).unwrap(), 0);
    assert!(budget.reserve_exact(1, 16, 2).is_err());
}

#[test]
fn budget_initialization_overflow_does_not_create_credit() {
    let mut budget = BodyScanBudget::default();
    assert!(budget.reserve(u64::MAX, 2).is_err());
    assert!(budget.reserve_exact(0, u64::MAX, 2).is_err());
    assert_eq!(budget.reserve(16, 1).unwrap(), 16);
}
