use radix_route_matcher::RadixTree;
use std::collections::HashMap;
use std::sync::Arc;
use super::radix_host::RadixHost;

/// Radix tree based hostname matching engine
///
/// This engine uses a radix tree for efficient hostname matching with reversed hostnames.
/// Hostnames are reversed (e.g., "api.example.com" -> "com.example.api") to enable
/// longest prefix matching from the TLD down.
///
/// **Lock-free concurrent reads**: Each query creates its own temporary iterator,
/// enabling true concurrent reads without any mutex contention. The tree itself
/// is immutable after initialization.
///
/// Supports wildcard patterns like "*.example.com" which become "com.example" (radix_key)
/// with wildcard_count tracking.
pub struct RadixHostMatchEngine<T> {
    tree: RadixTree,
    /// All RadixHost instances
    hosts: Vec<RadixHost<T>>,
    /// Mapping from tree_idx to list of host_idx
    /// tree_idx -> Vec<host_idx> (indices in hosts)
    tree_idx_to_host_idx: HashMap<i32, Vec<usize>>,
}

impl<T> RadixHostMatchEngine<T> {
    pub fn new() -> Self {
        Self {
            tree: RadixTree::new().expect("Failed to create radix tree"),
            hosts: Vec::new(),
            tree_idx_to_host_idx: HashMap::new(),
        }
    }

    /// Get the number of hosts in this engine
    pub fn host_count(&self) -> usize {
        self.hosts.len()
    }

    /// Get direct access to the underlying radix tree for advanced usage
    pub fn tree(&self) -> &RadixTree {
        &self.tree
    }

    /// Match a hostname and return the matched runtime
    ///
    /// # Arguments
    /// * `hostname` - The hostname to match (e.g., "api.example.com")
    ///
    /// # Returns
    /// `Some(Arc<T>)` if a match is found, `None` otherwise
    pub fn match_host(&self, hostname: &str) -> Option<Arc<T>> {
        let hostname_lower = hostname.to_lowercase();
        let reversed = RadixHost::<T>::reverse_hostname(&hostname_lower);

        println!("\n========== Radix Host Matching ==========");
        println!("Request Hostname: '{}'", hostname);
        println!("Reversed: '{}'", reversed);
        println!("Available hosts: {}", self.hosts.len());

        // Create iterator
        let iter = match self.tree.create_iter() {
            Ok(iter) => iter,
            Err(e) => {
                eprintln!("ERROR: Failed to create iterator: {}", e);
                return None;
            }
        };

        // Use search to initialize iterator, then iterate from longest to shortest using next_prefix
        println!("\n[Step 1] Searching in radix tree...");
        if !self.tree.search(&iter, &reversed) {
            println!("FAIL no match found in radix tree\n");
            return None;
        }

        println!("  OK found matches, iterating from longest to shortest...");

        // Get the first match (longest)
        let mut match_count = 0;
        loop {
            let tree_idx = if match_count == 0 {
                // First iteration: get current position from search
                match self.tree.next_prefix(&iter, &reversed) {
                    Some(idx) => idx,
                    None => break,
                }
            } else {
                // Subsequent iterations: move up to shorter prefixes
                match self.tree.next_prefix(&iter, &reversed) {
                    Some(idx) => idx,
                    None => break,
                }
            };

            match_count += 1;
            println!("  [Match #{}] Checking tree_idx: {}", match_count, tree_idx);

            if let Some(host_indices) = self.tree_idx_to_host_idx.get(&tree_idx) {
                println!("    -> {} host(s) at this tree node", host_indices.len());
                for &host_idx in host_indices {
                    if let Some(radix_host) = self.hosts.get(host_idx) {
                        println!(
                            "      Testing: original='{}', radix_key='{}', is_wildcard={}",
                            radix_host.original, radix_host.radix_key, radix_host.is_wildcard
                        );
                        if radix_host.matches(hostname) {
                            println!("      OK matched!");
                            return Some(radix_host.runtime.clone());
                        } else {
                            println!("      FAIL pattern did not match");
                        }
                    }
                }
            }
        }

        if match_count == 0 {
            println!("FAIL no matches found\n");
        } else {
            println!("FAIL checked {} match(es), none matched\n", match_count);
        }
        None
    }

