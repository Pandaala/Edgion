use super::*;

#[test]
fn empty_store_reports_zero() {
    let store = OutlierDetectionStore::new();
    assert_eq!(store.service_count(), 0);
    assert!(store.keys().is_empty());
}
