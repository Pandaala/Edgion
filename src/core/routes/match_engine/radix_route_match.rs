use super::radix_path::RadixPath;
use super::route_runtime::RouteEntry;
use crate::types::err::EdError;
use crate::types::err::EdError::RouteNotFound;
use crate::core::routes::radix_match::{RadixTreeBuilder, RadixTree, RouterError};
use pingora_proxy::Session;
use std::collections::HashMap;
use std::sync::Arc;

/// Radix tree based route matching engine
///
/// This engine uses a radix tree (compressed trie) for efficient path matching.
/// It's particularly good for large numbers of routes with common prefixes.
///
/// **Lock-free concurrent reads**: The tree is immutable after initialization,
/// enabling true concurrent reads without any mutex contention.
///
/// Multiple paths can map to the same route by storing the route_idx directly in the tree.
///
/// Uses dynamic dispatch (dyn RouteRuntime) to support any route implementation.
pub struct RadixRouteMatchEngine {
    tree: RadixTree,
    /// Routes implementing RouteRuntime trait
    routes: Vec<Arc<dyn RouteEntry>>,
    /// All RadixPath instances (flattened from all routes)
    radix_paths: Vec<RadixPath>,
    /// Mapping from tree value to list of path indices that share the same radix_key
    /// tree_value -> Vec<path_idx> (index in radix_paths)
    tree_value_to_path_idx: HashMap<u32, Vec<usize>>,
}

