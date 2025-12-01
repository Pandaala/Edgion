//! Global policy store for optional load balancing algorithms
//!
//! This module maintains a mapping of service keys to their configured LB policies.

use std::collections::{HashMap, HashSet};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use super::types::LbPolicy;

/// Global policy store
static POLICY_STORE: Lazy<PolicyStore> = Lazy::new(PolicyStore::default);

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
        Self {
            policies,
            route_refs,
        }
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
        self.policies.get(service_key)
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
        
        self.policies.entry(service_key.clone())
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
            if let Some(config_client) = crate::core::conf_sync::get_global_config_client() {
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
    /// use edgion::core::lb::optional_lb::get_global_policy_store;
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
        self.policies.iter()
            .map(|entry| entry.key().clone())
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_store_get_empty() {
        let store = PolicyStore::new();
        let policies = store.get("default/test-service");
        assert!(policies.is_empty());
    }
    
    #[test]
    fn test_policy_store_add_policy() {
        let store = PolicyStore::new();
        
        let policies = vec![LbPolicy::Ketama, LbPolicy::FnvHash];
        store.add_policy(
            "default/test-service".to_string(),
            "default/route1".to_string(),
            policies.clone()
        );
        
        let retrieved = store.get("default/test-service");
        assert_eq!(retrieved.len(), 2);
        assert!(retrieved.contains(&LbPolicy::Ketama));
        assert!(retrieved.contains(&LbPolicy::FnvHash));
    }
    
    #[test]
    fn test_policy_store_multiple_routes() {
        let store = PolicyStore::new();
        
        // Route 1 adds Ketama
        store.add_policy(
            "default/test-service".to_string(),
            "default/route1".to_string(),
            vec![LbPolicy::Ketama]
        );
        
        // Route 2 adds FnvHash
        store.add_policy(
            "default/test-service".to_string(),
            "default/route2".to_string(),
            vec![LbPolicy::FnvHash]
        );
        
        // Should have both policies
        let policies = store.get("default/test-service");
        assert_eq!(policies.len(), 2);
        assert!(policies.contains(&LbPolicy::Ketama));
        assert!(policies.contains(&LbPolicy::FnvHash));
        
        // Stats should show 1 service, 2 route refs
        let stats = store.stats();
        assert_eq!(stats.total_services, 1);
        assert_eq!(stats.total_route_refs, 2);
    }
    
    #[test]
    fn test_policy_store_remove_by_route() {
        let store = PolicyStore::new();
        
        // Add policies from two routes
        store.add_policy(
            "default/test-service".to_string(),
            "default/route1".to_string(),
            vec![LbPolicy::Ketama]
        );
        store.add_policy(
            "default/test-service".to_string(),
            "default/route2".to_string(),
            vec![LbPolicy::FnvHash]
        );
        
        // Remove route1
        store.remove_by_route("default/route1");
        
        // Service should still exist (route2 still references it)
        let policies = store.get("default/test-service");
        assert!(!policies.is_empty());
        
        // Remove route2
        store.remove_by_route("default/route2");
        
        // Service should be removed (no more references)
        let policies = store.get("default/test-service");
        assert!(policies.is_empty());
    }
    
    #[test]
    fn test_policy_store_batch_add() {
        let store = PolicyStore::new();
        
        let mut updates = HashMap::new();
        updates.insert("default/svc1".to_string(), vec![LbPolicy::Ketama]);
        updates.insert("default/svc2".to_string(), vec![LbPolicy::FnvHash, LbPolicy::LeastConnection]);
        
        store.batch_add("default/route1".to_string(), updates);
        
        assert_eq!(store.get("default/svc1"), vec![LbPolicy::Ketama]);
        assert_eq!(store.get("default/svc2").len(), 2);
        
        // Stats should show 2 services, 2 route refs (one route references both)
        let stats = store.stats();
        assert_eq!(stats.total_services, 2);
        assert_eq!(stats.total_route_refs, 2);
    }
    
    #[test]
    fn test_policy_store_clear() {
        let store = PolicyStore::new();
        
        store.add_policy(
            "default/svc1".to_string(),
            "default/route1".to_string(),
            vec![LbPolicy::Ketama]
        );
        store.add_policy(
            "default/svc2".to_string(),
            "default/route2".to_string(),
            vec![LbPolicy::FnvHash]
        );
        
        store.clear();
        
        assert!(store.get("default/svc1").is_empty());
        assert!(store.get("default/svc2").is_empty());
        
        let stats = store.stats();
        assert_eq!(stats.total_services, 0);
    }
    
    #[test]
    fn test_service_lb_policy_lifecycle() {
        let store = PolicyStore::new();
        
        // Add from route1
        store.add_policy(
            "default/service".to_string(),
            "default/route1".to_string(),
            vec![LbPolicy::Ketama]
        );
        
        // Add from route2 with overlapping policy
        store.add_policy(
            "default/service".to_string(),
            "default/route2".to_string(),
            vec![LbPolicy::Ketama, LbPolicy::FnvHash]
        );
        
        // Should have deduped policies
        let policies = store.get("default/service");
        assert_eq!(policies.len(), 2);
        
        // Remove route1 - service should still exist
        store.remove_by_route("default/route1");
        assert!(!store.get("default/service").is_empty());
        
        // Remove route2 - service should be cleaned up
        store.remove_by_route("default/route2");
        assert!(store.get("default/service").is_empty());
    }
    
    #[test]
    fn test_delete_lb_policies_by_resource_key() {
        let store = PolicyStore::new();
        
        // Add policies from multiple routes
        store.add_policy(
            "default/service1".to_string(),
            "default/route1".to_string(),
            vec![LbPolicy::Ketama]
        );
        store.add_policy(
            "default/service2".to_string(),
            "default/route1".to_string(),
            vec![LbPolicy::FnvHash]
        );
        store.add_policy(
            "default/service1".to_string(),
            "default/route2".to_string(),
            vec![LbPolicy::LeastConnection]
        );
        
        // Verify policies exist
        assert_eq!(store.get("default/service1").len(), 2);
        assert_eq!(store.get("default/service2").len(), 1);
        
        // Delete by resource key (route1)
        store.delete_lb_policies_by_resource_key("default/route1");
        
        // service1 should still exist (route2 still references it)
        let policies1 = store.get("default/service1");
        assert!(!policies1.is_empty());
        
        // service2 should be removed (only route1 referenced it)
        let policies2 = store.get("default/service2");
        assert!(policies2.is_empty());
        
        // Delete by resource key (route2)
        store.delete_lb_policies_by_resource_key("default/route2");
        
        // service1 should now be removed (no more references)
        let policies1 = store.get("default/service1");
        assert!(policies1.is_empty());
        
        // Stats should show 0 services
        let stats = store.stats();
        assert_eq!(stats.total_services, 0);
        assert_eq!(stats.total_route_refs, 0);
    }
}

