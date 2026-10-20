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
fn wildcard_links_skip_only_nonmatching_physical_ancestors() {
    let mixed = [
        "com",
        "*.com",
        "example.com",
        "*.example.com",
        "branch.example.com",
        "a.branch.example.com",
        "b.branch.example.com",
        "x.long.example.com",
        "y.long.example.com",
    ];
    for values in [mixed.as_slice(), &["a.example.com", "b.example.com", "grpc"], &[]] {
        let tree = HostnameRadixTree::build(values.iter().map(|value| pattern(value)).collect()).unwrap();
        // Recover actual topology from child edges, independently of the shortcut.
        let mut physical_parents = vec![NONE; tree.nodes.len()];
        for (index, node) in tree.nodes.iter().enumerate() {
            let start = node.children_offset as usize;
            for child in &tree.children[start..start + node.children_len as usize] {
                physical_parents[child.node_index as usize] = index as u32;
            }
        }
        assert_eq!(tree.nodes[0].wildcard_parent_index, NONE);
        let mut skipped = false;
        for (index, node) in tree.nodes.iter().enumerate() {
            let mut expected = physical_parents[index];
            while expected != NONE && tree.nodes[expected as usize].terminal_flags & HAS_WILDCARD == 0 {
                skipped = true;
                expected = physical_parents[expected as usize];
            }
            assert_eq!(
                node.wildcard_parent_index, expected,
                "node={index}, patterns={values:?}"
            );
        }
        if values == mixed {
            assert_ne!(tree.nodes[0].terminal_flags & HAS_WILDCARD, 0);
            assert!(skipped);
            for (query, expected) in [
                (
                    "a.branch.example.com",
                    vec!["a.branch.example.com", "*.example.com", "*.com"],
                ),
                ("example.com", vec!["example.com", "*.com"]),
                ("com", vec!["com"]),
                // Miss inside a compressed edge and retain the shorter wildcard.
                ("z.lung.example.com", vec!["*.example.com", "*.com"]),
                ("unknown.com", vec!["*.com"]),
                ("unknown.test", vec![]),
            ] {
                let actual: Vec<_> = tree
                    .matching_ids(query)
                    .map(|id| tree.patterns[id.0 as usize].as_str())
                    .collect();
                assert_eq!(actual, expected, "query={query}");
            }
        } else {
            assert!(tree.nodes.iter().all(|node| node.wildcard_parent_index == NONE));
        }
    }
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
