use super::*;

#[test]
fn empty_store_reports_zero() {
    let store = SecretStore::new();
    assert_eq!(store.count(), 0);
    assert!(store.keys().is_empty());
}
