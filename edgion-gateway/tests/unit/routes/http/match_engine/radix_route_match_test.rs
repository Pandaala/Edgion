use super::*;

/// The twelve levels `cmp_candidate` resolves, in the order it consults them.
const LEVELS: [&str; 12] = [
    "is_exact",
    "static_segment_count",
    "static_char_count",
    "segment_count",
    "has_method",
    "header_count",
    "query_count",
    "creation_ts_ms",
    "route_key",
    "rule_index",
    "match_index",
    "path_idx",
];

#[derive(Clone)]
struct Candidate {
    path: PathSpecificity,
    spec: MatchSpecificity,
}

/// Direct comparison without ranks or the final index. This independently
/// checks that ranking preserves the current path and non-path order.
fn direct_cmp(a: &Candidate, b: &Candidate) -> Ordering {
    b.path.cmp(&a.path).then_with(|| a.spec.cmp_precedence(&b.spec))
}

/// Report which level of the chain decides an ordered pair and the ordering
/// it yields, or `None` when the pair is fully tied.
///
/// This is a hand-written mirror of `cmp_candidate` + `cmp_precedence`, kept
/// so the fixture's coverage of each level can be asserted rather than
/// claimed. A mirror can drift, so every caller must also check the returned
/// `Ordering` against the real `cmp_candidate` — that is what pins the two
/// together if a level is ever reordered or a direction flipped.
fn deciding_level(a: &Candidate, ia: u32, b: &Candidate, ib: u32) -> Option<(&'static str, Ordering)> {
    let (sa, sb) = (&a.spec, &b.spec);
    let steps: [Ordering; 12] = [
        b.path.is_exact.cmp(&a.path.is_exact),
        b.path.static_segment_count.cmp(&a.path.static_segment_count),
        b.path.static_char_count.cmp(&a.path.static_char_count),
        b.path.segment_count.cmp(&a.path.segment_count),
        sb.has_method.cmp(&sa.has_method),
        sb.header_count.cmp(&sa.header_count),
        sb.query_count.cmp(&sa.query_count),
        sa.creation_ts_ms.cmp(&sb.creation_ts_ms),
        sa.route_key.cmp(&sb.route_key),
        sa.rule_index.cmp(&sb.rule_index),
        sa.match_index.cmp(&sb.match_index),
        ia.cmp(&ib),
    ];
    steps
        .iter()
        .position(|o| *o != Ordering::Equal)
        .map(|i| (LEVELS[i], steps[i]))
}

fn base_candidate() -> Candidate {
    Candidate {
        path: RadixPath::new("/a/:x/c", 0, false).specificity,
        spec: MatchSpecificity {
            has_method: true,
            header_count: 2,
            query_count: 1,
            creation_ts_ms: 1_000,
            route_key: "ns-a/route-1".to_string(),
            rule_index: 0,
            match_index: 0,
        },
    }
}

/// A base candidate plus one variant per precedence level, each differing
/// from the base in exactly one field, plus an exact duplicate of the base.
/// Constructed this way so every level of the chain — including the
/// `path_idx` tie-break — decides at least one pair.
fn sample_candidates() -> Vec<Candidate> {
    let mutations: [fn(&mut Candidate); 11] = [
        |c| c.path.is_exact = false,
        |c| c.path.static_segment_count = 1,
        |c| c.path.static_char_count = 4,
        |c| c.path.segment_count = 2,
        |c| c.spec.has_method = false,
        |c| c.spec.header_count = 1,
        |c| c.spec.query_count = 0,
        |c| c.spec.creation_ts_ms = 2_000,
        |c| c.spec.route_key = "ns-b/route-1".to_string(),
        |c| c.spec.rule_index = 1,
        |c| c.spec.match_index = 1,
    ];

    let mut out = vec![base_candidate()];
    for mutate in mutations {
        let mut variant = base_candidate();
        mutate(&mut variant);
        out.push(variant);
    }
    // Exact duplicate of the base: the only pair `path_idx` can decide.
    out.push(base_candidate());
    out
}

