use crate::lb::compiled_selector;
use crate::lb::runtime_state;
use crate::lb::EdgionBackend;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Common trait for round-robin stores used by endpoint and endpoint_slice handlers.
pub(in crate::backends::discovery) trait RoundRobinStoreOps {
    /// Return the backend set from the data layer.
    fn get_backends_for_service(&self, service_key: &str) -> Vec<EdgionBackend>;
}

impl<S> RoundRobinStoreOps for super::endpoint_slice::EpSliceStore<S>
where
    S: pingora_load_balancing::selection::BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    fn get_backends_for_service(&self, service_key: &str) -> Vec<EdgionBackend> {
        self.get_backends_for_service(service_key)
    }
}

// ---------------------------------------------------------------------------
// Shared free functions
// ---------------------------------------------------------------------------

/// Diff old vs new backend address sets, remove stale entries from runtime
/// state, and invalidate selector caches.
///
/// Uses the cell's pre-replace snapshot to look up the live `Arc<EdgionBackend>`
/// for each removed address; if the backend still has in-flight connections
/// (`conn() > 0`) it is marked draining rather than removed immediately.
pub(in crate::backends::discovery) fn cleanup_removed_backends(
    service_key: &str,
    old: &HashSet<SocketAddr>,
    new: &HashSet<SocketAddr>,
    old_cell_snapshot: &[Arc<EdgionBackend>],
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
        let mut old_backends_by_addr = HashMap::with_capacity(old_cell_snapshot.len());
        for backend in old_cell_snapshot {
            old_backends_by_addr
                .entry(backend.inner().addr.clone())
                .or_insert(backend);
        }
        let cb_store = crate::backends::get_outlier_detection_store();
        for addr in &removed {
            // Look up the live Arc<EdgionBackend> from the pre-replace snapshot
            // to read the in-flight conn_count directly from the atomic.
            let live = old_backends_by_addr.get(addr).copied();
            let in_flight = live.map_or(0, |b| b.conn());
            if in_flight > 0 {
                // live.is_some() is guaranteed here: in_flight = live.map_or(0, ...) > 0
                runtime_state::begin_draining(
                    service_key,
                    addr,
                    live.expect("live is Some when in_flight > 0 (guarded by map_or above)")
                        .clone(),
                );
            } else {
                runtime_state::remove_backend(service_key, addr);
            }
            cb_store.remove_backend(service_key, addr);
        }
    }
}

/// Push fresh backend lists from the data layer into every cell that this store
/// knows about. Called after `full_set` to ensure all known services are warm.
///
/// Always calls `replace_backends` even when the new list is empty — this
/// correctly clears stale backends on scale-to-zero.
///
/// Also runs `cleanup_removed_backends` to register draining for any backends
/// with in-flight connections, matching the behaviour of `update_affected_lbs`.
pub(in crate::backends::discovery) fn update_all_existing_lbs<S: RoundRobinStoreOps>(
    store: &S,
    service_keys: &HashSet<String>,
) {
    for service_key in service_keys {
        let new_backends = store.get_backends_for_service(service_key);
        let new_addrs: HashSet<SocketAddr> = new_backends.iter().map(|b| b.inner().addr.clone()).collect();

        let cell = compiled_selector::get_or_create_cell(service_key);
        let old_cell_snapshot = cell.backends_snapshot();
        let old_addrs: HashSet<SocketAddr> = old_cell_snapshot.iter().map(|b| b.inner().addr.clone()).collect();

        cleanup_removed_backends(service_key, &old_addrs, &new_addrs, &old_cell_snapshot);
        cell.replace_backends(new_backends);
    }
}

/// Sync cells and clean up removed backends for `affected_services`.
///
/// Uses the cell's pre-replace snapshot as the "old" address set so that
/// `cleanup_removed_backends` can read live conn counts from the
/// `Arc<EdgionBackend>` objects still held by in-flight requests.
pub(in crate::backends::discovery) fn update_affected_lbs<S: RoundRobinStoreOps>(
    store: &S,
    affected_services: &HashSet<String>,
) {
    for service_key in affected_services {
        let new_backends = store.get_backends_for_service(service_key);
        let new_addrs: HashSet<SocketAddr> = new_backends.iter().map(|b| b.inner().addr.clone()).collect();

        // Pre-replace snapshot: empty on first sync (no cell yet), which
        // means old_addrs == empty → removed == empty → no cleanup needed,
        // which is the correct behaviour for a first-time sync.
        let cell = compiled_selector::get_or_create_cell(service_key);
        let old_cell_snapshot = cell.backends_snapshot();
        let old_addrs: HashSet<SocketAddr> = old_cell_snapshot.iter().map(|b| b.inner().addr.clone()).collect();

        cleanup_removed_backends(service_key, &old_addrs, &new_addrs, &old_cell_snapshot);

        cell.replace_backends(new_backends);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pingora_load_balancing::Backend;

    fn backend(addr: &str) -> Arc<EdgionBackend> {
        Arc::new(EdgionBackend::new(Backend::new(addr).unwrap()))
    }

    fn draining_backend(service_key: &str) -> Option<Arc<EdgionBackend>> {
        let key_prefix = format!("{service_key}\0");
        runtime_state::all_draining_backends()
            .into_iter()
            .find_map(|(key, backend)| key.starts_with(&key_prefix).then_some(backend))
    }

    #[test]
    fn cleanup_uses_first_snapshot_backend_for_duplicate_address() {
        let service_key = "test-cleanup-first-duplicate";
        let first = backend("127.0.0.1:18080");
        let second = backend("127.0.0.1:18080");
        first.inc_conn();
        let addr = first.inner().addr.clone();
        let old = HashSet::from([addr]);

        cleanup_removed_backends(service_key, &old, &HashSet::new(), &[first.clone(), second]);

        let draining = draining_backend(service_key).expect("first duplicate with an in-flight connection must drain");
        assert!(Arc::ptr_eq(&draining, &first));
        runtime_state::remove_service(service_key);
    }

    #[test]
    fn cleanup_treats_missing_snapshot_backend_as_zero_inflight() {
        let service_key = "test-cleanup-missing-snapshot";
        let stale = backend("127.0.0.1:18081");
        let addr = stale.inner().addr.clone();
        runtime_state::begin_draining(service_key, &addr, stale);
        let old = HashSet::from([addr]);

        cleanup_removed_backends(service_key, &old, &HashSet::new(), &[]);

        assert!(draining_backend(service_key).is_none());
        runtime_state::remove_service(service_key);
    }
}
