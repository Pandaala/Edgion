use super::*;

/// Number of distinct participant routes; pins the participants-only bound.
fn participant_count(graph: &DelegationGraph) -> usize {
    let g = graph.inner.read();
    let mut keys: HashSet<&String> = HashSet::new();
    keys.extend(g.targets_by_parent.keys());
    keys.extend(g.declared_by_child.keys());
    keys.extend(g.roots.iter());
    keys.len()
}

fn named(ns: &str, name: &str) -> DelegationTarget {
    DelegationTarget {
        ns: ns.to_string(),
        name: DelegationTargetName::Named(name.to_string()),
    }
}

fn wildcard(ns: &str) -> DelegationTarget {
    DelegationTarget {
        ns: ns.to_string(),
        name: DelegationTargetName::Wildcard,
    }
}

fn unmasked(targets: Vec<DelegationTarget>) -> Vec<(DelegationTarget, Vec<DelegatingRule>)> {
    targets
        .into_iter()
        .map(|target| (target, vec![DelegatingRule::unscoped(0)]))
        .collect()
}

fn masked(target: DelegationTarget, mask: u32) -> (DelegationTarget, Vec<DelegatingRule>) {
    (target, vec![DelegatingRule::unscoped(mask)])
}

fn masks(rules: Option<Vec<DelegatingRule>>) -> Option<Vec<u32>> {
    rules.map(|rules| rules.iter().map(|rule| rule.mask).collect())
}

fn graph() -> DelegationGraph {
    DelegationGraph::new(RouteKindName::HTTPRoute)
}

fn key(parent: &str, target: DelegationTarget) -> DelegationKey {
    let (ns, name) = parent.split_once('/').unwrap();
    key_for(RouteKindName::HTTPRoute, ns, name, &target)
}

#[test]
fn named_edge_requires_both_sides() {
    let g = graph();
    g.update_parent("t/p", unmasked(vec![named("t", "c")]));
    assert!(g.confirmed_keys("t/c").is_empty(), "child has not declared the parent");

    g.update_child("t/c", "t", vec!["t/p".to_string()], true);
    assert_eq!(g.confirmed_keys("t/c"), vec![key("t/p", named("t", "c"))]);

    // Child declares a parent that targets a different name: unconfirmed.
    g.update_parent("t/p", unmasked(vec![named("t", "other")]));
    assert!(g.confirmed_keys("t/c").is_empty());

    // Parent targets the right name in another namespace: unconfirmed.
    g.update_parent("t/p", unmasked(vec![named("x", "c")]));
    assert!(g.confirmed_keys("t/c").is_empty());
}

#[test]
fn wildcard_target_matches_only_declaring_children() {
    let g = graph();
    g.update_parent("t/p", unmasked(vec![wildcard("t")]));
    g.update_child("t/c1", "t", vec!["t/p".to_string()], true);
    g.update_child("t/c2", "t", vec!["t/other".to_string()], true);
    g.update_child("x/c3", "x", vec!["t/p".to_string()], true);

    assert_eq!(g.children_of_target("t/p", &wildcard("t")), vec!["t/c1".to_string()]);
    assert_eq!(g.confirmed_keys("t/c1"), vec![key("t/p", wildcard("t"))]);
    assert!(g.confirmed_keys("t/c2").is_empty());
    assert!(g.confirmed_keys("x/c3").is_empty());
}

#[test]
fn wildcard_target_does_not_scan_namespace() {
    let g = graph();
    g.update_parent("t/p", unmasked(vec![wildcard("t")]));
    g.update_child("t/c", "t", vec!["t/p".to_string()], true);
    // The API takes no namespace listing: unrelated routes never enter the graph.
    assert_eq!(participant_count(&g), 2);
    assert_eq!(g.children_of_target("t/p", &wildcard("t")), vec!["t/c".to_string()]);
}

