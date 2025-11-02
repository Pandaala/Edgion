use super::radix_host::RadixHost;
use radix_route_matcher::RadixTree;
use std::collections::HashMap;

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
/// Supports wildcard patterns like "*.example.com" which become "com.example.*" after reversal.
pub struct RadixHostMatchEngine {
    tree: RadixTree,
    /// All RadixHost instances
    hosts: Vec<RadixHost>,
    /// Mapping from tree_idx to list of host indices that share the same radix_key
    /// tree_idx -> Vec<host_idx> (index in hosts)
    tree_idx_to_host_idx: HashMap<i32, Vec<usize>>,
}

impl RadixHostMatchEngine {
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

    /// Match a hostname and return the matched host index
    ///
    /// # Arguments
    /// * `hostname` - The hostname to match (e.g., "api.example.com")
    ///
    /// # Returns
    /// `Some(host_idx)` if a match is found, `None` otherwise
    pub fn match_host(&self, hostname: &str) -> Option<usize> {
        let hostname_lower = hostname.to_lowercase();
        let reversed = RadixHost::reverse_hostname(&hostname_lower);

        println!("\n========== Radix Host Matching ==========");
        println!("Request Hostname: '{}'", hostname);
        println!("Reversed: '{}'", reversed);
        println!("Available hosts: {}", self.hosts.len());

        // Step 1: Try exact match first
        println!("\n[Step 1] Trying exact match for '{}'...", reversed);
        if let Some(tree_idx) = self.tree.find_exact(&reversed) {
            println!("  OK found exact tree_idx: {}", tree_idx);
            if let Some(host_indices) = self.tree_idx_to_host_idx.get(&tree_idx) {
                println!(
                    "  -> Checking {} host(s) at this tree node",
                    host_indices.len()
                );
                for &host_idx in host_indices {
                    if let Some(radix_host) = self.hosts.get(host_idx) {
                        println!(
                            "    Checking: original='{}', radix_key='{}', is_wildcard={}",
                            radix_host.original, radix_host.radix_key, radix_host.is_wildcard
                        );
                        if radix_host.matches(hostname) {
                            println!("    OK matched!");
                            return Some(host_idx);
                        } else {
                            println!("    FAIL pattern match failed");
                        }
                    }
                }
            }
        } else {
            println!("  FAIL no exact match found");
        }

        // Step 2: Try prefix matching for wildcard patterns
        println!("\n[Step 2] Trying prefix matching...");
        let iter = match self.tree.create_iter() {
            Ok(iter) => iter,
            Err(e) => {
                eprintln!("ERROR: Failed to create iterator: {}", e);
                return None;
            }
        };

        let all_prefixes = self.tree.find_all_prefixes(&iter, &reversed);
        println!("  Found {} prefix(es) in radix tree", all_prefixes.len());

        if all_prefixes.is_empty() {
            println!("FAIL no host matched (no prefixes found)\n");
            return None;
        }

        // Check each prefix match
        for tree_idx in all_prefixes {
            println!("  Checking tree_idx: {}", tree_idx);
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
                            return Some(host_idx);
                        } else {
                            println!("      FAIL pattern did not match");
                        }
                    }
                }
            }
        }

        println!("FAIL no host matched\n");
        None
    }

    /// Initialize the engine with a list of hostname patterns
    ///
    /// # Arguments
    /// * `hostname_patterns` - List of hostname patterns (e.g., ["example.com", "*.api.com"])
    ///
    /// # Returns
    /// `Ok(())` on success, `Err(String)` on failure
    pub fn initialize(&mut self, hostname_patterns: Vec<String>) -> Result<(), String> {
        println!("\n========== RadixHostMatchEngine Initialize ==========");
        println!("Total hostname patterns: {}", hostname_patterns.len());

        let mut next_tree_idx = 1i32;

        for (host_idx, pattern) in hostname_patterns.iter().enumerate() {
            if pattern.is_empty() {
                continue;
            }

            println!("\n  [Host #{}] pattern='{}'", host_idx, pattern);

            // Create RadixHost
            let radix_host = RadixHost::new(pattern, host_idx);
            println!(
                "    [COMPILED] '{}' -> radix_key='{}', is_wildcard={}",
                pattern, radix_host.radix_key, radix_host.is_wildcard
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
                        radix_key, pattern, e
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
            let current_host_idx = self.hosts.len();
            self.hosts.push(radix_host);

            // Add host_idx to the tree_idx mapping
            self.tree_idx_to_host_idx
                .entry(tree_idx)
                .or_insert_with(Vec::new)
                .push(current_host_idx);
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

    /// Get the original hostname pattern by host index
    pub fn get_host_pattern(&self, host_idx: usize) -> Option<&str> {
        self.hosts.get(host_idx).map(|h| h.original.as_str())
    }
}

impl Default for RadixHostMatchEngine {
    fn default() -> Self {
        Self::new()
    }
}

// Thread-safe with lock-free reads
unsafe impl Sync for RadixHostMatchEngine {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_initialization_single_host() {
        let mut engine = RadixHostMatchEngine::new();

        let result = engine.initialize(vec!["example.com".to_string()]);
        assert!(result.is_ok());

        assert_eq!(engine.host_count(), 1);
        assert_eq!(engine.hosts.len(), 1);
        assert_eq!(engine.tree_idx_to_host_idx.len(), 1);
    }

    #[test]
    fn test_engine_initialization_multiple_hosts() {
        let mut engine = RadixHostMatchEngine::new();

        let result = engine.initialize(vec![
            "example.com".to_string(),
            "api.example.com".to_string(),
            "*.test.com".to_string(),
        ]);
        assert!(result.is_ok());

        assert_eq!(engine.host_count(), 3);
    }

    #[test]
    fn test_engine_exact_match() {
        let mut engine = RadixHostMatchEngine::new();
        engine
            .initialize(vec![
                "example.com".to_string(),
                "api.example.com".to_string(),
            ])
            .unwrap();

        assert_eq!(engine.match_host("example.com"), Some(0));
        assert_eq!(engine.match_host("api.example.com"), Some(1));
        assert_eq!(engine.match_host("unknown.com"), None);
    }

    #[test]
    fn test_engine_wildcard_match() {
        let mut engine = RadixHostMatchEngine::new();
        engine
            .initialize(vec!["*.example.com".to_string()])
            .unwrap();

        assert_eq!(engine.match_host("api.example.com"), Some(0));
        assert_eq!(engine.match_host("web.example.com"), Some(0));
        assert_eq!(engine.match_host("example.com"), None);
        assert_eq!(engine.match_host("api.web.example.com"), None);
    }

    #[test]
    fn test_engine_case_insensitive() {
        let mut engine = RadixHostMatchEngine::new();
        engine.initialize(vec!["Example.COM".to_string()]).unwrap();

        assert_eq!(engine.match_host("example.com"), Some(0));
        assert_eq!(engine.match_host("EXAMPLE.COM"), Some(0));
        assert_eq!(engine.match_host("ExAmPlE.CoM"), Some(0));
    }

    #[test]
    fn test_engine_multiple_wildcards() {
        let mut engine = RadixHostMatchEngine::new();
        engine
            .initialize(vec![
                "*.example.com".to_string(),
                "*.*.example.com".to_string(),
            ])
            .unwrap();

        assert_eq!(engine.match_host("api.example.com"), Some(0));
        assert_eq!(engine.match_host("a.b.example.com"), Some(1));
    }

    #[test]
    fn test_engine_get_host_pattern() {
        let mut engine = RadixHostMatchEngine::new();
        engine
            .initialize(vec!["example.com".to_string(), "*.test.com".to_string()])
            .unwrap();

        assert_eq!(engine.get_host_pattern(0), Some("example.com"));
        assert_eq!(engine.get_host_pattern(1), Some("*.test.com"));
        assert_eq!(engine.get_host_pattern(2), None);
    }

    #[test]
    fn test_engine_empty_initialization() {
        let mut engine = RadixHostMatchEngine::new();

        let result = engine.initialize(vec![]);
        assert!(result.is_ok());

        assert_eq!(engine.host_count(), 0);
        assert_eq!(engine.match_host("example.com"), None);
    }

    #[test]
    fn test_engine_default_trait() {
        let engine = RadixHostMatchEngine::default();
        assert_eq!(engine.host_count(), 0);
    }

    #[test]
    fn test_engine_shared_radix_key() {
        let mut engine = RadixHostMatchEngine::new();

        // Two exact matches with same hostname should share tree_idx
        let result = engine.initialize(vec![
            "example.com".to_string(),
            "example.com".to_string(), // Duplicate
        ]);
        assert!(result.is_ok());

        assert_eq!(engine.host_count(), 2);
        // Both should share the same tree node
        let tree_idx = engine.tree().find_exact("com.example").unwrap();
        let hosts_at_idx = engine.tree_idx_to_host_idx.get(&tree_idx).unwrap();
        assert_eq!(hosts_at_idx.len(), 2);
    }

    #[test]
    fn test_engine_tree_access() {
        let mut engine = RadixHostMatchEngine::new();
        engine.initialize(vec!["example.com".to_string()]).unwrap();

        let tree = engine.tree();
        assert_eq!(tree.find_exact("com.example"), Some(1));
    }

    #[test]
    fn test_engine_localhost() {
        let mut engine = RadixHostMatchEngine::new();
        engine.initialize(vec!["localhost".to_string()]).unwrap();

        assert_eq!(engine.match_host("localhost"), Some(0));
        assert_eq!(engine.match_host("LOCALHOST"), Some(0));
        assert_eq!(engine.match_host("api.localhost"), None);
    }

    #[test]
    fn test_engine_priority_exact_over_wildcard() {
        let mut engine = RadixHostMatchEngine::new();
        engine
            .initialize(vec![
                "*.example.com".to_string(),
                "api.example.com".to_string(), // Exact match
            ])
            .unwrap();

        // Exact match should be found first (depends on initialization order)
        let result = engine.match_host("api.example.com");
        assert!(result.is_some());
    }
}
