use super::radix_path::RadixPath;
use crate::ctx::EdgionHttpContext;
use crate::routes::http::match_unit::{MatchSpecificity, RouteMatchResult};
use crate::routes::http::HttpRouteRuleUnit;
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use edgion_resources::err::EdError;
use edgion_resources::err::EdError::RouteNotFound;
use edgion_resources::matcher::radix_tree::{MatchKind, RadixTree, RadixTreeBuilder, RouterError};
use pingora_proxy::Session;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

/// Total order over route candidates, mirroring the Gateway API precedence
/// contract. Levels 1-2 (match kind, path character length) come from the path
/// pattern's `priority_weight`; levels 3-7 (method, headers, query, creation
/// timestamp, resource name, rule/match index) come from the route's
/// [`MatchSpecificity`]. `path_idx` is the final tie-break, which makes the
/// order strict so ranking the whole snapshot yields a permutation.
fn cmp_candidate(
    weight_a: usize,
    spec_a: &MatchSpecificity,
    path_idx_a: u32,
    weight_b: usize,
    spec_b: &MatchSpecificity,
    path_idx_b: u32,
) -> Ordering {
    weight_b
        .cmp(&weight_a)
        .then_with(|| spec_a.cmp_precedence(spec_b))
        .then(path_idx_a.cmp(&path_idx_b))
}

/// Radix tree based route matching engine.
///
/// Uses a single `match_all_ext` tree traversal that returns both the matched
/// values and a [`MatchKind`] per value. The engine then applies lightweight
/// filtering:
/// - **Exact routes**: accepted only when `MatchKind::FullyConsumed`
/// - **Prefix routes**: accepted when `FullyConsumed` or `SegmentBoundary`
///   (rejects partial-segment matches like `/v2` vs `/v2example`)
///
/// **Lock-free concurrent reads**: the tree is immutable after initialization.
pub struct RadixRouteMatchEngine {
    tree: RadixTree,
    routes: Vec<Arc<HttpRouteRuleUnit>>,
    /// All RadixPath instances (flattened from all routes)
    radix_paths: Vec<RadixPath>,
    /// Path indices grouped by tree value, indexed by `tree_value - 1`.
    /// Tree values are dense and start at 1, so a flat vector replaces a hash
    /// lookup on the request path.
    tree_value_paths: Vec<Vec<u32>>,
}

impl RadixRouteMatchEngine {
    /// Build a new RadixRouteMatchEngine with the given route runtimes
    pub fn build(route_runtimes: Vec<Arc<HttpRouteRuleUnit>>) -> Result<Self, EdError> {
        let mut engine = Self {
            tree: RadixTreeBuilder::new()
                .freeze()
                .expect("Failed to create empty radix tree"),
            routes: Vec::new(),
            radix_paths: Vec::new(),
            tree_value_paths: Vec::new(),
        };

        engine.initialize_internal(route_runtimes)?;
        Ok(engine)
    }

    fn try_route_deep_match(
        &self,
        route_idx: usize,
        session: &Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
    ) -> Result<Option<RouteMatchResult>, EdError> {
        if let Some(route) = self.routes.get(route_idx) {
            match route.deep_match(session, ctx, index) {
                Ok(Some(matched_gi)) => {
                    return Ok(Some(RouteMatchResult {
                        route_unit: route.clone(),
                        matched_gateway: matched_gi,
                    }));
                }
                Ok(None) => return Ok(None),
                Err(_) => return Ok(None),
            }
        }
        Ok(None)
    }

    /// Match a route for the given request (recommended entry point).
    ///
    /// Performs a single `match_all_ext` tree traversal, filters candidates by
    /// [`MatchKind`], orders them by their build-time precedence rank, and runs
    /// deep matching.
    ///
    /// Priority order (highest first):
    /// 1. Static exact matches
    /// 2. Parameter exact matches
    /// 3. Static prefix matches
    /// 4. Parameter prefix matches
    ///
    /// Within each category, longer paths have higher priority.
    pub fn match_route(
        &self,
        session: &mut Session,
        ctx: &EdgionHttpContext,
        index: &GatewayInfoIndex,
    ) -> Result<RouteMatchResult, EdError> {
        let candidates = self.ordered_candidates(ctx.path());
        if candidates.is_empty() {
            return Err(RouteNotFound());
        }

        for (_, path_idx) in &candidates {
            let radix_path = &self.radix_paths[*path_idx as usize];
            if let Some(result) = self.try_route_deep_match(radix_path.route_idx, session, ctx, index)? {
                return Ok(result);
            }
        }

        Err(RouteNotFound())
    }