impl RadixRouteMatchEngine {
    /// Build a new RadixRouteMatchEngine with the given route runtimes
    pub fn build(route_runtimes: Vec<Arc<dyn RouteEntry>>) -> Result<Self, EdError> {
        let mut engine = Self {
            tree: RadixTreeBuilder::new().freeze().expect("Failed to create empty radix tree"),
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
    ) -> Result<Option<Arc<dyn RouteEntry>>, EdError> {
        if let Some(route) = self.routes.get(route_idx) {
            match route.deep_match(session) {
                Ok(true) => {
                    tracing::trace!("Route matched after deep_match: route_idx={}", route_idx);
                    return Ok(Some(route.clone()));
                }
                Ok(false) => {
                    tracing::trace!("Route {} failed deep_match", route_idx);
                    return Ok(None);
                }
                Err(e) => {
                    tracing::error!("deep_match failed for route_idx={}: {:?}", route_idx, e);
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    /// Try exact match for the request path
    /// Returns matched route if found, None if no exact match
    pub fn exact_match(&self, session: &mut Session) -> Result<Option<Arc<dyn RouteEntry>>, EdError> {
        let path = session.req_header().uri.path();

        tracing::trace!("[exact_match] Trying exact match for '{}'...", path);
        if let Some(values) = self.tree.match_exact(path) {
            tracing::trace!("Found {} value(s) from tree", values.len());
            for &tree_value in values {
                if let Some(path_indices) = self.tree_value_to_path_idx.get(&tree_value) {
                    tracing::trace!("Checking {} path(s) for value {}", path_indices.len(), tree_value);
                    for &path_idx in path_indices {
                        if let Some(radix_path) = self.radix_paths.get(path_idx) {
                            tracing::trace!(
                                "Checking: original='{}', radix_key='{}', is_prefix={}, route_idx={}",
                                radix_path.original, radix_path.radix_key, radix_path.is_prefix_match, radix_path.route_idx
                            );
                            // Skip paths with variables - they should be handled by prefix_match
                            if !radix_path.match_segments.is_empty() {
                                tracing::trace!("Skipping path with variables for exact match");
                                continue;
                            }
                            if !radix_path.matches(&path) {
                                tracing::trace!("Pattern match failed");
                                continue;
                            }
                            tracing::trace!("Pattern matched, trying deep match...");
                            if let Some(runtime) = self.try_route_deep_match(radix_path.route_idx, session)? {
                                tracing::debug!("Exact match succeeded");
                                return Ok(Some(runtime));
                            }
                        }
                    }
                }
            }
        }

        tracing::trace!("No exact match found");
        Ok(None)
    }

    /// Try prefix match for the request path
    /// Returns matched route if found, RouteNotFound error if no match
    pub fn prefix_match(&self, session: &mut Session) -> Result<Arc<dyn RouteEntry>, EdError> {
        let path = session.req_header().uri.path();

        tracing::trace!("[prefix_match] Trying prefix matching...");
        let all_values = self.tree.match_all_prefixes(path);
        tracing::trace!("Found {} value(s) from radix tree", all_values.len());

        if all_values.is_empty() {
            tracing::debug!("No prefix match found");
            return Err(RouteNotFound());
        }

        let mut matched_paths: Vec<usize> = Vec::new();
        for tree_value in all_values {
            tracing::trace!("Checking tree value: {}", tree_value);
            if let Some(path_indices) = self.tree_value_to_path_idx.get(&tree_value) {
                tracing::trace!("{} path(s) for this value", path_indices.len());
                for &path_idx in path_indices {
                    if let Some(radix_path) = self.radix_paths.get(path_idx) {
                        tracing::trace!(
                            "Testing: original='{}', radix_key='{}', is_prefix={}, route_idx={}",
                            radix_path.original, radix_path.radix_key, radix_path.is_prefix_match, radix_path.route_idx
                        );
                        if radix_path.matches(&path) {
                            tracing::trace!("Pattern matched");
                            matched_paths.push(path_idx);
                        } else {
                            tracing::trace!("Pattern did not match");
                        }
                    }
                }
            }
        }

        if matched_paths.is_empty() {
            tracing::debug!("No prefix match found (no patterns matched)");
            return Err(RouteNotFound());
        }

        tracing::trace!("Sorting {} matched path(s) by priority...", matched_paths.len());
        matched_paths.sort_by(|a, b| {
            let weight_a = self.radix_paths[*a].priority_weight;
            let weight_b = self.radix_paths[*b].priority_weight;
            weight_b.cmp(&weight_a)
        });

        for (i, path_idx) in matched_paths.iter().enumerate() {
            let radix_path = &self.radix_paths[*path_idx];
            tracing::trace!(
                "[{}] Trying: original='{}', priority={}, route_idx={}", i + 1,radix_path.original, radix_path.priority_weight, radix_path.route_idx);
            if let Some(runtime) = self.try_route_deep_match(radix_path.route_idx, session)? {
                tracing::debug!("Prefix match succeeded,original='{}', priority={}, route_idx={}", radix_path.original,radix_path.priority_weight, radix_path.route_idx);
                return Ok(runtime);
            }
        }

        // No route matched after trying all candidates
        tracing::debug!("No route matched (all deep matches failed)");
        Err(RouteNotFound())
    }

    /// Combined match route (for backward compatibility)
    /// Try exact match first, then prefix match
    pub fn match_route(&self, session: &mut Session) -> Result<Arc<dyn RouteEntry>, EdError> {
        tracing::trace!("========== Radix Route Matching ==========");

        // Try exact match first
        if let Some(route) = self.exact_match(session)? {
            return Ok(route);
        }

        // Fall back to prefix match
        self.prefix_match(session)
    }

    fn initialize_internal(&mut self, route_runtimes: Vec<Arc<dyn RouteEntry>>) -> Result<(), EdError> {
        tracing::debug!("========== RadixRouteMatchEngine Initialize ==========");
        tracing::debug!("Total route runtimes to compile: {}", route_runtimes.len());

        let mut builder = RadixTreeBuilder::new();
        let mut total_paths = 0usize;
        let mut next_tree_value = 1usize; // Start from 1
        let mut radix_key_to_value: HashMap<String, usize> = HashMap::new();

        for (route_idx, runtime) in route_runtimes.iter().enumerate() {
            // Extract all paths and their match types from the RouteRuntime
            let paths = runtime.extract_paths();

            tracing::debug!(
                "  [Route #{}] {} (paths: {})",
                route_idx,
                runtime.identifier(),
                paths.len()
            );

            for (path, is_prefix) in paths {
                if path.is_empty() {
                    continue;
                }

                // Log path compilation details
                tracing::debug!(
                    "    [COMPILING PATH] path='{}', route_idx={}, is_prefix={}, route_name={}",
                    path,
                    route_idx,
                    is_prefix,
                    runtime.identifier()
                );

                // Compile the path pattern with route_idx and is_prefix flag
                let radix_path = RadixPath::new(&path, route_idx, is_prefix);
                tracing::debug!(
                    "    [COMPILED] {} -> {} (radix_key='{}', priority={})",
                    path,
                    radix_path.match_type_str(),
                    radix_path.radix_key,
                    radix_path.priority_weight
                );

                let radix_key = radix_path.radix_key.clone();

                // Check if this radix_key already has a value assigned
                let tree_value = if let Some(&existing_value) = radix_key_to_value.get(&radix_key) {
                    tracing::debug!(
                        "    Reusing tree value: {} for radix_key: '{}'",
                        existing_value, radix_key
                    );
                    existing_value
                } else {
                    // First time seeing this radix_key, assign a new value and insert into builder
                    let new_value = next_tree_value;
                    builder.insert(&radix_key, new_value).map_err(|e: RouterError| {
                        EdError::InternalError(format!(
                            "Failed to insert radix key '{}' for path '{}' into radix tree: {}",
                            radix_key, path, e
                        ))
                    })?;

                    radix_key_to_value.insert(radix_key.clone(), new_value);
                    tracing::debug!("    Inserted radix_key: '{}' -> tree value: {}", radix_key, new_value);
                    next_tree_value += 1;
                    new_value
                };

                // Add RadixPath to the global list
                let path_idx = self.radix_paths.len();
                self.radix_paths.push(radix_path.clone());

                // Add path_idx to the tree_value mapping
                self.tree_value_to_path_idx
                    .entry(tree_value as u32)
                    .or_insert_with(Vec::new)
                    .push(path_idx);

                total_paths += 1;
            }

            // Store the RouteRuntime directly
            self.routes.push(runtime.clone());
        }

        // Freeze the builder to create the immutable tree
        tracing::debug!("Freezing radix tree...");
        self.tree = builder.freeze().map_err(|e: RouterError| {
            EdError::InternalError(format!("Failed to freeze radix tree: {}", e))
        })?;

        tracing::debug!("========== Initialization Complete ==========");
        tracing::debug!(
            "Summary: Total routes: {}, Total paths compiled: {}, Unique radix tree nodes: {}, RadixPath entries: {}",
            self.routes.len(),
            total_paths,
            self.tree_value_to_path_idx.len(),
            self.radix_paths.len()
        );
        tracing::debug!("==============================================");
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

// RadixRouteMatchEngine is now completely thread-safe with lock-free reads!
// The tree is immutable after initialization, and each query creates its own iterator.
unsafe impl Sync for RadixRouteMatchEngine {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Mock RouteRuntime for testing
    #[derive(Clone)]
    struct MockRoute {
        name: String,
        paths: Vec<(String, bool)>,
        should_match: bool,
    }

    impl MockRoute {
        fn new(name: &str, paths: Vec<(&str, bool)>) -> Self {
            Self {
                name: name.to_string(),
                paths: paths.iter().map(|(p, is_prefix)| (p.to_string(), *is_prefix)).collect(),
                should_match: true,
            }
        }

        #[allow(dead_code)]
        fn with_match_result(mut self, should_match: bool) -> Self {
            self.should_match = should_match;
            self
        }
    }

    impl RouteEntry for MockRoute {
        fn extract_paths(&self) -> Vec<(String, bool)> {
            self.paths.clone()
        }

        fn identifier(&self) -> String {
            self.name.clone()
        }

        fn deep_match(&self, _session: &Session) -> Result<bool, EdError> {
            Ok(self.should_match)
        }
    }

    // Helper to create a mock session (for future use)
    #[allow(dead_code)]
    fn create_mock_session(_path: &str) -> Session {
        // This is a simplified mock - in real tests you'd need proper session setup
        // For now, we'll focus on testing initialization and internal logic
        unimplemented!("Mock session creation - focus on initialization tests")
    }

    #[test]
    fn test_engine_initialization_single_route() {
        let route = MockRoute::new("test-route", vec![("/api", false)]);
        let routes: Vec<Arc<dyn RouteEntry>> = vec![Arc::new(route)];

        let engine = RadixRouteMatchEngine::build(routes).unwrap();

        assert_eq!(engine.route_count(), 1);
        assert_eq!(engine.radix_paths.len(), 1);
        assert_eq!(engine.tree_value_to_path_idx.len(), 1);
    }

    #[test]
    fn test_engine_initialization_multiple_routes() {
        let routes: Vec<Arc<dyn RouteEntry>> = vec![
            Arc::new(MockRoute::new("route1", vec![("/api", false)])),
            Arc::new(MockRoute::new("route2", vec![("/users", true)])),
            Arc::new(MockRoute::new("route3", vec![("/posts/:id", false)])),
        ];

        let engine = RadixRouteMatchEngine::build(routes).unwrap();

        assert_eq!(engine.route_count(), 3);
        assert_eq!(engine.radix_paths.len(), 3);
    }

    #[test]
    fn test_engine_multiple_paths_per_route() {
        let route = MockRoute::new("multi-path-route", vec![("/api/v1", false), ("/api/v2", false)]);

        let routes: Vec<Arc<dyn RouteEntry>> = vec![Arc::new(route)];
        let engine = RadixRouteMatchEngine::build(routes).unwrap();

        assert_eq!(engine.route_count(), 1);
        assert_eq!(engine.radix_paths.len(), 2);
    }

    #[test]
    fn test_engine_shared_radix_key() {
        // Two routes with same prefix but different suffixes
        let routes: Vec<Arc<dyn RouteEntry>> = vec![
            Arc::new(MockRoute::new("route1", vec![("/api/users", false)])),
            Arc::new(MockRoute::new("route2", vec![("/api/posts", false)])),
        ];

        let engine = RadixRouteMatchEngine::build(routes).unwrap();

        // Both paths share the same radix_key "/api/"
        // So we should have 2 routes, 2 paths, but potentially shared tree nodes
        assert_eq!(engine.route_count(), 2);
        assert_eq!(engine.radix_paths.len(), 2);
    }

    #[test]
    fn test_engine_priority_ordering() {
        let routes: Vec<Arc<dyn RouteEntry>> = vec![
            Arc::new(MockRoute::new("prefix", vec![("/api", true)])),
            Arc::new(MockRoute::new("exact", vec![("/api", false)])),
        ];

        let engine = RadixRouteMatchEngine::build(routes).unwrap();

        // Exact match_engine should have higher priority
        let exact_path = &engine.radix_paths[1];
        let prefix_path = &engine.radix_paths[0];

        assert!(exact_path.priority_weight > prefix_path.priority_weight);
    }

    #[test]
    fn test_engine_empty_initialization() {
        let routes: Vec<Arc<dyn RouteEntry>> = vec![];
        let engine = RadixRouteMatchEngine::build(routes).unwrap();

        assert_eq!(engine.route_count(), 0);
        assert_eq!(engine.radix_paths.len(), 0);
    }

    #[test]
    fn test_engine_tree_access() {
        let route = MockRoute::new("test", vec![("/api", false)]);
        let routes: Vec<Arc<dyn RouteEntry>> = vec![Arc::new(route)];
        let engine = RadixRouteMatchEngine::build(routes).unwrap();

        // Can access the underlying tree
        let tree = engine.tree();
        // The new API returns values as &[u32], check if it matches
        assert_eq!(tree.match_exact("/api"), Some(&[1u32][..]));
    }

    #[test]
    fn test_engine_radix_key_reuse() {
        // Multiple paths with same radix_key should reuse tree value
        let routes: Vec<Arc<dyn RouteEntry>> = vec![
            Arc::new(MockRoute::new("r1", vec![("/api/:v1", false)])),
            Arc::new(MockRoute::new("r2", vec![("/api/:v2", false)])),
        ];

        let engine = RadixRouteMatchEngine::build(routes).unwrap();

        // Verify that both paths share the same radix_key and tree value
        assert_eq!(engine.route_count(), 2);
        assert_eq!(engine.radix_paths.len(), 2);

        // Both have radix_key "/api/"
        assert_eq!(engine.radix_paths[0].radix_key, "/api/");
        assert_eq!(engine.radix_paths[1].radix_key, "/api/");

        // Should share the same tree value
        let values = engine.tree().match_exact("/api/").unwrap();
        assert_eq!(values.len(), 1);
        let tree_value = values[0];
        let paths_at_value = engine.tree_value_to_path_idx.get(&tree_value).unwrap();
        assert_eq!(paths_at_value.len(), 2);
    }

    #[test]
    fn test_engine_default_trait() {
        let engine = RadixRouteMatchEngine::default();
        assert_eq!(engine.route_count(), 0);
    }

    // Original RadixTree tests (adapted for new API)

    #[test]
    fn test_exact_match() {
        let mut builder = RadixTreeBuilder::new();
        builder.insert("/", 1).unwrap();
        builder.insert("/api", 2).unwrap();
        builder.insert("/api/users", 3).unwrap();
        let tree = builder.freeze().unwrap();

        assert_eq!(tree.match_exact("/"), Some(&[1][..]));
        assert_eq!(tree.match_exact("/api"), Some(&[2][..]));
        assert_eq!(tree.match_exact("/api/users"), Some(&[3][..]));
        assert_eq!(tree.match_exact("/api/users/1"), None);
        assert_eq!(tree.match_exact("/missing"), None);
    }

    #[test]
    fn test_longest_prefix() {
        let mut builder = RadixTreeBuilder::new();
        builder.insert("/", 1).unwrap();
        builder.insert("/api", 2).unwrap();
        builder.insert("/api/users", 3).unwrap();
        builder.insert("/assets", 4).unwrap();
        let tree = builder.freeze().unwrap();

        assert_eq!(tree.match_route_longest("/api/users/123"), &[3]);
        assert_eq!(tree.match_route_longest("/api/health"), &[2]);
        assert_eq!(tree.match_route_longest("/assets/logo.png"), &[4]);
        assert_eq!(tree.match_route_longest("/unknown"), &[1]);
    }

    #[test]
    fn test_all_prefixes_order_shortest_to_longest() {
        let mut builder = RadixTreeBuilder::new();
        builder.insert("/", 1).unwrap();
        builder.insert("/api", 2).unwrap();
        builder.insert("/api/v1", 3).unwrap();
        builder.insert("/api/v1/users", 4).unwrap();
        let tree = builder.freeze().unwrap();

        let prefixes = tree.match_all_prefixes("/api/v1/users/42");
        // New API returns shortest to longest
        assert_eq!(prefixes, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_ascii_paths_instead_of_unicode() {
        let mut builder = RadixTreeBuilder::new();
        builder.insert("/service", 10).unwrap();
        builder.insert("/service/user", 11).unwrap();
        let tree = builder.freeze().unwrap();

        assert_eq!(tree.match_route_longest("/service/user/detail"), &[11]);
        let prefixes = tree.match_all_prefixes("/service/user/detail");
        assert_eq!(prefixes, vec![10, 11]);
    }
}