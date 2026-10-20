use super::radix_path::{PathSpecificity, RadixPath};
use crate::ctx::EdgionHttpContext;
use crate::routes::http::match_unit::{MatchSpecificity, RouteMatchResult};
use crate::routes::http::HttpRouteRuleUnit;
use crate::runtime::store::listener_gateway_info::GatewayInfoIndex;
use edgion_algo::matcher::radix_path::{PathMatchKind, PathRadixBuilder, PathRadixError, PathRadixTree};
use edgion_resources::err::EdError;
use edgion_resources::err::EdError::RouteNotFound;
use pingora_proxy::Session;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

/// Total order over immutable route candidates. Path specificity sorts first,
/// followed by method, headers, query, creation time, name, and rule/match index.
/// The final path index makes the order strict, allowing request subsets to use
/// precomputed integer ranks without comparing these fields again.
fn cmp_candidate(
    path_a: PathSpecificity,
    spec_a: &MatchSpecificity,
    path_idx_a: u32,
    path_b: PathSpecificity,
    spec_b: &MatchSpecificity,
    path_idx_b: u32,
) -> Ordering {
    path_b
        .cmp(&path_a)
        .then_with(|| spec_a.cmp_precedence(spec_b))
        .then(path_idx_a.cmp(&path_idx_b))
}

/// Radix tree based route matching engine.
///
/// Uses a single `for_each_match_ext` tree traversal that visits both the matched
/// values and a [`PathMatchKind`] per value. The engine then applies lightweight
/// filtering:
/// - **Exact routes**: accepted only when `PathMatchKind::FullyConsumed`
/// - **Prefix routes**: accepted when `FullyConsumed` or `SegmentBoundary`
///   (rejects partial-segment matches like `/v2` vs `/v2example`)
///
/// **Lock-free concurrent reads**: the tree is immutable after initialization.
pub struct RadixRouteMatchEngine {
    tree: PathRadixTree,
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
            tree: PathRadixBuilder::new()
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
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    /// Match a route for the given request (recommended entry point).
    ///
    /// Performs a single `for_each_match_ext` tree traversal, filters candidates by
    /// [`PathMatchKind`], orders them by their build-time precedence rank, and runs
    /// deep matching.
    ///
    /// Exact precedes Prefix; ties compare aggregate static segment count,
    /// static character count, then segment count. Parameter names never rank.
    /// Remaining ties use the existing non-path specificity chain.
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
    /// Split out of [`Self::match_route`] so the [`PathMatchKind`] filter and the
    /// ordering can be exercised without a live `Session`; `match_route` adds
    /// only deep matching on top of this.
    ///
    /// The returned pairs are `(precedence_rank, path_idx)`. The rank is a
    /// strict total order computed once at build time, so ordering here is a
    /// plain integer sort: no comparator call, no `String` comparison, and no
    /// lookup into `self.routes`.
    fn ordered_candidates(&self, path: &str) -> SmallVec<[(u32, u32); 8]> {
        let mut candidates: SmallVec<[(u32, u32); 8]> = SmallVec::new();

        self.tree.for_each_match_ext(path, |tree_value, match_kind| {
            let Some(bucket_idx) = (tree_value as usize).checked_sub(1) else {
                return;
            };
            let Some(path_indices) = self.tree_value_paths.get(bucket_idx) else {
                return;
            };
            for &path_idx in path_indices {
                if let Some(radix_path) = self.radix_paths.get(path_idx as usize) {
                    if !radix_path.is_prefix_match && match_kind != PathMatchKind::FullyConsumed {
                        continue;
                    }
                    if radix_path.is_prefix_match && match_kind == PathMatchKind::PartialSegment {
                        continue;
                    }
                    candidates.push((radix_path.precedence_rank, path_idx));
                }
            }
        });

        candidates.sort_unstable();
        candidates
    }

    fn initialize_internal(&mut self, route_runtimes: Vec<Arc<HttpRouteRuleUnit>>) -> Result<(), EdError> {
        let mut builder = PathRadixBuilder::new();
        let mut total_paths = 0usize;
        let mut next_tree_value = 1usize;
        let mut path_to_value: HashMap<String, usize> = HashMap::new();

        for (route_idx, runtime) in route_runtimes.iter().enumerate() {
            for (path, is_prefix) in runtime.extract_paths() {
                if path.is_empty() {
                    continue;
                }

                let radix_path = RadixPath::new(path, route_idx, is_prefix);
                let tree_key = radix_path.tree_key();

                let tree_value = if let Some(&existing_value) = path_to_value.get(tree_key) {
                    existing_value
                } else {
                    let new_value = next_tree_value;
                    builder.insert(tree_key, new_value).map_err(|e: PathRadixError| {
                        EdError::InternalError(format!("Failed to insert path '{}' into radix tree: {}", tree_key, e))
                    })?;
                    path_to_value.insert(tree_key.to_string(), new_value);
                    next_tree_value += 1;
                    new_value
                };

                let path_idx = self.radix_paths.len() as u32;
                self.radix_paths.push(radix_path);
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
            .map_err(|e: PathRadixError| EdError::InternalError(format!("Failed to freeze radix tree: {}", e)))?;

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
                pa.specificity,
                &self.routes[pa.route_idx].specificity,
                *a,
                pb.specificity,
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
include!("../../../../tests/support/routes/http/match_engine/radix_route_match_test/245_impl_item.rs");

impl Default for RadixRouteMatchEngine {
    fn default() -> Self {
        let builder = PathRadixBuilder::new();
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
#[path = "../../../../tests/unit/routes/http/match_engine/radix_route_match_test.rs"]
mod tests;
