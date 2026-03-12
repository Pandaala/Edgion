use super::radix_path::RadixPath;
use crate::core::common::matcher::radix_tree::{MatchKind, RadixTree, RadixTreeBuilder, RouterError};
use crate::core::gateway::routes::http::match_unit::RouteMatchResult;
use crate::core::gateway::routes::http::HttpRouteRuleUnit;
use crate::core::gateway::runtime::GatewayInfo;
use crate::types::ctx::EdgionHttpContext;
use crate::types::err::EdError;
use crate::types::err::EdError::RouteNotFound;
use pingora_proxy::Session;
use std::collections::HashMap;
use std::sync::Arc;

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
    /// Routes stored as concrete HttpRouteRuleUnit type
    routes: Vec<Arc<HttpRouteRuleUnit>>,
    /// All RadixPath instances (flattened from all routes)
    radix_paths: Vec<RadixPath>,
    /// Mapping from tree value to list of path indices that share the same path
    /// tree_value -> Vec<path_idx> (index in radix_paths)
    tree_value_to_path_idx: HashMap<u32, Vec<usize>>,
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
            tree_value_to_path_idx: HashMap::new(),
        };

        engine.initialize_internal(route_runtimes)?;
        Ok(engine)
    }

    /// Get the number of routes in this engine
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Get direct access to the underlying radix tree for advanced usage
    pub fn tree(&self) -> &RadixTree {
        &self.tree
    }

    fn try_route_deep_match(
        &self,
        route_idx: usize,
        session: &Session,
        ctx: &EdgionHttpContext,
        gateway_infos: &[GatewayInfo],
    ) -> Result<Option<RouteMatchResult>, EdError> {
        if let Some(route) = self.routes.get(route_idx) {
            match route.deep_match(session, ctx, gateway_infos) {
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
    /// [`MatchKind`], sorts by priority, and runs deep matching.
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
        gateway_infos: &[GatewayInfo],
    ) -> Result<RouteMatchResult, EdError> {
        let path = session.req_header().uri.path();
        let all_results = self.tree.match_all_ext(path);

        if all_results.is_empty() {
            return Err(RouteNotFound());
        }

        let mut matched_paths: Vec<usize> = Vec::new();
        for (tree_value, match_kind) in &all_results {
            if let Some(path_indices) = self.tree_value_to_path_idx.get(tree_value) {
                for &path_idx in path_indices {
                    if let Some(radix_path) = self.radix_paths.get(path_idx) {
                        if !radix_path.is_prefix_match && *match_kind != MatchKind::FullyConsumed {
                            continue;
                        }
                        if radix_path.is_prefix_match && *match_kind == MatchKind::PartialSegment {
                            continue;
                        }
                        matched_paths.push(path_idx);
                    }
                }
            }
        }

        if matched_paths.is_empty() {
            return Err(RouteNotFound());
        }

        matched_paths.sort_by(|a, b| {
            let weight_a = self.radix_paths[*a].priority_weight;
            let weight_b = self.radix_paths[*b].priority_weight;
            if weight_a != weight_b {
                return weight_b.cmp(&weight_a);
            }
            let route_a = &self.routes[self.radix_paths[*a].route_idx];
            let route_b = &self.routes[self.radix_paths[*b].route_idx];
            route_b.header_matcher_count().cmp(&route_a.header_matcher_count())
        });

        for path_idx in &matched_paths {
            let radix_path = &self.radix_paths[*path_idx];
            if let Some(result) = self.try_route_deep_match(radix_path.route_idx, session, ctx, gateway_infos)? {
                return Ok(result);
            }
        }

        Err(RouteNotFound())
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

                let path_idx = self.radix_paths.len();
                self.radix_paths.push(radix_path.clone());
                self.tree_value_to_path_idx
                    .entry(tree_value as u32)
                    .or_default()
                    .push(path_idx);

                total_paths += 1;
            }

            self.routes.push(runtime.clone());
        }

        self.tree = builder
            .freeze()
            .map_err(|e: RouterError| EdError::InternalError(format!("Failed to freeze radix tree: {}", e)))?;

        tracing::info!(
            component = "radix_engine",
            routes = self.routes.len(),
            paths = total_paths,
            tree_entries = self.tree_value_to_path_idx.len(),
            "initialized"
        );
        Ok(())
    }
}

impl Default for RadixRouteMatchEngine {
    fn default() -> Self {
        // Create an empty frozen tree
        let builder = RadixTreeBuilder::new();
        let tree = builder.freeze().expect("Failed to create empty radix tree");

        Self {
            tree,
            routes: Vec::new(),
            radix_paths: Vec::new(),
            tree_value_to_path_idx: HashMap::new(),
        }
    }
}
