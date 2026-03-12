//! Global policy store for optional load balancing algorithms
//!
//! This module maintains a mapping of service keys to their configured LB policies.

use super::types::LbPolicy;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Global policy store
static POLICY_STORE: LazyLock<PolicyStore> = LazyLock::new(PolicyStore::default);

/// Get global policy store instance
pub fn get_global_policy_store() -> &'static PolicyStore {
    &POLICY_STORE
}

/// Load balancing policy configuration for a service
///
/// Tracks both the policies and which HTTPRoutes are referencing them.
/// When the last route reference is removed, the entire entry is cleaned up.
#[derive(Debug, Clone)]
pub struct ServiceLbPolicy {
    /// The LB policies for this service
    policies: Vec<LbPolicy>,
    /// Set of HTTPRoute resource keys that reference this service
    /// Format: "namespace/route-name"
    route_refs: HashSet<String>,
}

impl ServiceLbPolicy {
    /// Create new policy with initial route reference
    pub fn new(policies: Vec<LbPolicy>, route_key: String) -> Self {
        let mut route_refs = HashSet::new();
        route_refs.insert(route_key);
        Self { policies, route_refs }
    }

    /// Add a route reference and merge policies
    pub fn add_route_ref(&mut self, route_key: String, new_policies: Vec<LbPolicy>) {
        self.route_refs.insert(route_key);

        // Merge new policies (dedup)
        for policy in new_policies {
            if !self.policies.contains(&policy) {
                self.policies.push(policy);
            }
        }
    }

    /// Remove a route reference
    /// Returns true if this was the last reference (entry should be deleted)
    pub fn remove_route_ref(&mut self, route_key: &str) -> bool {
        self.route_refs.remove(route_key);
        self.route_refs.is_empty()
    }

    /// Get the policies
    pub fn policies(&self) -> &[LbPolicy] {
        &self.policies
    }

    /// Get the number of route references
    pub fn ref_count(&self) -> usize {
        self.route_refs.len()
    }
}

/// Store for service-level LB policy configurations
///
/// This store maintains a mapping from service keys (namespace/service-name)
/// to their configured load balancing policies with lifecycle tracking.
///
/// Uses DashMap for simple concurrent access without complex RCU patterns.
pub struct PolicyStore {
    policies: DashMap<String, ServiceLbPolicy>,
}

impl PolicyStore {
    /// Create a new empty policy store
    pub fn new() -> Self {
        Self {
            policies: DashMap::new(),
        }
    }

    /// Get policies for a service
    ///
    /// # Arguments
    /// * `service_key` - The service key (format: "namespace/service-name")
    ///
    /// # Returns
    /// * `Vec<LbPolicy>` - List of policies for the service (empty if not configured)
    pub fn get(&self, service_key: &str) -> Vec<LbPolicy> {
        self.policies
            .get(service_key)
            .map(|entry| entry.value().policies().to_vec())
            .unwrap_or_default()
    }

    /// Add or update policies for a service with route reference tracking
    ///
    /// # Arguments
    /// * `service_key` - The service key (format: "namespace/service-name")
    /// * `route_key` - The HTTPRoute resource key (format: "namespace/route-name")
    /// * `policies` - List of policies to add
    ///
    /// # Returns
    /// * `bool` - true if there was a change (new key or new policies added), false otherwise
    fn add_policy(&self, service_key: String, route_key: String, policies: Vec<LbPolicy>) -> bool {
        if policies.is_empty() {
            return false;
        }

        let mut changed = false;

        self.policies
            .entry(service_key.clone())
            .and_modify(|entry| {
                let old_policies_len = entry.policies.len();
                entry.add_route_ref(route_key.clone(), policies.clone());
                // Check if any new policies were added
                if entry.policies.len() > old_policies_len {
                    changed = true;
                }
            })
            .or_insert_with(|| {
                // New service key, this is a change
                changed = true;
                ServiceLbPolicy::new(policies.clone(), route_key.clone())
            });

        tracing::debug!(
            service_key = %service_key,
            route_key = %route_key,
            policies = ?policies,
            changed = changed,
            "Added LB policies for service"
        );

        changed
    }