    /// Initialize the engine with a list of RadixHost instances
    ///
    /// # Arguments
    /// * `hosts` - List of RadixHost instances
    ///
    /// # Returns
    /// `Ok(())` on success, `Err(String)` on failure
    pub fn initialize(&mut self, hosts: Vec<RadixHost<T>>) -> Result<(), String> {
        println!("\n========== RadixHostMatchEngine Initialize ==========");
        println!("Total hosts: {}", hosts.len());

        let mut next_tree_idx = 1i32;

        for radix_host in hosts {
            println!(
                "\n  [Host] pattern='{}', radix_key='{}', is_wildcard={}, wildcard_count={}",
                radix_host.original,
                radix_host.radix_key,
                radix_host.is_wildcard,
                radix_host.wildcard_count
            );

            let radix_key = radix_host.radix_key.clone();

            // Check if this radix_key already exists in the tree
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
                    format!(
                        "Failed to insert radix key '{}' for pattern '{}' into radix tree: {}",
                        radix_key, radix_host.original, e
                    )
                })?;

                println!(
                    "    Inserted radix_key: '{}' -> tree_idx: {}",
                    radix_key, new_tree_idx
                );
                next_tree_idx += 1;
                new_tree_idx
            };

            // Add RadixHost to the list
            let host_idx = self.hosts.len();
            self.hosts.push(radix_host);

            // Map tree_idx to host_idx (append to list)
            self.tree_idx_to_host_idx
                .entry(tree_idx)
                .or_insert_with(Vec::new)
                .push(host_idx);
        }

        println!("\n========== Initialization Complete ==========");
        println!("Summary:");
        println!("  - Total hosts: {}", self.hosts.len());
        println!(
            "  - Unique radix tree nodes: {}",
            self.tree_idx_to_host_idx.len()
        );
        println!("==============================================\n");
        Ok(())
    }
}

impl<T> Default for RadixHostMatchEngine<T> {
    fn default() -> Self {
        Self::new()
    }
}

