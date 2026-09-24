use super::*;

#[test]
fn empty_store_reports_zero() {
    let store = HealthCheckConfigStore::new();
    assert_eq!(store.count(), 0);
    assert!(store.keys().is_empty());
}
