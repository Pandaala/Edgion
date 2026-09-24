use super::*;
use std::mem::{align_of, size_of};

fn pattern(value: &str) -> RadixHostnamePattern {
    RadixHostnamePattern::parse(value).unwrap()
}

#[test]
fn flat_layout_sizes_are_stable() {
    assert_eq!(size_of::<FlatHostnameNode>(), 20);
    assert_eq!(align_of::<FlatHostnameNode>(), 4);
    assert_eq!(size_of::<ChildEntry>(), 5);
    assert_eq!(align_of::<ChildEntry>(), 1);
    assert!(size_of::<MatchingIds<'_, '_>>() <= 40);
}

#[test]
fn planned_and_emitted_pool_sizes_match() {
    let tree = HostnameRadixTree::build(vec![
        pattern("a.example.com"),
        pattern("*.example.com"),
        pattern("b.example.com"),
        pattern("grpc"),
    ])
    .unwrap();
    assert_eq!(tree.nodes.len(), tree.children.len() + 1);
    assert_eq!(tree.edge_bytes.len(), "a.example.combgrpc".len());
}

#[test]
fn checked_planning_limits_reject_each_pool_without_allocating_it() {
    let patterns = vec![pattern("a.example.com"), pattern("b.example.com")];
    for (limits, kind) in [
        (
            FreezeLimits {
                patterns: 1,
                ..FreezeLimits::PRODUCTION
            },
            CapacityKind::Patterns,
        ),
        (
            FreezeLimits {
                nodes: 1,
                ..FreezeLimits::PRODUCTION
            },
            CapacityKind::Nodes,
        ),
        (
            FreezeLimits {
                children: 1,
                ..FreezeLimits::PRODUCTION
            },
            CapacityKind::Children,
        ),
        (
            FreezeLimits {
                edge_bytes: 1,
                ..FreezeLimits::PRODUCTION
            },
            CapacityKind::EdgeBytes,
        ),
        (
            FreezeLimits {
                edge_len: 1,
                ..FreezeLimits::PRODUCTION
            },
            CapacityKind::EdgeLength,
        ),
        (
            FreezeLimits {
                suffix_len: 1,
                ..FreezeLimits::PRODUCTION
            },
            CapacityKind::SuffixLength,
        ),
        (
            FreezeLimits {
                children_per_node: 1,
                ..FreezeLimits::PRODUCTION
            },
            CapacityKind::ChildrenPerNode,
        ),
    ] {
        let Err(error) = HostnameRadixTree::build_with_limits(patterns.clone(), limits) else {
            panic!("the injected capacity must reject this topology");
        };
        assert_eq!(error.kind, kind);
    }
}

#[test]
fn exact_first_is_lazy_about_parent_wildcards() {
    let tree = HostnameRadixTree::build(vec![pattern("a.example.com"), pattern("*.example.com")]).unwrap();
    let mut ids = tree.matching_ids("a.example.com");
    let parent_before = ids.next_node;
    assert!(ids.next().is_some());
    assert_eq!(ids.next_node, parent_before);
    assert!(ids.next().is_some());
}

#[test]
fn terminal_base_decodes_single_and_double_terminals_without_using_none() {
    let exact_only = FlatHostnameNode {
        terminal_id: NONE - 1,
        terminal_flags: HAS_EXACT,
        ..FlatHostnameNode::EMPTY
    };
    assert_eq!(terminal_id(&exact_only, false), Some(HostPatternId(NONE - 1)));
    assert_eq!(terminal_id(&exact_only, true), None);

    let wildcard_only = FlatHostnameNode {
        terminal_id: NONE - 1,
        terminal_flags: HAS_WILDCARD,
        ..FlatHostnameNode::EMPTY
    };
    assert_eq!(terminal_id(&wildcard_only, false), None);
    assert_eq!(terminal_id(&wildcard_only, true), Some(HostPatternId(NONE - 1)));

    let both = FlatHostnameNode {
        terminal_id: NONE - 2,
        terminal_flags: HAS_EXACT | HAS_WILDCARD,
        ..FlatHostnameNode::EMPTY
    };
    assert_eq!(terminal_id(&both, false), Some(HostPatternId(NONE - 2)));
    assert_eq!(terminal_id(&both, true), Some(HostPatternId(NONE - 1)));

    let invalid_both = FlatHostnameNode {
        terminal_id: NONE - 1,
        terminal_flags: HAS_EXACT | HAS_WILDCARD,
        ..FlatHostnameNode::EMPTY
    };
    assert_eq!(terminal_id(&invalid_both, true), None);
}

#[test]
fn topology_and_pattern_ids_are_deterministic_across_input_orders() {
    let values = ["a.example.com", "*.example.com", "b.example.com", "grpc"];
    let forward = HostnameRadixTree::build(values.iter().map(|value| pattern(value)).collect()).unwrap();
    let reverse = HostnameRadixTree::build(values.iter().rev().map(|value| pattern(value)).collect()).unwrap();
    assert_eq!(forward.nodes.as_ref(), reverse.nodes.as_ref());
    assert_eq!(forward.children.as_ref(), reverse.children.as_ref());
    assert_eq!(forward.edge_bytes.as_ref(), reverse.edge_bytes.as_ref());
    assert_eq!(forward.patterns.as_ref(), reverse.patterns.as_ref());
}