    /// Batch add policies for multiple services from a single route
    ///
    /// # Arguments
    /// * `route_key` - The HTTPRoute resource key
    /// * `updates` - HashMap of service keys to their policies
    pub fn batch_add(&self, route_key: String, updates: HashMap<String, Vec<LbPolicy>>) {
        let service_count = updates.len();
        let mut changed_services = Vec::new();

        for (service_key, policies) in updates {
            if !policies.is_empty() {
                let changed = self.add_policy(service_key.clone(), route_key.clone(), policies);
                if changed {
                    changed_services.push(service_key);
                }
            }
        }

        // Trigger endpoint slice update events for changed services
        if !changed_services.is_empty() {
            if let Some(config_client) = crate::core::gateway::conf_sync::get_global_config_client() {
                for service_key in &changed_services {
                    config_client.trigger_endpoint_slice_update_event(service_key);
                    tracing::debug!(
                        service_key = %service_key,
                        route_key = %route_key,
                        "Triggered endpoint slice update event for LB policy change"
                    );
                }
            } else {
                tracing::warn!(
                    changed_services = ?changed_services,
                    "Cannot trigger endpoint slice updates: global config client not initialized"
                );
            }
        }

        tracing::debug!(
            route_key = %route_key,
            services = service_count,
            changed_services = changed_services.len(),
            "Batch added LB policies"
        );
    }

    /// Remove all policy references for a specific HTTPRoute
    ///
    /// Cleans up entries where this was the last route reference.
    ///
    /// # Arguments
    /// * `route_key` - The HTTPRoute resource key to remove
    pub fn remove_by_route(&self, route_key: &str) {
        let mut removed_services = Vec::new();

        // Iterate through all entries and remove this route's references
        self.policies.retain(|service_key, policy| {
            let mut entry = policy.clone();
            let should_delete = entry.remove_route_ref(route_key);

            if should_delete {
                removed_services.push(service_key.clone());
                false // Remove from map
            } else {
                // Update the entry with modified route_refs
                *policy = entry;
                true // Keep in map
            }
        });

        if !removed_services.is_empty() {
            tracing::info!(
                route_key = %route_key,
                removed_services = ?removed_services,
                "Removed LB policies for route"
            );
        } else {
            tracing::debug!(
                route_key = %route_key,
                "No LB policies to remove for route"
            );
        }
    }

    /// Batch remove policies for multiple routes
    ///
    /// # Arguments
    /// * `route_keys` - List of HTTPRoute resource keys to remove
    pub fn batch_remove_routes(&self, route_keys: &[String]) {
        for route_key in route_keys {
            self.remove_by_route(route_key);
        }
    }

    /// Delete all policy references for a specific resource key (HTTPRoute)
    ///
    /// This is an alias for `remove_by_route` with a more explicit name.
    /// Cleans up entries where this was the last route reference.
    ///
    /// # Arguments
    /// * `resource_key` - The HTTPRoute resource key to delete (format: "namespace/route-name")
    ///
    /// # Example
    /// ```
    /// use edgion::core::gateway::lb::lb_policy::get_global_policy_store;
    ///
    /// let store = get_global_policy_store();
    /// store.delete_lb_policies_by_resource_key("default/my-route");
    /// ```
    pub fn delete_lb_policies_by_resource_key(&self, resource_key: &str) {
        self.remove_by_route(resource_key);
        tracing::info!(
            resource_key = %resource_key,
            "Deleted LB policies by resource key"
        );
    }

    /// Get all configured services
    pub fn all_services(&self) -> Vec<String> {
        self.policies.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Get statistics about the policy store
    pub fn stats(&self) -> PolicyStoreStats {
        let total_services = self.policies.len();
        let mut total_route_refs = 0;

        for entry in self.policies.iter() {
            total_route_refs += entry.value().ref_count();
        }

        PolicyStoreStats {
            total_services,
            total_route_refs,
        }
    }

    /// Clear all policies
    pub fn clear(&self) {
        self.policies.clear();
        tracing::info!("Cleared all LB policies");
    }
}

/// Statistics about the policy store
#[derive(Debug, Clone)]
pub struct PolicyStoreStats {
    /// Total number of services with policies
    pub total_services: usize,
    /// Total number of route references
    pub total_route_refs: usize,
}

impl Default for PolicyStore {
    fn default() -> Self {
        Self::new()
    }
}
