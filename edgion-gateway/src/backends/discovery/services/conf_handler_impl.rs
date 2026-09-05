use super::{get_global_service_store, ServiceStore};
use crate::backends::discovery::get_global_dns_cache;
use crate::lb::compiled_selector;
use crate::lb::runtime_state;
use edgion_common::conf_sync::traits::{arc_map_into_owned, arc_map_to_owned, ConfHandler};
use k8s_openapi::api::core::v1::Service;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Create a ServiceStore handler for registration with ConfigClient
pub fn create_service_handler() -> Box<dyn ConfHandler<Service> + Send + Sync> {
    Box::new(get_global_service_store())
}

impl ConfHandler<Service> for ServiceStore {
    fn full_set(&self, data: &HashMap<String, Arc<Service>>) {
        tracing::info!(component = "service_store", cnt = data.len(), "full set");

        // Snapshot the previous key set before the swap so we can drop per-backend
        // runtime state and selector cells for Services removed by this full set.
        // Per-service policy config (health check / circuit breaker / degrade
        // threshold / LB policy) is owned exclusively by the EdgionBackendTrafficPolicy
        // conf-handler, so a Service resync must never touch those stores.
        let old_keys = self.keys();
        self.replace_all(arc_map_to_owned(data));

        let incoming_keys: HashSet<&String> = data.keys().collect();
        let mut removed_keys: HashSet<String> = HashSet::new();
        for old_key in &old_keys {
            if !incoming_keys.contains(old_key) {
                runtime_state::remove_service(old_key);
                compiled_selector::drop_cell(old_key);
                removed_keys.insert(old_key.clone());
            }
        }
        // Self-resolved ExternalName addresses outlive the Service otherwise: the
        // key is unreachable once the Service is gone, but it would sit in the
        // cache until unrelated traffic evicts it. The guard keys off any removal,
        // not off ExternalName-ness, so a gateway that never removes a Service
        // never materializes the global cache — one that removes any Service does.
        if !removed_keys.is_empty() {
            get_global_dns_cache().invalidate_services(&removed_keys);
        }
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<Service>>,
        update: HashMap<String, Arc<Service>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "service_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        // Per-service policy config is owned by the EdgionBackendTrafficPolicy
        // conf-handler; only per-backend runtime state and selector cells are
        // cleaned up here for removed Services.
        for key in &remove {
            runtime_state::remove_service(key);
            compiled_selector::drop_cell(key);
        }

        // Merge add and update for storage, extracting from Arc
        let mut add_or_update = arc_map_into_owned(add);
        add_or_update.extend(arc_map_into_owned(update));

        self.update(add_or_update, &remove);

        // After the store swap, as in `full_set`: the sweep is a non-atomic
        // scan-then-remove, so running it once the deleted Services are already
        // invisible leaves the smallest window for an in-flight request to
        // re-insert an entry behind it. An *updated* Service needs no
        // invalidation at all — the queried hostname and the effective resolver
        // are both part of the DNS cache key, so the previous generation's entry
        // is already unreachable.
        if !remove.is_empty() {
            get_global_dns_cache().invalidate_services(&remove);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_service(namespace: &str, name: &str) -> Service {
        let json = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "namespace": namespace,
                "name": name
            },
            "spec": {
                "ports": [{
                    "port": 80,
                    "targetPort": 8080
                }],
                "selector": {
                    "app": name
                }
            }
        });
        serde_json::from_value(json).expect("Failed to create Service")
    }

    /// Seed the process-global DNS cache under a key unique to the caller, so
    /// these tests never collide with another test in the same binary.
    fn seed_dns_cache(service_key: &str) {
        get_global_dns_cache().insert(
            service_key,
            "seed.example.test",
            "fp-test",
            vec!["10.20.0.1".parse().expect("valid IPv4")],
            std::time::Duration::from_secs(60),
            std::time::Instant::now(),
        );
    }

    fn dns_cache_has(service_key: &str) -> bool {
        get_global_dns_cache()
            .get(service_key, "seed.example.test", "fp-test", std::time::Instant::now())
            .is_some()
    }

    #[test]
    fn partial_update_invalidates_dns_cache_for_removed_services() {
        let store = ServiceStore::new();
        let removed = "dnscache-partial-removed/svc";
        let kept = "dnscache-partial-kept/svc";
        seed_dns_cache(removed);
        seed_dns_cache(kept);

        store.partial_update(HashMap::new(), HashMap::new(), HashSet::from([removed.to_string()]));

        assert!(!dns_cache_has(removed), "removed Service's entry must be invalidated");
        assert!(dns_cache_has(kept), "an unrelated Service's entry must survive");
    }

    #[test]
    fn partial_update_does_not_invalidate_dns_cache_for_updated_services() {
        // Deliberate: `external_name` and the resolver fingerprint are both in
        // the cache key, so the previous generation's entry is already
        // unreachable. Invalidating here would force a re-resolve after any
        // unrelated spec edit.
        let store = ServiceStore::new();
        let updated = "dnscache-partial-updated/svc";
        seed_dns_cache(updated);

        let mut update = HashMap::new();
        update.insert(
            updated.to_string(),
            Arc::new(create_test_service("dnscache-partial-updated", "svc")),
        );
        store.partial_update(HashMap::new(), update, HashSet::new());

        assert!(dns_cache_has(updated), "an updated Service must not be invalidated");
    }

    #[test]
    fn full_set_invalidates_dns_cache_for_services_absent_from_the_new_set() {
        let store = ServiceStore::new();
        let dropped = "dnscache-fullset-dropped/svc";
        let retained = "dnscache-fullset-retained/svc";

        // The dropped key must be in the store first, or `full_set` computes an
        // empty removal set and the sweep never runs.
        let mut initial = HashMap::new();
        initial.insert(
            dropped.to_string(),
            Arc::new(create_test_service("dnscache-fullset-dropped", "svc")),
        );
        store.full_set(&initial);

        seed_dns_cache(dropped);
        seed_dns_cache(retained);

        // A new full set that no longer contains the dropped key.
        store.full_set(&HashMap::new());

        assert!(!dns_cache_has(dropped), "dropped Service's entry must be invalidated");
        assert!(
            dns_cache_has(retained),
            "a Service this store never held must not be swept"
        );
    }

    #[test]
    fn test_full_set() {
        let store = ServiceStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );
        data.insert(
            "default/svc2".to_string(),
            Arc::new(create_test_service("default", "svc2")),
        );

        store.full_set(&data);

        assert!(store.contains("default/svc1"));
        assert!(store.contains("default/svc2"));
        assert!(!store.contains("default/svc3"));
    }

    #[test]
    fn test_partial_update_add() {
        let store = ServiceStore::new();

        let mut add = HashMap::new();
        add.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );

        store.partial_update(add, HashMap::new(), HashSet::new());

        assert!(store.contains("default/svc1"));
    }

    #[test]
    fn test_partial_update_update() {
        let store = ServiceStore::new();

        // First add a service
        let mut data = HashMap::new();
        data.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );
        store.full_set(&data);

        // Then update it
        let mut update = HashMap::new();
        update.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );

        store.partial_update(HashMap::new(), update, HashSet::new());

        assert!(store.contains("default/svc1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let store = ServiceStore::new();

        // First add a service
        let mut data = HashMap::new();
        data.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );
        store.full_set(&data);

        // Then remove it
        let mut remove = HashSet::new();
        remove.insert("default/svc1".to_string());
        store.partial_update(HashMap::new(), HashMap::new(), remove);

        assert!(!store.contains("default/svc1"));
    }
}
