//! Global store for BackendTLSPolicy resources
//!
//! This module provides a thread-safe store for BackendTLSPolicy resources
//! with O(1) reverse lookup capability and efficient incremental updates.
//!
//! ## Performance Optimization
//!
//! The store maintains two data structures:
//! 1. **Primary map**: `namespace/name -> BackendTLSPolicy` for direct lookups (RwLock)
//! 2. **Reverse index**: `namespace/group/kind/name -> Vec<BackendTLSPolicy>` for target lookups (ArcSwap)
//!
//! The reverse index enables O(1) lookup when finding policies that apply to a specific
//! Service, instead of O(n) iteration over all policies. Policies in the reverse index
//! are pre-sorted by Gateway API precedence rules (oldest creation timestamp first,
//! then alphabetically by name).
//!
//! ## Thread Safety
//!
//! - **policies**: Protected by RwLock for concurrent reads, exclusive writes
//! - **reverse_index**: Uses ArcSwap for lock-free atomic updates
//! - Query path (`get_policies_for_target`) is completely lock-free
//!
//! ## Update Strategy
//!
//! - **Full replacement** (`replace_all`): Rebuilds entire reverse index
//! - **Incremental update** (`update`): Only rebuilds affected target entries

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::{Arc, RwLock};

use crate::types::resources::BackendTLSPolicy;

static GLOBAL_BACKEND_TLS_POLICY_STORE: LazyLock<Arc<BackendTLSPolicyStore>> =
    LazyLock::new(|| Arc::new(BackendTLSPolicyStore::new()));

pub fn get_global_backend_tls_policy_store() -> Arc<BackendTLSPolicyStore> {
    GLOBAL_BACKEND_TLS_POLICY_STORE.clone()
}

/// Type alias for the backend TLS policy map (key: namespace/name)
type BackendTLSPolicyMap = HashMap<String, Arc<BackendTLSPolicy>>;

/// Type alias for the reverse index map (key: target identifier, value: list of policies sorted by priority)
type ReverseIndexMap = HashMap<String, Vec<Arc<BackendTLSPolicy>>>;

pub struct BackendTLSPolicyStore {
    /// Main policy map (namespace/name -> policy), protected by RwLock
    policies: RwLock<BackendTLSPolicyMap>,
    /// Reverse index (target identifier -> sorted policies) for O(1) query
    /// Key format: "namespace/group/kind/name"
    reverse_index: ArcSwap<ReverseIndexMap>,
}

