use super::*;
use crate::plugins::http::PluginStore;

#[test]
fn empty_keys_yield_empty() {
    let store = PluginStore::new();
    assert!(resolve_gateway_plugins(&store.scope(), &[]).is_empty());
}

#[test]
fn dangling_keys_dropped() {
    let store = PluginStore::new();
    // A key that does not exist in the empty store is dropped.
    let out = resolve_gateway_plugins(&store.scope(), &["nope/missing".to_string()]);
    assert!(out.is_empty());
}
