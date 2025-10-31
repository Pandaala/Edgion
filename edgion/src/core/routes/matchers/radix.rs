use super::radix_path::RadixPath;
use super::route_runtime::RouteRuntime;
use crate::types::err::EdError;
use crate::types::err::EdError::RouteNotFound;
use pingora_proxy::Session;
use radix_route_matcher::RadixTree;
use std::collections::HashMap;
use std::sync::Arc;

/// Radix tree based route matching engine
///
/// This engine uses a radix tree (compressed trie) for efficient path matching.
/// It's particularly good for large numbers of routes with common prefixes.
///
/// **Lock-free concurrent reads**: Each query creates its own temporary iterator,
/// enabling true concurrent reads without any mutex contention. The tree itself
/// is immutable after initialization.
///
/// Multiple paths can map to the same route by storing the route_idx directly in the tree.
///
/// Uses dynamic dispatch (dyn RouteRuntime) to support any route implementation.
pub struct RadixRouteMatchEngine {
    tree: RadixTree,
    /// Routes implementing RouteRuntime trait
    routes: Vec<Arc<dyn RouteRuntime>>,
    /// All RadixPath instances (flattened from all routes)
    radix_paths: Vec<RadixPath>,
    /// Mapping from tree_idx to list of path indices that share the same radix_key
    /// tree_idx -> Vec<path_idx> (index in radix_paths)
    tree_idx_to_path_idx: HashMap<i32, Vec<usize>>,
}

impl RadixRouteMatchEngine {
    pub fn new() -> Self {
        Self {
            tree: RadixTree::new().expect("Failed to create radix tree"),
            routes: Vec::new(),
            radix_paths: Vec::new(),
            tree_idx_to_path_idx: HashMap::new(),
        }
    }

    /// Get the number of routes in this engine
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Get direct access to the underlying radix tree for advanced usage
    pub fn tree(&self) -> &RadixTree {
        &self.tree
    }
}