fn rank_of(candidates: &[Candidate]) -> Vec<u32> {
    let mut ordered: Vec<u32> = (0..candidates.len() as u32).collect();
    ordered.sort_unstable_by(|a, b| {
        let (ca, cb) = (&candidates[*a as usize], &candidates[*b as usize]);
        cmp_candidate(ca.path, &ca.spec, *a, cb.path, &cb.spec, *b)
    });
    let mut rank = vec![0u32; candidates.len()];
    for (r, idx) in ordered.into_iter().enumerate() {
        rank[idx as usize] = r as u32;
    }
    rank
}

/// Two jobs at once:
///
/// 1. Guard the fixture used by the ordering tests below — one that silently
///    stopped reaching the deeper levels would make those tests look stronger
///    than they are.
/// 2. Pin `deciding_level` to the real comparator. The mirror is only useful
///    while it agrees with `cmp_candidate`, so every pair's mirrored ordering
///    is checked against production. Reordering a level in `cmp_candidate` or
///    flipping a direction in `cmp_precedence` fails here.
#[test]
fn sample_candidates_exercises_every_precedence_level() {
    let candidates = sample_candidates();
    let mut seen: Vec<&str> = Vec::new();
    for (i, a) in candidates.iter().enumerate() {
        for (j, b) in candidates.iter().enumerate() {
            if i == j {
                continue;
            }
            let produced = cmp_candidate(a.path, &a.spec, i as u32, b.path, &b.spec, j as u32);
            match deciding_level(a, i as u32, b, j as u32) {
                Some((level, ordering)) => {
                    assert_eq!(
                        ordering, produced,
                        "`deciding_level` drifted from `cmp_candidate` at `{level}` ({i} vs {j})"
                    );
                    if !seen.contains(&level) {
                        seen.push(level);
                    }
                }
                None => assert_eq!(produced, Ordering::Equal, "mirror saw a tie, comparator did not"),
            }
        }
    }
    for level in LEVELS {
        assert!(seen.contains(&level), "no pair is decided by `{level}`");
    }
}