    /// Collect the candidates a request path matches and order them by their
    /// build-time precedence rank.
    ///
    /// Split out of [`Self::match_route`] so the [`MatchKind`] filter and the
    /// ordering can be exercised without a live `Session`; `match_route` adds
    /// only deep matching on top of this.
    ///
    /// The returned pairs are `(precedence_rank, path_idx)`. The rank is a
    /// strict total order computed once at build time, so ordering here is a
    /// plain integer sort: no comparator call, no `String` comparison, and no
    /// lookup into `self.routes`.
    fn ordered_candidates(&self, path: &str) -> SmallVec<[(u32, u32); 8]> {
        let all_results = self.tree.match_all_ext(path);
        let mut candidates: SmallVec<[(u32, u32); 8]> = SmallVec::new();

        for (tree_value, match_kind) in &all_results {
            let Some(bucket_idx) = (*tree_value as usize).checked_sub(1) else {
                continue;
            };
            let Some(path_indices) = self.tree_value_paths.get(bucket_idx) else {
                continue;
            };
            for &path_idx in path_indices {
                if let Some(radix_path) = self.radix_paths.get(path_idx as usize) {
                    if !radix_path.is_prefix_match && *match_kind != MatchKind::FullyConsumed {
                        continue;
                    }
                    if radix_path.is_prefix_match && *match_kind == MatchKind::PartialSegment {
                        continue;
                    }
                    candidates.push((radix_path.precedence_rank, path_idx));
                }
            }
        }

        candidates.sort_unstable();
        candidates
    }

    fn initialize_internal(&mut self, route_runtimes: Vec<Arc<HttpRouteRuleUnit>>) -> Result<(), EdError> {
        let mut builder = RadixTreeBuilder::new();
        let mut total_paths = 0usize;
        let mut next_tree_value = 1usize;
        let mut path_to_value: HashMap<String, usize> = HashMap::new();

        for (route_idx, runtime) in route_runtimes.iter().enumerate() {
            let paths = runtime.extract_paths();

            for (path, is_prefix) in paths {
                if path.is_empty() {
                    continue;
                }

                let radix_path = RadixPath::new(&path, route_idx, is_prefix);
                let tree_key = radix_path.tree_key().to_string();

                let tree_value = if let Some(&existing_value) = path_to_value.get(&tree_key) {
                    existing_value
                } else {
                    let new_value = next_tree_value;
                    builder.insert(&tree_key, new_value).map_err(|e: RouterError| {
                        EdError::InternalError(format!("Failed to insert path '{}' into radix tree: {}", tree_key, e))
                    })?;
                    path_to_value.insert(tree_key.clone(), new_value);
                    next_tree_value += 1;
                    new_value
                };

                let path_idx = self.radix_paths.len() as u32;
                self.radix_paths.push(radix_path.clone());
                let bucket_idx = tree_value - 1;
                if self.tree_value_paths.len() <= bucket_idx {
                    self.tree_value_paths.resize(bucket_idx + 1, Vec::new());
                }
                self.tree_value_paths[bucket_idx].push(path_idx);

                total_paths += 1;
            }

            self.routes.push(runtime.clone());
        }

        self.tree = builder
            .freeze()
            .map_err(|e: RouterError| EdError::InternalError(format!("Failed to freeze radix tree: {}", e)))?;

        self.assign_precedence_ranks();

        tracing::info!(
            component = "radix_engine",
            routes = self.routes.len(),
            paths = total_paths,
            tree_entries = self.tree_value_paths.len(),
            "initialized"
        );
        Ok(())
    }