/// A route takes the minimum level over its confirmed parents plus one:
/// `c` sits under `a` (level 2) and `b` (level 3), so it is level 3, not 4;
/// dropping the shallower edge moves it to 4.
#[test]
fn levels_follow_min_parent() {
    let g = graph();
    g.set_root("t/r", true);
    g.update_parent("t/r", unmasked(vec![named("t", "a"), named("t", "x")]));
    g.update_parent("t/x", unmasked(vec![named("t", "b")]));
    g.update_parent("t/a", unmasked(vec![named("t", "c")]));
    g.update_parent("t/b", unmasked(vec![named("t", "c")]));
    g.update_child("t/a", "t", vec!["t/r".to_string()], true);
    g.update_child("t/x", "t", vec!["t/r".to_string()], true);
    g.update_child("t/b", "t", vec!["t/x".to_string()], true);
    g.update_child("t/c", "t", vec!["t/b".to_string(), "t/a".to_string()], true);

    assert_eq!(
        g.recompute_level("t/r"),
        LevelChange {
            old: None,
            new: Some(1)
        }
    );
    for key in ["t/a", "t/x", "t/b"] {
        g.recompute_level(key);
    }
    assert_eq!(g.level("t/a"), Some(2));
    assert_eq!(g.level("t/b"), Some(3));
    assert_eq!(
        g.recompute_level("t/c"),
        LevelChange {
            old: None,
            new: Some(3)
        },
        "min(2, 3) + 1, not max + 1"
    );
    assert_eq!(g.recompute_level("t/c").old, Some(3));

    // Only the deeper parent remains: c follows it.
    g.update_parent("t/a", unmasked(vec![]));
    assert_eq!(
        g.recompute_level("t/c"),
        LevelChange {
            old: Some(3),
            new: Some(4)
        }
    );
}

#[test]
fn level_above_five_is_none() {
    let g = graph();
    g.set_root("t/n0", true);
    for i in 0..6 {
        let p = format!("t/n{i}");
        let c = format!("t/n{}", i + 1);
        g.update_parent(&p, unmasked(vec![named("t", &format!("n{}", i + 1))]));
        g.update_child(&c, "t", vec![p], true);
    }
    for i in 0..7 {
        g.recompute_level(&format!("t/n{i}"));
    }
    assert_eq!(g.level("t/n4"), Some(5));
    assert_eq!(g.level("t/n5"), None);
    assert_eq!(g.level("t/n6"), None);
}

/// A cycle that held levels through a root loses them once the root
/// detaches: the stale levels feed each other upward until the 5-level cap
/// cuts them off, so recomputation terminates at `None` rather than counting
/// forever. A cycle that never had a root starts and stays at `None`.
#[test]
fn cycle_without_root_terminates_at_none() {
    let g = graph();
    g.set_root("t/r", true);
    g.update_parent("t/r", unmasked(vec![named("t", "a")]));
    g.update_parent("t/a", unmasked(vec![named("t", "b")]));
    g.update_parent("t/b", unmasked(vec![named("t", "a")]));
    g.update_child("t/a", "t", vec!["t/r".to_string(), "t/b".to_string()], true);
    g.update_child("t/b", "t", vec!["t/a".to_string()], true);
    for key in ["t/r", "t/a", "t/b"] {
        g.recompute_level(key);
    }
    assert_eq!((g.level("t/a"), g.level("t/b")), (Some(2), Some(3)));

    g.set_root("t/r", false);
    assert_eq!(g.recompute_level("t/r").new, None);
    let mut iterations = 0;
    loop {
        iterations += 1;
        let a = g.recompute_level("t/a");
        let b = g.recompute_level("t/b");
        if a.old == a.new && b.old == b.new {
            break;
        }
        assert!(
            iterations <= MAX_ROUTE_DELEGATION_LEVELS,
            "level recompute must terminate"
        );
    }
    assert!(iterations > 1, "the stale levels had to be drained");
    assert_eq!(g.level("t/a"), None);
    assert_eq!(g.level("t/b"), None);

    // A rootless cycle from scratch never gains a level.
    let g = graph();
    g.update_parent("t/a", unmasked(vec![named("t", "b")]));
    g.update_parent("t/b", unmasked(vec![named("t", "a")]));
    g.update_child("t/a", "t", vec!["t/b".to_string()], true);
    g.update_child("t/b", "t", vec!["t/a".to_string()], true);
    for _ in 0..2 {
        assert_eq!(g.recompute_level("t/a").new, None);
        assert_eq!(g.recompute_level("t/b").new, None);
    }
}

#[test]
fn cycle_with_root_converges() {
    let g = graph();
    g.set_root("t/r", true);
    g.update_parent("t/r", unmasked(vec![named("t", "a")]));
    g.update_parent("t/a", unmasked(vec![named("t", "b")]));
    g.update_parent("t/b", unmasked(vec![named("t", "a")]));
    g.update_child("t/a", "t", vec!["t/r".to_string(), "t/b".to_string()], true);
    g.update_child("t/b", "t", vec!["t/a".to_string()], true);

    for _ in 0..6 {
        g.recompute_level("t/r");
        g.recompute_level("t/a");
        g.recompute_level("t/b");
    }
    assert_eq!(g.level("t/a"), Some(2));
    assert_eq!(g.level("t/b"), Some(3));
}