impl BackendTLSPolicyStore {
    pub fn new() -> Self {
        Self {
            policies: RwLock::new(HashMap::new()),
            reverse_index: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Sort policies by Gateway API precedence rules
    ///
    /// 1. Older creation timestamp first
    /// 2. Alphabetically by name (on ties)
    #[inline]
    fn sort_policies_by_precedence(policies: &mut [Arc<BackendTLSPolicy>]) {
        policies.sort_by(|a, b| {
            // 1. Sort by creation timestamp (oldest first)
            let ts_cmp = a.metadata.creation_timestamp.cmp(&b.metadata.creation_timestamp);
            if ts_cmp != std::cmp::Ordering::Equal {
                return ts_cmp;
            }

            // 2. Sort alphabetically by name (on ties)
            let name_a = a.metadata.name.as_deref().unwrap_or("");
            let name_b = b.metadata.name.as_deref().unwrap_or("");
            name_a.cmp(name_b)
        });
    }

    /// Build reverse index from policies
    ///
    /// Creates a map from target identifier to sorted list of policies
    fn build_reverse_index(policies: &BackendTLSPolicyMap) -> ReverseIndexMap {
        let mut index: HashMap<String, Vec<Arc<BackendTLSPolicy>>> = HashMap::new();

        // Build initial index
        for policy in policies.values() {
            let policy_namespace = policy.namespace().unwrap_or("");

            for target_ref in &policy.spec.target_refs {
                let target_key = Self::build_target_key(policy_namespace, target_ref);

                index.entry(target_key).or_default().push(policy.clone());
            }
        }

        // Sort each policy list by Gateway API precedence rules
        for policies_list in index.values_mut() {
            Self::sort_policies_by_precedence(policies_list);
        }

        index
    }

    /// Build a target key from target reference components
    /// Key format: namespace/name
    /// Per Gateway API spec, targetRef can only reference resources in the same namespace as the policy.
    #[inline]
    fn build_target_key(
        policy_namespace: &str,
        target_ref: &crate::types::resources::backend_tls_policy::BackendTLSPolicyTargetRef,
    ) -> String {
        let target_name = &target_ref.name;
        format!("{}/{}", policy_namespace, target_name)
    }

    /// Extract all target keys from a policy
    fn extract_target_keys(policy: &BackendTLSPolicy) -> Vec<String> {
        let policy_namespace = policy.namespace().unwrap_or("");
        policy
            .spec
            .target_refs
            .iter()
            .map(|target_ref| Self::build_target_key(policy_namespace, target_ref))
            .collect()
    }

    /// Check if a policy matches a given target key
    fn policy_matches_target(policy: &BackendTLSPolicy, target_key: &str) -> bool {
        let policy_namespace = policy.namespace().unwrap_or("");

        policy
            .spec
            .target_refs
            .iter()
            .any(|target_ref| Self::build_target_key(policy_namespace, target_ref) == target_key)
    }

    /// Check if a backend TLS policy exists
    pub fn contains(&self, key: &str) -> bool {
        self.policies
            .read()
            .map(|policies| policies.contains_key(key))
            .unwrap_or(false)
    }

    /// Get a backend TLS policy by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<Arc<BackendTLSPolicy>> {
        self.policies
            .read()
            .ok()
            .and_then(|policies| policies.get(key).cloned())
    }

    /// Get a backend TLS policy by namespace and name
    pub fn get_by_ns_name(&self, namespace: &str, name: &str) -> Option<Arc<BackendTLSPolicy>> {
        let key = format!("{}/{}", namespace, name);
        self.get(&key)
    }

    /// Replace all backend TLS policies atomically
    pub fn replace_all(&self, new_policies: HashMap<String, Arc<BackendTLSPolicy>>) {
        // Update policies map and build reverse_index under write lock to prevent race conditions
        let reverse_index = {
            let mut policies_write = self.policies.write().unwrap();
            *policies_write = new_policies;

            // Build reverse_index while holding write lock to ensure consistency
            Self::build_reverse_index(&policies_write)
        }; // Write lock released here

        // Atomic swap of reverse index
        self.reverse_index.store(Arc::new(reverse_index));
    }

    /// Update backend TLS policies with intelligent index rebuild strategy
    ///
    /// Uses incremental rebuild when few targets are affected, otherwise full rebuild
    pub fn update(&self, add_or_update: HashMap<String, Arc<BackendTLSPolicy>>, remove: &HashSet<String>) {
        // Early return if no changes
        if add_or_update.is_empty() && remove.is_empty() {
            return;
        }

        // Perform all operations under a single write lock to prevent race conditions
        let new_index = {
            let mut policies_write = self.policies.write().unwrap();

            // Step 1: Collect affected target keys (under write lock protection)
            let mut affected_targets = HashSet::new();

            // 1.1 Collect targets from policies being removed
            for key in remove.iter() {
                if let Some(old_policy) = policies_write.get(key) {
                    affected_targets.extend(Self::extract_target_keys(old_policy));
                }
            }

            // 1.2 Collect targets from policies being added/updated (both old and new)
            for (key, new_policy) in &add_or_update {
                // If updating, collect targets from old policy
                if let Some(old_policy) = policies_write.get(key) {
                    affected_targets.extend(Self::extract_target_keys(old_policy));
                }
                // Collect targets from new policy
                affected_targets.extend(Self::extract_target_keys(new_policy));
            }

            // Early return if no affected targets (shouldn't happen, but be safe)
            if affected_targets.is_empty() {
                tracing::warn!("Update called with changes but no affected targets");
                return;
            }

            // Step 2: Update policies map
            for (key, policy) in add_or_update {
                policies_write.insert(key, policy);
            }
            for key in remove {
                policies_write.remove(key);
            }

            // Step 3: Choose rebuild strategy based on affected targets
            let total_policies = policies_write.len();
            let affected_count = affected_targets.len();

            // Use incremental update only if it's beneficial
            // Heuristic: incremental is O(n*m), full rebuild is O(m)
            // Use incremental when n < m/10
            let use_incremental = affected_count < total_policies.max(10) / 10;

            if use_incremental {
                // Incremental update: only rebuild affected targets
                let current_index = self.reverse_index.load();
                let mut new_index = (**current_index).clone();

                for target_key in &affected_targets {
                    let mut target_policies = Vec::new();

                    // Find all policies that match this target
                    for policy in policies_write.values() {
                        if Self::policy_matches_target(policy, target_key) {
                            target_policies.push(policy.clone());
                        }
                    }

                    if target_policies.is_empty() {
                        // No policies for this target, remove the entry
                        new_index.remove(target_key);
                    } else {
                        // Sort by Gateway API precedence rules
                        Self::sort_policies_by_precedence(&mut target_policies);
                        new_index.insert(target_key.clone(), target_policies);
                    }
                }

                tracing::debug!(
                    affected_targets = affected_count,
                    total_policies = total_policies,
                    strategy = "incremental",
                    "Update completed"
                );

                new_index
            } else {
                // Full rebuild is more efficient
                tracing::debug!(
                    affected_targets = affected_count,
                    total_policies = total_policies,
                    strategy = "full_rebuild",
                    "Update completed (using full rebuild)"
                );

                Self::build_reverse_index(&policies_write)
            }
        }; // Write lock released here

        // Atomic swap of reverse index (after releasing write lock)
        self.reverse_index.store(Arc::new(new_index));
    }

    /// Get all policies
    pub fn get_all(&self) -> HashMap<String, Arc<BackendTLSPolicy>> {
        self.policies
            .read()
            .map(|policies| policies.clone())
            .unwrap_or_default()
    }

    /// Get all policies in a specific namespace
    pub fn get_by_namespace(&self, namespace: &str) -> Vec<Arc<BackendTLSPolicy>> {
        self.policies
            .read()
            .map(|policies| {
                let prefix = format!("{}/", namespace);
                policies
                    .iter()
                    .filter(|(key, _)| key.starts_with(&prefix))
                    .map(|(_, policy)| policy.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get policies that apply to a given target (O(1) lookup using reverse index)
    ///
    /// Returns policies sorted by Gateway API precedence rules:
    /// 1. Older creation timestamp first
    /// 2. Alphabetically by name (on ties)
    pub fn get_policies_for_target(&self, name: &str, namespace: Option<&str>) -> Vec<Arc<BackendTLSPolicy>> {
        let index = self.reverse_index.load();

        // Build target key (namespace/name)
        let target_namespace = namespace.unwrap_or("");
        let target_key = format!("{}/{}", target_namespace, name);

        // O(1) lookup in reverse index
        index.get(&target_key).cloned().unwrap_or_default()
    }

    /// Count of policies
    pub fn count(&self) -> usize {
        self.policies.read().map(|policies| policies.len()).unwrap_or(0)
    }

    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> BackendTLSPolicyStoreStats {
        let policies_count = self.policies.read().map(|p| p.len()).unwrap_or(0);
        let reverse_index = self.reverse_index.load();
        BackendTLSPolicyStoreStats {
            policies: policies_count,
            reverse_index_targets: reverse_index.len(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendTLSPolicyStoreStats {
    pub policies: usize,
    pub reverse_index_targets: usize,
}

impl Default for BackendTLSPolicyStore {
    fn default() -> Self {
        Self::new()
    }
}