/// The rank must never invert a pair relative to the direct
/// comparator. `direct_cmp` is retyped here independently of
/// `cmp_candidate`, so reordering `cmp_candidate`'s two top-level steps,
/// flipping the path-specificity direction, or dropping the specificity
/// step all fail here.
///
/// Its reach stops there: both sides call the same
/// `MatchSpecificity::cmp_precedence`, so a change *inside* that chain is
/// invisible to this test (it is covered by the directional tests in
/// `match_unit.rs` and by the drift check in
/// `sample_candidates_exercises_every_precedence_level`). Whether
/// `assign_precedence_ranks` writes the ranks correctly is likewise out of
/// scope here — that is
/// `conf_handler_impl::tests::engine_ranks_reproduce_direct_candidate_ordering`.
#[test]
fn rank_order_never_inverts_the_direct_comparator() {
    let candidates = sample_candidates();
    let rank = rank_of(&candidates);

    // A permutation, otherwise two candidates would share a rank and the
    // request-path sort could not distinguish them.
    let mut seen = rank.clone();
    seen.sort_unstable();
    assert_eq!(seen, (0..candidates.len() as u32).collect::<Vec<_>>());

    // Every subset of the fixture, so no ordering of candidates is missed.
    // Sorting by a unique rank is input-order independent, so the subsets
    // need no shuffling.
    let mut checked = 0usize;
    for mask in 1u32..(1 << candidates.len()) {
        let mut subset: Vec<u32> = (0..candidates.len() as u32).filter(|i| mask & (1 << i) != 0).collect();
        if subset.len() < 2 {
            continue;
        }
        subset.sort_unstable_by_key(|idx| rank[*idx as usize]);
        for pair in subset.windows(2) {
            let (a, b) = (&candidates[pair[0] as usize], &candidates[pair[1] as usize]);
            assert_ne!(
                direct_cmp(a, b),
                Ordering::Greater,
                "rank order puts a lower-precedence candidate first"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, (1 << candidates.len()) - candidates.len() - 1);
}

/// `cmp_candidate` must be a strict total order, otherwise ranking the
/// snapshot once would not be a permutation and `sort_unstable` on the
/// request path would be underdetermined.
///
/// These properties hold structurally for any lexicographic `.then` chain
/// over `Ord` fields, so this test cannot fail while `cmp_candidate` keeps
/// that shape. It is kept as an executable statement of the requirement, to
/// fail loudly if the comparator is ever rewritten into something that is
/// not a total order (a hand-rolled branch, a partial comparison, or a key
/// that is not unique).
#[test]
fn cmp_candidate_is_a_strict_total_order() {
    let candidates = sample_candidates();
    let cmp = |i: usize, j: usize| {
        let (a, b) = (&candidates[i], &candidates[j]);
        cmp_candidate(a.path, &a.spec, i as u32, b.path, &b.spec, j as u32)
    };

    for i in 0..candidates.len() {
        assert_eq!(cmp(i, i), Ordering::Equal);
        for j in 0..candidates.len() {
            assert_eq!(cmp(i, j).reverse(), cmp(j, i), "not antisymmetric: {i} vs {j}");
        }
    }

    // Transitivity: the property `sort_unstable_by` requires and the one a
    // reordered or non-total inner comparison would break.
    for i in 0..candidates.len() {
        for j in 0..candidates.len() {
            for k in 0..candidates.len() {
                if cmp(i, j) == Ordering::Less && cmp(j, k) == Ordering::Less {
                    assert_eq!(cmp(i, k), Ordering::Less, "not transitive: {i} < {j} < {k}");
                }
            }
        }
    }
}

/// A delegating unit is an ordinary radix candidate ranked by its own match
/// only (per-level precedence): a sibling root's longer prefix outranks it, and
/// the child's paths never enter the root engine.
#[test]
fn delegating_candidate_ranks_by_its_own_path_in_the_root_engine() {
    use crate::routes::http::routes_mgr::GlobalHttpRouteManagers;
    use edgion_common::conf_sync::traits::ConfHandler;
    use edgion_resources::{HTTPRoute, ListenerKey, ResourceMeta};

    let listener = ListenerKey::new("0.0.0.0:18720".parse().unwrap(), "HTTP");
    let route = |name: &str, parent: Option<&str>, path: &str, backends: serde_json::Value| {
        let parent_ref = match parent {
            Some(p) => serde_json::json!({"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": p}),
            None => serde_json::json!({"group": "gateway.networking.k8s.io", "kind": "Gateway", "name": "gw1"}),
        };
        let mut route: HTTPRoute = crate::test_current_status::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
            "metadata": {"namespace": "default", "name": name, "creationTimestamp": "2026-09-20T00:00:00Z"},
            "spec": {
                "resolvedStatusController": "edgion.io/gateway-controller",
                "parentRefs": [parent_ref],
                "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": path}}], "backendRefs": backends}]
            }
        }))
        .unwrap();
        if parent.is_none() {
            route.spec.resolved_listeners = Some(vec![listener.clone()]);
        }
        route.pre_parse();
        Arc::new(route)
    };
    let a = route(
        "rk-a",
        None,
        "/api",
        serde_json::json!([{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "rk-child"}]),
    );
    let b = route(
        "rk-b",
        None,
        "/api/users",
        serde_json::json!([{"name": "svc", "port": 80}]),
    );
    let child = route(
        "rk-child",
        Some("rk-a"),
        "/api/users/deep",
        serde_json::json!([{"name": "svc", "port": 80}]),
    );

    let mgr = GlobalHttpRouteManagers::new();
    mgr.full_set(&[&a, &b, &child].iter().map(|r| (r.key_name(), Arc::clone(r))).collect());
    let table = mgr.get_or_create_listener_manager(&listener).load_route_table();
    let engine = table.catch_all_routes.as_ref().unwrap().match_engine.clone().unwrap();

    let ordered: Vec<(&str, bool)> = engine
        .ordered_candidates_for_test("/api/users/deep")
        .iter()
        .map(|(_, path_idx)| {
            let unit = &engine.routes[engine.paths_for_test()[*path_idx as usize].route_idx];
            (unit.matched_info.rsc.name.as_str(), unit.delegate.is_some())
        })
        .collect();
    assert_eq!(ordered, [("rk-b", false), ("rk-a", true)]);
}