    /// Order the whole candidate set once and store each path's rank.
    ///
    /// Route precedence is derived entirely from immutable snapshot data, so it
    /// is resolved here — on the config-publish path — instead of per request.
    /// Because [`cmp_candidate`] is a strict total order, sorting any subset of
    /// candidates by the resulting rank yields exactly the order a direct
    /// comparator sort would produce.
    fn assign_precedence_ranks(&mut self) {
        let mut ordered: Vec<u32> = (0..self.radix_paths.len() as u32).collect();
        ordered.sort_unstable_by(|a, b| {
            let pa = &self.radix_paths[*a as usize];
            let pb = &self.radix_paths[*b as usize];
            cmp_candidate(
                pa.priority_weight,
                &self.routes[pa.route_idx].specificity,
                *a,
                pb.priority_weight,
                &self.routes[pb.route_idx].specificity,
                *b,
            )
        });
        for (rank, path_idx) in ordered.into_iter().enumerate() {
            self.radix_paths[path_idx as usize].precedence_rank = rank as u32;
        }
    }
}

#[cfg(test)]
impl RadixRouteMatchEngine {
    /// Test-only view of the compiled paths, so tests can assert against the
    /// ranks that `assign_precedence_ranks` actually wrote.
    pub(crate) fn paths_for_test(&self) -> &[RadixPath] {
        &self.radix_paths
    }

    /// Test-only view of the dense bucket table (`tree_value - 1` -> path indices).
    pub(crate) fn buckets_for_test(&self) -> &[Vec<u32>] {
        &self.tree_value_paths
    }

    /// Test-only entry into the request path's filter-and-order step, which
    /// `match_route` otherwise reaches only with a live `Session`.
    pub(crate) fn ordered_candidates_for_test(&self, path: &str) -> SmallVec<[(u32, u32); 8]> {
        self.ordered_candidates(path)
    }
}

impl Default for RadixRouteMatchEngine {
    fn default() -> Self {
        let builder = RadixTreeBuilder::new();
        let tree = builder.freeze().expect("Failed to create empty radix tree");

        Self {
            tree,
            routes: Vec::new(),
            radix_paths: Vec::new(),
            tree_value_paths: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nine levels `cmp_candidate` resolves, in the order it consults them.
    const LEVELS: [&str; 9] = [
        "weight",
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
        weight: usize,
        spec: MatchSpecificity,
    }

    /// Order used before this optimization: `priority_weight` descending, then
    /// the Gateway API precedence chain. It has no final tie-break, so it can
    /// return `Equal` — the request path relied on a stable sort to resolve that.
    fn legacy_cmp(a: &Candidate, b: &Candidate) -> Ordering {
        b.weight.cmp(&a.weight).then_with(|| a.spec.cmp_precedence(&b.spec))
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
        let steps: [Ordering; 9] = [
            b.weight.cmp(&a.weight),
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
            weight: 40,
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
        let mutations: [fn(&mut Candidate); 8] = [
            |c| c.weight = 23,
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
            cmp_candidate(ca.weight, &ca.spec, *a, cb.weight, &cb.spec, *b)
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
                let produced = cmp_candidate(a.weight, &a.spec, i as u32, b.weight, &b.spec, j as u32);
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

    /// The rank must never invert a pair relative to the pre-optimization
    /// comparator. `legacy_cmp` is retyped here independently of
    /// `cmp_candidate`, so reordering `cmp_candidate`'s two top-level steps,
    /// flipping the `priority_weight` direction, or dropping the specificity
    /// step all fail here.
    ///
    /// Its reach stops there: both sides call the same
    /// `MatchSpecificity::cmp_precedence`, so a change *inside* that chain is
    /// invisible to this test (it is covered by the directional tests in
    /// `match_unit.rs` and by the drift check in
    /// `sample_candidates_exercises_every_precedence_level`). Whether
    /// `assign_precedence_ranks` writes the ranks correctly is likewise out of
    /// scope here — that is
    /// `conf_handler_impl::tests::engine_ranks_reproduce_the_legacy_per_request_ordering`.
    #[test]
    fn rank_order_never_inverts_the_legacy_comparator() {
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
                    legacy_cmp(a, b),
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
            cmp_candidate(a.weight, &a.spec, i as u32, b.weight, &b.spec, j as u32)
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
}
