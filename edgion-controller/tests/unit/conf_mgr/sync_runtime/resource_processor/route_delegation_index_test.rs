use super::*;

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_named_registration_and_lookup() {
    let idx = RouteDelegationIndex::new();
    idx.update_parent("infra/parent", set(&["billing/child"]), HashSet::new());

    assert_eq!(idx.parents_of("billing/child", "billing"), vec!["infra/parent"]);
    assert!(idx.parents_of("billing/other", "billing").is_empty());
}

#[test]
fn test_wildcard_registration_and_lookup() {
    let idx = RouteDelegationIndex::new();
    idx.update_parent("infra/parent", HashSet::new(), set(&["billing"]));

    // Any route in the wildcard namespace resolves to the parent
    assert_eq!(idx.parents_of("billing/any-route", "billing"), vec!["infra/parent"]);
    // Routes in other namespaces do not
    assert!(idx.parents_of("payments/any-route", "payments").is_empty());
}

#[test]
fn test_named_and_wildcard_union_dedup() {
    let idx = RouteDelegationIndex::new();
    // Same parent declares both a named edge and a wildcard edge covering the child
    idx.update_parent("infra/parent", set(&["billing/child"]), set(&["billing"]));
    idx.update_parent("infra/other-parent", HashSet::new(), set(&["billing"]));

    let parents = idx.parents_of("billing/child", "billing");
    assert_eq!(parents, vec!["infra/other-parent", "infra/parent"]);
}

#[test]
fn test_update_replaces_edges() {
    let idx = RouteDelegationIndex::new();
    idx.update_parent("infra/parent", set(&["billing/child"]), set(&["tenant-a"]));
    assert_eq!(idx.parents_of("billing/child", "billing"), vec!["infra/parent"]);
    assert_eq!(idx.parents_of("tenant-a/x", "tenant-a"), vec!["infra/parent"]);

    // Re-declare with different edges: old ones must disappear
    idx.update_parent("infra/parent", set(&["payments/child2"]), HashSet::new());
    assert!(idx.parents_of("billing/child", "billing").is_empty());
    assert!(idx.parents_of("tenant-a/x", "tenant-a").is_empty());
    assert_eq!(idx.parents_of("payments/child2", "payments"), vec!["infra/parent"]);
}

#[test]
fn test_update_with_empty_sets_clears() {
    let idx = RouteDelegationIndex::new();
    idx.update_parent("infra/parent", set(&["billing/child"]), set(&["billing"]));
    idx.update_parent("infra/parent", HashSet::new(), HashSet::new());
    assert!(idx.parents_of("billing/child", "billing").is_empty());
}

#[test]
fn test_remove_parent() {
    let idx = RouteDelegationIndex::new();
    idx.update_parent("infra/parent", set(&["billing/child"]), set(&["billing"]));
    idx.remove_parent("infra/parent");
    assert!(idx.parents_of("billing/child", "billing").is_empty());
    // Removing again is a no-op
    idx.remove_parent("infra/parent");
}

#[test]
fn test_self_exclusion() {
    let idx = RouteDelegationIndex::new();
    // A parent wildcard-delegates to its own namespace: it must not list
    // itself as its own delegating parent (would requeue itself forever).
    idx.update_parent("infra/parent", HashSet::new(), set(&["infra"]));
    assert!(idx.parents_of("infra/parent", "infra").is_empty());
    // Sibling routes in the namespace still resolve to it
    assert_eq!(idx.parents_of("infra/sibling", "infra"), vec!["infra/parent"]);
}

#[test]
fn test_update_parent_returns_edge_delta() {
    let idx = RouteDelegationIndex::new();
    // First declaration: everything is "changed"
    let d1 = idx.update_parent("infra/parent", set(&["billing/a"]), set(&["tenant-a"]));
    assert_eq!(d1.changed_named, vec!["billing/a"]);
    assert_eq!(d1.changed_namespaces, vec!["tenant-a"]);

    // Same edges again: empty delta
    let d2 = idx.update_parent("infra/parent", set(&["billing/a"]), set(&["tenant-a"]));
    assert!(d2.is_empty());

    // Swap the named child, drop the wildcard: both differences reported
    let d3 = idx.update_parent("infra/parent", set(&["billing/b"]), HashSet::new());
    assert_eq!(d3.changed_named, vec!["billing/a", "billing/b"]);
    assert_eq!(d3.changed_namespaces, vec!["tenant-a"]);
}

#[test]
fn test_clear() {
    let idx = RouteDelegationIndex::new();
    idx.update_parent("infra/parent", set(&["billing/child"]), set(&["billing"]));
    idx.clear();
    assert!(idx.parents_of("billing/child", "billing").is_empty());
}