#[test]
fn ordinary_route_is_not_participant() {
    let g = graph();
    assert!(!g.is_participant("t/plain"));
    assert_eq!(participant_count(&g), 0);
    g.update_parent("t/p", unmasked(vec![named("t", "c")]));
    assert!(g.is_participant("t/p"));
    g.update_parent("t/p", unmasked(vec![]));
    assert!(!g.is_participant("t/p"));
    g.set_root("t/r", true);
    assert!(g.is_participant("t/r"));
    g.set_root("t/r", false);
    assert!(!g.is_participant("t/r"));
}

#[test]
fn remove_parent_returns_children_to_requeue() {
    let g = graph();
    g.update_parent("t/p", unmasked(vec![wildcard("t")]));
    g.update_child("t/c1", "t", vec!["t/p".to_string()], true);
    g.update_child("t/c2", "t", vec!["t/p".to_string()], true);
    let mut requeue = g.remove("t/p");
    requeue.sort();
    assert_eq!(requeue, vec!["t/c1".to_string(), "t/c2".to_string()]);
    assert!(g.confirmed_keys("t/c1").is_empty());
    assert!(!g.is_participant("t/p"));
}

#[test]
fn update_child_returns_only_parents_naming_it() {
    let g = graph();
    g.update_parent("t/p1", unmasked(vec![named("t", "c")]));
    g.update_parent("t/p2", unmasked(vec![wildcard("t")]));
    let first = g.update_child("t/c", "t", vec!["t/p1".to_string()], true);
    assert_eq!(first, vec!["t/p1".to_string()]);
    // p2 (wildcard) and p3 (no targets) never read the declaration.
    let second = g.update_child("t/c", "t", vec!["t/p2".to_string(), "t/p3".to_string()], true);
    assert_eq!(
        second,
        vec!["t/p1".to_string()],
        "the old naming parent loses the child"
    );
    let third = g.update_child("t/c", "t", vec!["t/p1".to_string(), "t/p2".to_string()], true);
    assert_eq!(third, vec!["t/p1".to_string()]);
    let fourth = g.update_child("t/c", "t", vec![], true);
    assert_eq!(fourth, vec!["t/p1".to_string()]);
    assert!(!g.is_participant("t/c"));
}

/// Security I4: a route in another namespace declaring a platform parent
/// (which delegates elsewhere, by wildcard or by another name) must not be
/// able to requeue that parent by churning its declaration.
#[test]
fn foreign_declaration_never_requeues_unrelated_parent() {
    let g = graph();
    g.update_parent("platform/p", unmasked(vec![wildcard("team"), named("team", "billing")]));
    for _ in 0..3 {
        assert!(g
            .update_child("evil/c", "evil", vec!["platform/p".to_string()], true)
            .is_empty());
        assert!(g.on_route_deleted("evil/c").is_empty());
    }
    // A same-namespace child reached only through the wildcard target.
    assert!(g
        .update_child("team/other", "team", vec!["platform/p".to_string()], true)
        .is_empty());
    assert!(g.on_route_deleted("team/other").is_empty());
}

#[test]
fn removing_named_child_requeues_naming_parent() {
    let g = graph();
    g.update_parent("platform/p", unmasked(vec![named("team", "billing")]));
    g.update_parent("platform/q", unmasked(vec![wildcard("team")]));
    g.update_child(
        "team/billing",
        "team",
        vec!["platform/p".to_string(), "platform/q".to_string()],
        true,
    );
    assert_eq!(g.remove("team/billing"), vec!["platform/p".to_string()]);
}

#[test]
fn update_parent_reports_target_delta() {
    let g = graph();
    let d = g.update_parent("t/p", unmasked(vec![named("t", "a"), named("t", "b")]));
    assert_eq!(d.added.len(), 2);
    assert!(d.removed.is_empty());
    let d = g.update_parent("t/p", unmasked(vec![named("t", "b"), wildcard("t")]));
    assert_eq!(d.added, vec![wildcard("t")]);
    assert_eq!(d.removed, vec![named("t", "a")]);
    let d = g.update_parent("t/p", unmasked(vec![]));
    assert_eq!(d.removed.len(), 2);
}

