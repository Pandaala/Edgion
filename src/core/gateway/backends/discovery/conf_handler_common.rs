use crate::core::gateway::lb::runtime_state;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::Backend;
use std::collections::HashSet;

/// Common trait for round-robin stores used by endpoint and endpoint_slice handlers.
///
/// Both `EndpointStore<RoundRobin>` and `EpSliceStore<RoundRobin>` expose these
/// four methods with identical signatures, so we can share `update_all_existing_lbs`
/// and `update_affected_lbs` generically.
pub(in crate::core::gateway::backends::discovery) trait RoundRobinStoreOps {
    /// Return all service keys that currently have an active LoadBalancer.
    fn get_existing_service_keys(&self) -> Vec<String>;
    /// Update the LoadBalancer for `service_key` if one exists; remove it when
    /// the data layer no longer has data for that key.
    fn update_lb_if_exists(&self, service_key: &str);
    /// Return the backend set currently held by the existing LB (pre-update snapshot).
    fn get_backends_from_lb(&self, service_key: &str) -> Vec<Backend>;
    /// Return the backend set from the data layer (post-update view).
    fn get_backends_for_service(&self, service_key: &str) -> Vec<Backend>;
}

impl<S> RoundRobinStoreOps for super::endpoint::EndpointStore<S>
where
    S: pingora_load_balancing::selection::BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    fn get_existing_service_keys(&self) -> Vec<String> {
        self.get_existing_service_keys()
    }

    fn update_lb_if_exists(&self, service_key: &str) {
        self.update_lb_if_exists(service_key);
    }

    fn get_backends_from_lb(&self, service_key: &str) -> Vec<Backend> {
        self.get_backends_from_lb(service_key)
    }

    fn get_backends_for_service(&self, service_key: &str) -> Vec<Backend> {
        self.get_backends_for_service(service_key)
    }
}

impl<S> RoundRobinStoreOps for super::endpoint_slice::EpSliceStore<S>
where
    S: pingora_load_balancing::selection::BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    fn get_existing_service_keys(&self) -> Vec<String> {
        self.get_existing_service_keys()
    }

    fn update_lb_if_exists(&self, service_key: &str) {
        self.update_lb_if_exists(service_key);
    }

    fn get_backends_from_lb(&self, service_key: &str) -> Vec<Backend> {
        self.get_backends_from_lb(service_key)
    }

    fn get_backends_for_service(&self, service_key: &str) -> Vec<Backend> {
        self.get_backends_for_service(service_key)
    }
}

// ---------------------------------------------------------------------------
// Shared free functions
// ---------------------------------------------------------------------------

/// Diff old vs new backend address sets, remove stale entries from runtime
/// state, and invalidate selector caches.
///
/// The `added` variable that appeared in both original handlers was a dead
/// variable and has been removed; the cache is invalidated whenever the two
/// sets differ.
pub(in crate::core::gateway::backends::discovery) fn cleanup_removed_backends(
    service_key: &str,
    old: &HashSet<SocketAddr>,
    new: &HashSet<SocketAddr>,
) {
    let removed: Vec<SocketAddr> = old.difference(new).cloned().collect();

    if old != new {
        runtime_state::invalidate_selector_cache(service_key);
    }

    if !removed.is_empty() {
        tracing::debug!(
            service_key = %service_key,
            removed_count = removed.len(),
            "Cleaning stale backend runtime state"
        );
        let cb_store = crate::core::gateway::backends::get_circuit_breaker_store();
        for addr in &removed {
            if runtime_state::get_count(service_key, addr) > 0 {
                runtime_state::mark_backend_draining(service_key, addr);
            } else {
                runtime_state::remove_backend(service_key, addr);
            }
            cb_store.remove_backend(service_key, addr);
        }
    }
}

/// Refresh every LB that already exists in `store` from its data layer.
pub(in crate::core::gateway::backends::discovery) fn update_all_existing_lbs<S: RoundRobinStoreOps>(store: &S) {
    for service_key in store.get_existing_service_keys() {
        store.update_lb_if_exists(&service_key);
    }
}

/// Refresh LBs for `affected_services` and clean up any removed backends.
pub(in crate::core::gateway::backends::discovery) fn update_affected_lbs<S: RoundRobinStoreOps>(
    store: &S,
    affected_services: &HashSet<String>,
) {
    for service_key in affected_services {
        let old_addrs: HashSet<SocketAddr> = store
            .get_backends_from_lb(service_key)
            .into_iter()
            .map(|b| b.addr)
            .collect();
        store.update_lb_if_exists(service_key);
        let new_addrs: HashSet<SocketAddr> = store
            .get_backends_for_service(service_key)
            .into_iter()
            .map(|b| b.addr)
            .collect();
        cleanup_removed_backends(service_key, &old_addrs, &new_addrs);
    }
}
