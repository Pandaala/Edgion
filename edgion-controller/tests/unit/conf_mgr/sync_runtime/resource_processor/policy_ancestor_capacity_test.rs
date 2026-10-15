use super::*;
fn entry(owner: &str, name: &str) -> Value {
    json!({"controllerName":owner,"ancestorRef":{"name":name,"namespace":"team"},"conditions":[]})
}
#[test]
fn policy_ancestor_capacity_counts_protected_history_and_reserves_existing_keys() {
    for protected_count in [15, 16] {
        let protected: Vec<_> = (0..protected_count)
            .map(|i| entry("foreign", &format!("gateway-{i}")))
            .collect();
        let observed = json!({"ancestors":protected});
        let out = compose(
            "team",
            "owner",
            &observed,
            &json!({"ancestors":[entry("owner","local")]}),
            |_| true,
        );
        assert_eq!(out.persisted["ancestors"].as_array().unwrap().len(), 16);
        assert_eq!(out.local["ancestors"].as_array().unwrap().len(), 16 - protected_count);
        assert_eq!(
            &out.persisted["ancestors"].as_array().unwrap()[..protected_count],
            protected.as_slice()
        );
    }
    let mut observed: Vec<_> = (0..15).map(|i| entry("foreign", &format!("gateway-{i}"))).collect();
    observed.push(entry("owner", "z-existing"));
    let out = compose(
        "team",
        "owner",
        &json!({"ancestors":observed}),
        &json!({"ancestors":[entry("owner","a-new"),entry("owner","z-existing")]}),
        |_| true,
    );
    assert_eq!(out.local["ancestors"][0]["ancestorRef"]["name"], "z-existing");
}
#[test]
fn policy_ancestor_capacity_preserves_unknown_owner_and_canonical_existing_key() {
    let unknown = entry("owner", "unknown");
    let mut observed: Vec<_> = (0..15).map(|i| entry("foreign", &format!("gateway-{i}"))).collect();
    observed.push(unknown.clone());
    let out = compose(
        "team",
        "owner",
        &json!({"ancestors":observed}),
        &json!({"ancestors":[entry("owner","new")]}),
        |parent| parent.name != "unknown",
    );
    assert!(out.local["ancestors"].as_array().unwrap().is_empty());
    assert_eq!(out.persisted["ancestors"][15], unknown);
    let mut existing = entry("owner", "existing");
    existing["ancestorRef"].as_object_mut().unwrap().remove("namespace");
    let out = compose(
        "team",
        "owner",
        &json!({"ancestors":[existing]}),
        &json!({"ancestors":[entry("owner","existing"),entry("owner","existing")]}),
        |_| true,
    );
    assert_eq!(out.local["ancestors"].as_array().unwrap().len(), 1);
}
#[test]
fn policy_ancestor_capacity_never_truncates_overfull_protected_input() {
    let protected: Vec<_> = (0..17).map(|i| entry("foreign", &format!("gateway-{i}"))).collect();
    let out = compose(
        "team",
        "owner",
        &json!({"ancestors":protected}),
        &json!({"ancestors":[entry("owner","new")]}),
        |_| true,
    );
    assert!(!out.persistable);
    assert_eq!(out.persisted["ancestors"].as_array().unwrap().len(), 17);
    assert!(out.local["ancestors"].as_array().unwrap().is_empty());
}
