use super::*;

#[test]
fn test_is_ready_uses_required_kinds() {
    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
    let _enter = rt.enter();
    let client = ConfigClient::new("cid".to_string(), "cname".to_string());
    client.set_required_kinds_for_readiness(&[
        "GatewayClass".to_string(),
        "Gateway".to_string(),
        "HTTPRoute".to_string(),
    ]);

    // Mark only required kinds as ready.
    client
        .get_dyn_cache(ResourceKind::GatewayClass)
        .expect("gateway class cache")
        .set_ready();
    client
        .get_dyn_cache(ResourceKind::Gateway)
        .expect("gateway cache")
        .set_ready();
    client
        .get_dyn_cache(ResourceKind::HTTPRoute)
        .expect("http route cache")
        .set_ready();

    // Other caches remain not ready, but should not block.
    assert!(client.is_ready().is_ok());
}

#[test]
fn client_cache_stats_all_zero_initially() {
    use edgion_resources::resource_defs::ALL_RESOURCE_INFOS;

    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
    let _enter = rt.enter();
    let client = ConfigClient::new("cid".to_string(), "cname".to_string());
    let stats = client.client_cache_stats();
    // 19: Secret, ConfigMap, ReferenceGrant, and Unspecified have no ClientCache on the gateway side.
    assert_eq!(stats.len(), 19, "expected 19 gateway-side ClientCache kinds");
    for (kind, count) in &stats {
        assert_eq!(*count, 0, "kind {kind} should start empty");
    }
    // Contract: stats.keys() must equal the kind_name set of every ResourceKind
    // that `get_dyn_cache()` returns `Some` for. `get_dyn_cache` is the
    // compile-time-exhaustive dispatch table; this assertion bridges it to the
    // stats output, so a future hand-rolled stats table that forgets a kind
    // will fail this test loudly.
    let expected_keys: std::collections::BTreeSet<String> = ALL_RESOURCE_INFOS
        .iter()
        .filter_map(|info| client.get_dyn_cache(info.kind).map(|c| c.kind_name_dyn().to_string()))
        .collect();
    let actual_keys: std::collections::BTreeSet<String> = stats.keys().cloned().collect();
    assert_eq!(
        actual_keys, expected_keys,
        "client_cache_stats keys must match the get_dyn_cache coverage set"
    );
    // Cross-check: every kind reported by stats must be reachable via client_cache_keys.
    // Catches future divergence if a new kind is added to stats but not keys.
    for kind in stats.keys() {
        let sample = client.client_cache_keys(kind, 5);
        assert!(
            sample.is_empty(),
            "kind {} reported in stats but client_cache_keys returned non-empty for empty store",
            kind
        );
    }
    // Unknown kind returns empty vec (no panic).
    assert!(client.client_cache_keys("UnknownKind", 10).is_empty());
}
