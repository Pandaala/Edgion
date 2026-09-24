use super::*;

#[test]
fn empty_store_reports_empty_breakdown() {
    let store = PluginStore::new();
    let by_kind = store.count_by_kind();
    assert!(by_kind.is_empty());
    let keys = store.keys_by_kind("JwtAuth");
    assert!(keys.is_empty());
}