#[test]
fn parents_naming_tracks_named_targets_only() {
    let g = graph();
    g.update_parent("a/p1", unmasked(vec![named("t", "c"), wildcard("t")]));
    g.update_parent("a/p2", unmasked(vec![named("t", "c")]));
    assert_eq!(g.parents_naming("t/c"), vec!["a/p1".to_string(), "a/p2".to_string()]);
    assert!(g.parents_naming("t/other").is_empty(), "a wildcard target names nobody");

    // Retargeting drops the old named entry.
    g.update_parent("a/p1", unmasked(vec![named("t", "d")]));
    assert_eq!(g.parents_naming("t/c"), vec!["a/p2".to_string()]);
    assert_eq!(g.parents_naming("t/d"), vec!["a/p1".to_string()]);

    // Removal (empty targets or route removal) clears the index.
    g.update_parent("a/p2", unmasked(Vec::new()));
    assert!(g.parents_naming("t/c").is_empty());
    g.remove("a/p1");
    assert!(g.parents_naming("t/d").is_empty());
    assert!(g.all_parents().is_empty());
}

#[test]
fn edge_rules_collect_matching_targets_and_rule_change_is_a_delta() {
    let g = graph();
    let delta = g.update_parent("a/p", vec![masked(named("t", "c"), 0b01), masked(wildcard("t"), 0b10)]);
    assert_eq!(delta.added.len(), 2);
    assert_eq!(masks(g.edge_rules("a/p", "t/c")), Some(vec![0b01, 0b10]));
    assert_eq!(masks(g.edge_rules("a/p", "t/x")), Some(vec![0b10]));
    assert_eq!(g.edge_rules("a/p", "z/c"), None);
    assert_eq!(g.edge_rules("a/missing", "t/c"), None);

    // Same targets, one mask changed: reported as added, never as removed.
    let delta = g.update_parent("a/p", vec![masked(named("t", "c"), 0b100), masked(wildcard("t"), 0b10)]);
    assert_eq!(delta.added, vec![named("t", "c")]);
    assert!(delta.removed.is_empty());
    assert_eq!(g.all_parents(), vec!["a/p".to_string()]);

    // A scope change alone is a delta too (children refresh scope status).
    let scoped = DelegatingRule {
        mask: 0b10,
        scopes: Some(vec!["/api".to_string()]),
    };
    let delta = g.update_parent(
        "a/p",
        vec![masked(named("t", "c"), 0b100), (wildcard("t"), vec![scoped.clone()])],
    );
    assert_eq!(delta.added, vec![wildcard("t")]);

    // Unchanged reprocess: empty delta.
    let delta = g.update_parent(
        "a/p",
        vec![masked(named("t", "c"), 0b100), (wildcard("t"), vec![scoped])],
    );
    assert_eq!(delta, TargetDelta::default());
}

#[test]
fn blocked_route_gets_no_level_at_any_depth() {
    let g = graph();
    g.set_root("t/r", true);
    g.update_parent("t/r", unmasked(vec![named("t", "m")]));
    g.update_parent("t/m", unmasked(vec![named("t", "c")]));
    g.update_child("t/m", "t", vec!["t/r".to_string()], true);
    g.update_child("t/c", "t", vec!["t/m".to_string()], true);
    for key in ["t/r", "t/m", "t/c"] {
        g.recompute_level(key);
    }
    assert_eq!(g.level("t/c"), Some(3));

    // A blocked middle route keeps its edges but loses its level.
    g.set_blocked("t/m", true);
    assert_eq!(g.recompute_level("t/m").new, None);
    assert_eq!(g.children_of("t/m"), vec!["t/c".to_string()]);
    assert_eq!(g.recompute_level("t/c").new, None);
    assert_eq!(g.confirmed_keys("t/c").len(), 1, "edges are kept");

    // A blocked root has no level either; unblocking restores it.
    g.set_blocked("t/m", false);
    g.set_blocked("t/r", true);
    assert_eq!(g.recompute_level("t/r").new, None);
    g.set_blocked("t/r", false);
    assert_eq!(g.recompute_level("t/r").new, Some(1));
    g.remove("t/m");
    assert!(!g.is_blocked("t/m"));
}

#[test]
fn inert_declaration_is_visible_but_never_confirmed() {
    let g = graph();
    g.update_parent("t/root", unmasked(vec![named("t", "p")]));
    g.set_root("t/root", true);
    g.recompute_level("t/root");
    g.update_parent("t/p", unmasked(vec![named("t", "c"), wildcard("t")]));
    g.update_child("t/p", "t", vec!["t/root".to_string()], true);
    g.recompute_level("t/p");
    g.update_child("t/c", "t", vec!["t/p".to_string()], false);

    assert_eq!(g.declared_parents("t/c"), vec!["t/p".to_string()]);
    assert!(g.confirmed_keys("t/c").is_empty(), "an inert declaration forms no edge");
    assert_eq!(g.recompute_level("t/c").new, None, "an inert route gets no level");
    assert!(g.children_of("t/p").is_empty(), "an inert route is no level dependent");
    assert!(g.children_of_target("t/p", &named("t", "c")).is_empty());
}