impl RadixRouteMatchEngine {
    fn try_route_deep_match(
        &self,
        route_idx: usize,
        session: &Session,
    ) -> Result<Option<Arc<dyn RouteRuntime>>, EdError> {
        if let Some(route) = self.routes.get(route_idx) {
            match route.deep_match(session) {
                Ok(true) => {
                    println!("OK route matched after deep_match: route_idx={}", route_idx);
                    return Ok(Some(route.clone()));
                }
                Ok(false) => {
                    println!("FAIL route {} failed deep_match", route_idx);
                    return Ok(None);
                }
                Err(e) => {
                    eprintln!(
                        "ERROR: deep_match failed for route_idx={}: {:?}",
                        route_idx, e
                    );
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    pub fn match_route(&self, session: &mut Session) -> Result<Arc<dyn RouteRuntime>, EdError> {
        let path = session.req_header().uri.path();

        println!("\n========== Radix Route Matching ==========");
        println!("Request Path: '{}'", path);
        println!("Available routes: {}", self.routes.len());
        println!("Available paths: {}", self.radix_paths.len());

        // Step 1: Try exact match first (highest priority)
        println!("\n[Step 1] Trying exact match for '{}'...", path);
        if let Some(tree_idx) = self.tree.find_exact(path) {
            println!("  OK found exact tree_idx: {}", tree_idx);
            if let Some(path_indices) = self.tree_idx_to_path_idx.get(&tree_idx) {
                println!(
                    "  -> Checking {} path(s) at this tree node",
                    path_indices.len()
                );
                for &path_idx in path_indices {
                    if let Some(radix_path) = self.radix_paths.get(path_idx) {
                        println!("    Checking: original='{}', radix_key='{}', is_prefix={}, route_idx={}",
                                 radix_path.original, radix_path.radix_key, radix_path.is_prefix_match, radix_path.route_idx);
                        if !radix_path.matches(&path) {
                            println!("    FAIL pattern match failed");
                            continue;
                        }
                        println!("    OK pattern matched, trying deep match...");
                        if let Some(runtime) =
                            self.try_route_deep_match(radix_path.route_idx, session)?
                        {
                            println!("OK route matched\n");
                            return Ok(runtime);
                        }
                    }
                }
            }
        } else {
            println!("  FAIL no exact match found");
        }

        // Step 2: Try prefix matching (if exact match failed or didn't exist)
        println!("\n[Step 2] Trying prefix matching...");
        let iter = self
            .tree
            .create_iter()
            .map_err(|e| EdError::InternalError(format!("Failed to create iterator: {}", e)))?;
        let all_prefixes = self.tree.find_all_prefixes(&iter, &path);
        println!("  Found {} prefix(es) in radix tree", all_prefixes.len());
        if all_prefixes.is_empty() {
            println!("FAIL no route matched (no prefixes found)\n");
            return Err(RouteNotFound());
        }

        let mut matched_paths: Vec<usize> = Vec::new();
        for tree_idx in all_prefixes {
            println!("  Checking tree_idx: {}", tree_idx);
            if let Some(path_indices) = self.tree_idx_to_path_idx.get(&tree_idx) {
                println!("    -> {} path(s) at this tree node", path_indices.len());
                for &path_idx in path_indices {
                    if let Some(radix_path) = self.radix_paths.get(path_idx) {
                        println!("      Testing: original='{}', radix_key='{}', is_prefix={}, route_idx={}",
                                 radix_path.original, radix_path.radix_key, radix_path.is_prefix_match, radix_path.route_idx);
                        if radix_path.matches(&path) {
                            println!("      OK pattern matched");
                            matched_paths.push(path_idx);
                        } else {
                            println!("      FAIL pattern did not match");
                        }
                    }
                }
            }
        }

        if matched_paths.is_empty() {
            println!("FAIL no route matched (no patterns matched)\n");
            return Err(RouteNotFound());
        }

        println!(
            "\n[Step 3] Sorting {} matched path(s) by priority...",
            matched_paths.len()
        );
        matched_paths.sort_by(|a, b| {
            let weight_a = self.radix_paths[*a].priority_weight;
            let weight_b = self.radix_paths[*b].priority_weight;
            weight_b.cmp(&weight_a)
        });

        for (i, path_idx) in matched_paths.iter().enumerate() {
            let radix_path = &self.radix_paths[*path_idx];
            println!(
                "  [{}] Trying: original='{}', priority={}, route_idx={}",
                i + 1,
                radix_path.original,
                radix_path.priority_weight,
                radix_path.route_idx
            );
            if let Some(runtime) = self.try_route_deep_match(radix_path.route_idx, session)? {
                println!("OK route matched\n");
                return Ok(runtime);
            }
        }

        // No route matched after trying all candidates
        println!("FAIL no route matched (all deep matches failed)\n");
        Err(RouteNotFound())
    }

    pub fn initialize(
        &mut self,
        route_runtimes: Vec<Arc<dyn RouteRuntime>>,
    ) -> Result<(), EdError> {
        println!("\n========== RadixRouteMatchEngine Initialize ==========");
        println!("Total route runtimes to compile: {}", route_runtimes.len());

        let mut total_paths = 0usize;
        let mut next_tree_idx = 1i32; // Start from 1, as 0 might be reserved

        for (route_idx, runtime) in route_runtimes.iter().enumerate() {
            // Extract all paths and their match types from the RouteRuntime
            let paths = runtime.extract_paths();

            println!(
                "\n  [Route #{}] {} (paths: {})",
                route_idx,
                runtime.identifier(),
                paths.len()
            );

            for (path, is_prefix) in paths {
                if path.is_empty() {
                    continue;
                }

                // Log path compilation details
                println!(
                    "    [COMPILING PATH] path='{}', route_idx={}, is_prefix={}, route_name={}",
                    path,
                    route_idx,
                    is_prefix,
                    runtime.identifier()
                );

                // Compile the path pattern with route_idx and is_prefix flag
                let radix_path = RadixPath::new(&path, route_idx, is_prefix);
                println!(
                    "    [COMPILED] {} -> {} (radix_key='{}', priority={})",
                    path,
                    radix_path.match_type_str(),
                    radix_path.radix_key,
                    radix_path.priority_weight
                );

                let radix_key = radix_path.radix_key.clone();

                // Check if this radix_key already exists in the tree using find_exact
                let tree_idx = if let Some(existing_tree_idx) = self.tree.find_exact(&radix_key) {
                    println!(
                        "    Reusing tree_idx: {} for radix_key: '{}'",
                        existing_tree_idx, radix_key
                    );
                    existing_tree_idx
                } else {
                    // First time seeing this radix_key, insert into tree
                    let new_tree_idx = next_tree_idx;
                    self.tree.insert(&radix_key, new_tree_idx).map_err(|e| {
                        EdError::InternalError(format!(
                            "Failed to insert radix key '{}' for path '{}' into radix tree: {}",
                            radix_key, path, e
                        ))
                    })?;

                    println!(
                        "    Inserted radix_key: '{}' -> tree_idx: {}",
                        radix_key, new_tree_idx
                    );
                    next_tree_idx += 1;
                    new_tree_idx
                };

                // Add RadixPath to the global list
                let path_idx = self.radix_paths.len();
                self.radix_paths.push(radix_path.clone());

                // Add path_idx to the tree_idx mapping
                self.tree_idx_to_path_idx
                    .entry(tree_idx)
                    .or_insert_with(Vec::new)
                    .push(path_idx);

                total_paths += 1;
            }

            // Store the RouteRuntime directly
            self.routes.push(runtime.clone());
        }

        println!("\n========== Initialization Complete ==========");
        println!("Summary:");
        println!("  - Total routes: {}", self.routes.len());
        println!("  - Total paths compiled: {}", total_paths);
        println!(
            "  - Unique radix tree nodes: {}",
            self.tree_idx_to_path_idx.len()
        );
        println!("  - RadixPath entries: {}", self.radix_paths.len());
        println!("==============================================\n");
        Ok(())
    }
}

impl Default for RadixRouteMatchEngine {
    fn default() -> Self {
        Self::new()
    }
}

// RadixRouteMatchEngine is now completely thread-safe with lock-free reads!
// The tree is immutable after initialization, and each query creates its own iterator.
unsafe impl Sync for RadixRouteMatchEngine {}

#[cfg(test)]
mod tests {
    use super::*;
    use radix_route_matcher::RadixTree;
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
                paths: paths
                    .iter()
                    .map(|(p, is_prefix)| (p.to_string(), *is_prefix))
                    .collect(),
                should_match: true,
            }
        }

        #[allow(dead_code)]
        fn with_match_result(mut self, should_match: bool) -> Self {
            self.should_match = should_match;
            self
        }
    }

    impl RouteRuntime for MockRoute {
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
        let mut engine = RadixRouteMatchEngine::new();

        let route = MockRoute::new("test-route", vec![("/api", false)]);
        let routes: Vec<Arc<dyn RouteRuntime>> = vec![Arc::new(route)];

        let result = engine.initialize(routes);
        assert!(result.is_ok());

        assert_eq!(engine.route_count(), 1);
        assert_eq!(engine.radix_paths.len(), 1);
        assert_eq!(engine.tree_idx_to_path_idx.len(), 1);
    }

    #[test]
    fn test_engine_initialization_multiple_routes() {
        let mut engine = RadixRouteMatchEngine::new();

        let routes: Vec<Arc<dyn RouteRuntime>> = vec![
            Arc::new(MockRoute::new("route1", vec![("/api", false)])),
            Arc::new(MockRoute::new("route2", vec![("/users", true)])),
            Arc::new(MockRoute::new("route3", vec![("/posts/{id}", false)])),
        ];

        let result = engine.initialize(routes);
        assert!(result.is_ok());

        assert_eq!(engine.route_count(), 3);
        assert_eq!(engine.radix_paths.len(), 3);
    }

    #[test]
    fn test_engine_multiple_paths_per_route() {
        let mut engine = RadixRouteMatchEngine::new();

        let route = MockRoute::new(
            "multi-path-route",
            vec![("/api/v1", false), ("/api/v2", false)],
        );

        let routes: Vec<Arc<dyn RouteRuntime>> = vec![Arc::new(route)];
        let result = engine.initialize(routes);
        assert!(result.is_ok());

        assert_eq!(engine.route_count(), 1);
        assert_eq!(engine.radix_paths.len(), 2);
    }

    #[test]
    fn test_engine_shared_radix_key() {
        let mut engine = RadixRouteMatchEngine::new();

        // Two routes with same prefix but different suffixes
        let routes: Vec<Arc<dyn RouteRuntime>> = vec![
            Arc::new(MockRoute::new("route1", vec![("/api/users", false)])),
            Arc::new(MockRoute::new("route2", vec![("/api/posts", false)])),
        ];

        let result = engine.initialize(routes);
        assert!(result.is_ok());

        // Both paths share the same radix_key "/api/"
        // So we should have 2 routes, 2 paths, but potentially shared tree nodes
        assert_eq!(engine.route_count(), 2);
        assert_eq!(engine.radix_paths.len(), 2);
    }

    #[test]
    fn test_engine_priority_ordering() {
        let mut engine = RadixRouteMatchEngine::new();

        let routes: Vec<Arc<dyn RouteRuntime>> = vec![
            Arc::new(MockRoute::new("prefix", vec![("/api", true)])),
            Arc::new(MockRoute::new("exact", vec![("/api", false)])),
        ];

        let result = engine.initialize(routes);
        assert!(result.is_ok());

        // Exact match should have higher priority
        let exact_path = &engine.radix_paths[1];
        let prefix_path = &engine.radix_paths[0];

        assert!(exact_path.priority_weight > prefix_path.priority_weight);
    }

    #[test]
    fn test_engine_empty_initialization() {
        let mut engine = RadixRouteMatchEngine::new();

        let routes: Vec<Arc<dyn RouteRuntime>> = vec![];
        let result = engine.initialize(routes);
        assert!(result.is_ok());

        assert_eq!(engine.route_count(), 0);
        assert_eq!(engine.radix_paths.len(), 0);
    }

    #[test]
    fn test_engine_tree_access() {
        let mut engine = RadixRouteMatchEngine::new();

        let route = MockRoute::new("test", vec![("/api", false)]);
        let routes: Vec<Arc<dyn RouteRuntime>> = vec![Arc::new(route)];
        engine.initialize(routes).unwrap();

        // Can access the underlying tree
        let tree = engine.tree();
        assert_eq!(tree.find_exact("/api"), Some(1));
    }

    #[test]
    fn test_engine_radix_key_reuse() {
        let mut engine = RadixRouteMatchEngine::new();

        // Multiple paths with same radix_key should reuse tree_idx
        let routes: Vec<Arc<dyn RouteRuntime>> = vec![
            Arc::new(MockRoute::new("r1", vec![("/api/{v1}", false)])),
            Arc::new(MockRoute::new("r2", vec![("/api/{v2}", false)])),
        ];

        let result = engine.initialize(routes);
        assert!(result.is_ok());

        // Both have radix_key "/api/"
        assert_eq!(engine.radix_paths[0].radix_key, "/api/");
        assert_eq!(engine.radix_paths[1].radix_key, "/api/");

        // Should share the same tree_idx
        let tree_idx_1 = engine.tree().find_exact("/api/").unwrap();
        let paths_at_idx = engine.tree_idx_to_path_idx.get(&tree_idx_1).unwrap();
        assert_eq!(paths_at_idx.len(), 2);
    }

    #[test]
    fn test_engine_default_trait() {
        let engine = RadixRouteMatchEngine::default();
        assert_eq!(engine.route_count(), 0);
    }

    // Original RadixTree tests (keep for tree-level verification)

    #[test]
    fn test_exact_match() {
        let mut tree = RadixTree::new().expect("create tree");
        tree.insert("/", 1).unwrap();
        tree.insert("/api", 2).unwrap();
        tree.insert("/api/users", 3).unwrap();

        assert_eq!(tree.find_exact("/"), Some(1));
        assert_eq!(tree.find_exact("/api"), Some(2));
        assert_eq!(tree.find_exact("/api/users"), Some(3));
        assert_eq!(tree.find_exact("/api/users/1"), None);
        assert_eq!(tree.find_exact("/missing"), None);
    }

    #[test]
    fn test_longest_prefix() {
        let mut tree = RadixTree::new().expect("create tree");
        tree.insert("/", 1).unwrap();
        tree.insert("/api", 2).unwrap();
        tree.insert("/api/users", 3).unwrap();
        tree.insert("/assets", 4).unwrap();

        let iter = tree.create_iter().unwrap();
        assert_eq!(tree.longest_prefix(&iter, "/api/users/123"), Some(3));
        let iter = tree.create_iter().unwrap();
        assert_eq!(tree.longest_prefix(&iter, "/api/health"), Some(2));
        let iter = tree.create_iter().unwrap();
        assert_eq!(tree.longest_prefix(&iter, "/assets/logo.png"), Some(4));
        let iter = tree.create_iter().unwrap();
        assert_eq!(tree.longest_prefix(&iter, "/unknown"), Some(1));
    }

    #[test]
    fn test_all_prefixes_order_longest_to_shortest() {
        let mut tree = RadixTree::new().expect("create tree");
        tree.insert("/", 1).unwrap();
        tree.insert("/api", 2).unwrap();
        tree.insert("/api/v1", 3).unwrap();
        tree.insert("/api/v1/users", 4).unwrap();

        let iter = tree.create_iter().unwrap();
        let prefixes = tree.find_all_prefixes(&iter, "/api/v1/users/42");
        // Expect longest to shortest
        assert_eq!(prefixes, vec![4, 3, 2, 1]);
    }

    #[test]
    fn test_ascii_paths_instead_of_unicode() {
        let mut tree = RadixTree::new().expect("create tree");
        tree.insert("/service", 10).unwrap();
        tree.insert("/service/user", 11).unwrap();

        let iter = tree.create_iter().unwrap();
        assert_eq!(tree.longest_prefix(&iter, "/service/user/detail"), Some(11));
        let iter = tree.create_iter().unwrap();
        let prefixes = tree.find_all_prefixes(&iter, "/service/user/detail");
        assert_eq!(prefixes, vec![11, 10]);
    }

    #[test]
    fn test_insert_update_and_remove() {
        let mut tree = RadixTree::new().expect("create tree");
        tree.insert("/api", 2).unwrap();
        assert_eq!(tree.find_exact("/api"), Some(2));

        // Update existing key value
        tree.insert("/api", 5).unwrap();
        assert_eq!(tree.find_exact("/api"), Some(5));

        // Remove
        tree.remove("/api").unwrap();
        assert_eq!(tree.find_exact("/api"), None);
    }

    #[test]
    fn test_iterator_sequence_longest_to_shortest() {
        let mut tree = RadixTree::new().expect("create tree");
        tree.insert("/", 1).unwrap();
        tree.insert("/api", 2).unwrap();
        tree.insert("/api/users", 3).unwrap();

        let iter = tree.create_iter().unwrap();
        assert!(tree.search(&iter, "/api/users/123"));
        let mut seen = Vec::new();
        while let Some(idx) = tree.next_prefix(&iter, "/api/users/123") {
            seen.push(idx);
        }
        assert_eq!(seen, vec![3, 2, 1]);
    }
}
