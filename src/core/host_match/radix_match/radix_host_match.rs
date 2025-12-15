use super::radix_host::RadixHost;
use crate::core::routes::radix_match::{RadixTreeBuilder, RadixTree, RouterError};
use std::collections::HashMap;
use std::sync::Arc;

/// Radix tree based hostname matching engine
///
/// This engine uses a radix tree for efficient hostname matching with reversed hostnames.
/// Hostnames are reversed (e.g., "api.example.com" -> "com.example.api") to enable
/// longest prefix matching from the TLD down.
///
/// **Lock-free concurrent reads**: The tree is immutable after initialization,
/// enabling true concurrent reads without any mutex contention.
///
/// Supports wildcard patterns like "*.example.com" which become "com.example" (radix_key)
/// with wildcard_count tracking.
pub struct RadixHostMatchEngine<T> {
    tree: RadixTree,
    /// All RadixHost instances
    hosts: Vec<RadixHost<T>>,
    /// Mapping from tree value to list of host_idx
    /// tree_value -> Vec<host_idx> (indices in hosts)
    tree_value_to_host_idx: HashMap<u32, Vec<usize>>,
}

impl<T> RadixHostMatchEngine<T> {
    pub fn new() -> Self {
        // Create an empty frozen tree
        let builder = RadixTreeBuilder::new();
        let tree = builder.freeze().expect("Failed to create empty radix tree");

        Self {
            tree,
            hosts: Vec::new(),
            tree_value_to_host_idx: HashMap::new(),
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

        tracing::trace!("========== Radix Host Matching ==========");
        tracing::trace!("Request Hostname: '{}', Reversed: '{}', Available hosts: {}",
            hostname, reversed, self.hosts.len());

        // Get all matching prefixes (returns shortest to longest)
        tracing::trace!("[Step 1] Searching in radix tree...");
        let all_values = self.tree.match_all_prefixes(&reversed);

        if all_values.is_empty() {
            tracing::trace!("No match found in radix tree");
            return None;
        }

        tracing::trace!("Found {} value(s), checking from longest to shortest...", all_values.len());

        // Iterate from longest to shortest (reverse order since API returns shortest to longest)
        let mut match_count = 0;
        for &tree_value in all_values.iter().rev() {
            match_count += 1;
            tracing::trace!("  [Match #{}] Checking tree value: {}", match_count, tree_value);

            if let Some(host_indices) = self.tree_value_to_host_idx.get(&tree_value) {
                tracing::trace!("    -> {} host(s) for this value", host_indices.len());
                for &host_idx in host_indices {
                    if let Some(radix_host) = self.hosts.get(host_idx) {
                        tracing::trace!(
                            "      Testing: original='{}', radix_key='{}', is_wildcard={}",
                            radix_host.original, radix_host.radix_key, radix_host.is_wildcard
                        );
                        if radix_host.matches(hostname) {
                            tracing::trace!("      Matched!");
                            return Some(radix_host.runtime.clone());
                        } else {
                            tracing::trace!("      Pattern did not match");
                        }
                    }
                }
            }
        }

        if match_count == 0 {
            tracing::trace!("No matches found");
        } else {
            tracing::trace!("Checked {} match(es), none matched", match_count);
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
        tracing::debug!("========== RadixHostMatchEngine Initialize ==========");
        tracing::debug!("Total hosts: {}", hosts.len());

        let mut builder = RadixTreeBuilder::new();
        let mut next_tree_value = 1usize;
        let mut radix_key_to_value: HashMap<String, usize> = HashMap::new();

        for radix_host in hosts {
            tracing::debug!(
                "  [Host] pattern='{}', radix_key='{}', is_wildcard={}, wildcard_count={}",
                radix_host.original, radix_host.radix_key, radix_host.is_wildcard, radix_host.wildcard_count
            );

            let radix_key = radix_host.radix_key.clone();

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
                    format!(
                        "Failed to insert radix key '{}' for pattern '{}' into radix tree: {}",
                        radix_key, radix_host.original, e
                    )
                })?;

                radix_key_to_value.insert(radix_key.clone(), new_value);
                tracing::debug!("    Inserted radix_key: '{}' -> tree value: {}", radix_key, new_value);
                next_tree_value += 1;
                new_value
            };

            // Add RadixHost to the list
            let host_idx = self.hosts.len();
            self.hosts.push(radix_host);

            // Map tree_value to host_idx (append to list)
            self.tree_value_to_host_idx
                .entry(tree_value as u32)
                .or_insert_with(Vec::new)
                .push(host_idx);
        }

        // Freeze the builder to create the immutable tree
        tracing::debug!("Freezing radix tree...");
        self.tree = builder.freeze().map_err(|e: RouterError| {
            format!("Failed to freeze radix tree: {}", e)
        })?;

        tracing::debug!("========== Initialization Complete ==========");
        tracing::debug!(
            "Summary: Total hosts: {}, Unique radix tree nodes: {}",
            self.hosts.len(),
            self.tree_value_to_host_idx.len()
        );
        tracing::debug!("==============================================");
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
            Self { name: name.to_string() }
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
        assert_eq!(engine.tree_value_to_host_idx.len(), 1);
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
        assert_eq!(engine.tree_value_to_host_idx.len(), 3);
    }

    #[test]
    fn test_engine_exact_match() {
        let mut engine: RadixHostMatchEngine<MockHostRuntime> = RadixHostMatchEngine::new();
        let runtime = Arc::new(MockHostRuntime::new("test", vec!["example.com", "api.example.com"]));

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
        let runtime = Arc::new(MockHostRuntime::new("test", vec!["*.example.com", "*.*.example.com"]));

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
        assert_eq!(tree.match_exact("com.example"), Some(&[1u32][..]));
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
        assert_eq!(engine.tree_value_to_host_idx.len(), 3); // example.com and *.example.com share same radix_key

        // Test exact match_engine for example.com (should match_engine runtime1, not wildcard runtime4)
        let r1 = engine.match_host("example.com");
        assert!(r1.is_some());
        assert_eq!(r1.unwrap().identifier(), "runtime1");

        // Test wildcard match_engine for *.example.com (should match_engine runtime4)
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

        // Test exact match_engine for test.com
        let r3 = engine.match_host("test.com");
        assert!(r3.is_some());
        assert_eq!(r3.unwrap().identifier(), "runtime3");

        // Test that *.example.com doesn't match_engine too many levels
        let r_none = engine.match_host("sub.api.example.com");
        assert!(r_none.is_none());
    }
}