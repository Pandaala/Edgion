use super::*;

#[test]
fn empty_store_reports_zero() {
    let store = HealthStatusStore::new();
    assert_eq!(store.tracked_service_count(), 0);
    assert!(store.keys().is_empty());
}