// Thread-safe with lock-free reads
unsafe impl<T: Send + Sync> Sync for RadixHostMatchEngine<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock runtime for testing
    #[derive(Clone)]
    struct MockHostRuntime {
        name: String,
    }

    impl MockHostRuntime {
        fn new(name: &str, _hosts: Vec<&str>) -> Self {
            Self {
                name: name.to_string(),
            }
        }

        fn identifier(&self) -> String {
            self.name.clone()
        }
    }

    #[test]
    fn test_engine_initialization_single_host() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();

        let runtime = Arc::new(MockHostRuntime::new("test-runtime", vec!["example.com"]));
        let host = RadixHost::new("example.com", runtime);
        let result = engine.initialize(vec![host]);
        assert!(result.is_ok());

        assert_eq!(engine.host_count(), 1);
        assert_eq!(engine.tree_idx_to_host_idx.len(), 1);
    }

    #[test]
    fn test_engine_initialization_multiple_hosts() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();

        let runtime = Arc::new(MockHostRuntime::new(
            "multi-host-runtime",
            vec!["example.com", "api.example.com", "*.test.com"],
        ));

        let hosts = vec![
            RadixHost::new("example.com", runtime.clone()),
            RadixHost::new("api.example.com", runtime.clone()),
            RadixHost::new("*.test.com", runtime.clone()),
        ];

        let result = engine.initialize(hosts);
        assert!(result.is_ok());

        assert_eq!(engine.host_count(), 3);
        assert_eq!(engine.tree_idx_to_host_idx.len(), 3);
    }

    #[test]
    fn test_engine_exact_match() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();
        let runtime = Arc::new(MockHostRuntime::new(
            "test",
            vec!["example.com", "api.example.com"],
        ));

        let hosts = vec![
            RadixHost::new("example.com", runtime.clone()),
            RadixHost::new("api.example.com", runtime.clone()),
        ];
        engine.initialize(hosts).unwrap();

        assert!(engine.match_host("example.com").is_some());
        assert!(engine.match_host("api.example.com").is_some());
        assert!(engine.match_host("unknown.com").is_none());
    }

    #[test]
    fn test_engine_wildcard_match() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();
        let runtime = Arc::new(MockHostRuntime::new("test", vec!["*.example.com"]));
        let host = RadixHost::new("*.example.com", runtime);
        engine.initialize(vec![host]).unwrap();

        assert!(engine.match_host("api.example.com").is_some());
        assert!(engine.match_host("web.example.com").is_some());
        assert!(engine.match_host("example.com").is_none());
        assert!(engine.match_host("api.web.example.com").is_none());
    }

    #[test]
    fn test_engine_case_insensitive() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();
        let runtime = Arc::new(MockHostRuntime::new("test", vec!["Example.COM"]));
        let host = RadixHost::new("Example.COM", runtime);
        engine.initialize(vec![host]).unwrap();

        assert!(engine.match_host("example.com").is_some());
        assert!(engine.match_host("EXAMPLE.COM").is_some());
        assert!(engine.match_host("ExAmPlE.CoM").is_some());
    }

    #[test]
    fn test_engine_multiple_wildcards() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();
        let runtime = Arc::new(MockHostRuntime::new(
            "test",
            vec!["*.example.com", "*.*.example.com"],
        ));

        let hosts = vec![
            RadixHost::new("*.example.com", runtime.clone()),
            RadixHost::new("*.*.example.com", runtime.clone()),
        ];
        engine.initialize(hosts).unwrap();

        assert!(engine.match_host("api.example.com").is_some());
        assert!(engine.match_host("a.b.example.com").is_some());
    }

    #[test]
    fn test_engine_empty_initialization() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();

        let result = engine.initialize(vec![]);
        assert!(result.is_ok());

        assert_eq!(engine.host_count(), 0);
        assert!(engine.match_host("example.com").is_none());
    }

    #[test]
    fn test_engine_default_trait() {
        let engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::default();
        assert_eq!(engine.host_count(), 0);
    }

    #[test]
    fn test_engine_tree_access() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();
        let runtime = Arc::new(MockHostRuntime::new("test", vec!["example.com"]));
        let host = RadixHost::new("example.com", runtime);
        engine.initialize(vec![host]).unwrap();

        let tree = engine.tree();
        assert_eq!(tree.find_exact("com.example"), Some(1));
    }

    #[test]
    fn test_engine_localhost() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();
        let runtime = Arc::new(MockHostRuntime::new("test", vec!["localhost"]));
        let host = RadixHost::new("localhost", runtime);
        engine.initialize(vec![host]).unwrap();

        assert!(engine.match_host("localhost").is_some());
        assert!(engine.match_host("LOCALHOST").is_some());
        assert!(engine.match_host("api.localhost").is_none());
    }

    #[test]
    fn test_engine_multiple_runtimes() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();
        let runtime1 = Arc::new(MockHostRuntime::new("runtime1", vec!["example.com"]));
        let runtime2 = Arc::new(MockHostRuntime::new("runtime2", vec!["*.api.com"]));
        let runtime3 = Arc::new(MockHostRuntime::new("runtime3", vec!["test.com"]));
        let runtime4 = Arc::new(MockHostRuntime::new("runtime4", vec!["*.example.com"]));

        let hosts = vec![
            RadixHost::new("example.com", runtime1.clone()),
            RadixHost::new("*.api.com", runtime2.clone()),
            RadixHost::new("test.com", runtime3.clone()),
            RadixHost::new("*.example.com", runtime4.clone()),
        ];

        engine.initialize(hosts).unwrap();

        assert_eq!(engine.host_count(), 4);
        assert_eq!(engine.tree_idx_to_host_idx.len(), 3); // example.com and *.example.com share same radix_key

        // Test exact match for example.com (should match runtime1, not wildcard runtime4)
        let r1 = engine.match_host("example.com");
        assert!(r1.is_some());
        assert_eq!(r1.unwrap().identifier(), "runtime1");

        // Test wildcard match for *.example.com (should match runtime4)
        let r4 = engine.match_host("api.example.com");
        assert!(r4.is_some());
        assert_eq!(r4.unwrap().identifier(), "runtime4");

        // Test another subdomain for *.example.com
        let r4_2 = engine.match_host("web.example.com");
        assert!(r4_2.is_some());
        assert_eq!(r4_2.unwrap().identifier(), "runtime4");

        // Test *.api.com wildcard
        let r2 = engine.match_host("v1.api.com");
        assert!(r2.is_some());
        assert_eq!(r2.unwrap().identifier(), "runtime2");

        // Test exact match for test.com
        let r3 = engine.match_host("test.com");
        assert!(r3.is_some());
        assert_eq!(r3.unwrap().identifier(), "runtime3");

        // Test that *.example.com doesn't match too many levels
        let r_none = engine.match_host("sub.api.example.com");
        assert!(r_none.is_none());
    }
}
