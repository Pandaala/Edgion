use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use crate::core::conf_sync::traits::ConfHandler;
use super::{EpSliceStore, get_global_ep_slice_store};
use super::discovery_impl::EndpointSliceLoadBalancer;

/// Implement ConfHandler for Arc<EpSliceStore>
impl ConfHandler<EndpointSlice> for Arc<EpSliceStore> {
    fn full_set(&self, data: &HashMap<String, EndpointSlice>) {
        (**self).full_set(data)
    }

    fn partial_update(&self, add: HashMap<String, EndpointSlice>, update: HashMap<String, EndpointSlice>, remove: HashSet<String>) {
        (**self).partial_update(add, update, remove)
    }
}

/// Create an EpSliceStore handler for registration with ConfigClient
pub fn create_ep_slice_handler() -> Box<dyn ConfHandler<EndpointSlice> + Send + Sync> {
    Box::new(get_global_ep_slice_store())
}

impl ConfHandler<EndpointSlice> for EpSliceStore {
    fn full_set(&self, data: &HashMap<String, EndpointSlice>) {
        tracing::info!(component = "ep_slice_store", cnt = data.len(), "full set");
        
        // Convert EndpointSlice to Arc<EndpointSliceLoadBalancer>
        let lb_map: HashMap<String, Arc<EndpointSliceLoadBalancer>> = data
            .iter()
            .map(|(key, ep_slice)| {
                let lb = EndpointSliceLoadBalancer::new(ep_slice.clone());
                (key.clone(), lb)
            })
            .collect();
        
        self.replace_all(lb_map);
    }

    fn partial_update(&self, add: HashMap<String, EndpointSlice>, update: HashMap<String, EndpointSlice>, remove: HashSet<String>) {
        let add_count = add.len();
        let update_count = update.len();
        let remove_count = remove.len();
        
        // Handle updates in-place (no ArcSwap rebuild)
        // Update EndpointSlice data + trigger LoadBalancer refresh
        for (key, ep_slice) in update {
            let _ = self.update_in_place_and_refresh_lb(&key, ep_slice);
        }
        
        // Only rebuild ArcSwap for add/remove operations
        let arcswap_rebuilt = if !add.is_empty() || !remove.is_empty() {
            self.apply_modifications(|map| {
                for key in &remove {
                    map.remove(key);
                }
                for (key, ep_slice) in add {
                    let lb = EndpointSliceLoadBalancer::new(ep_slice);
                    map.insert(key, lb);
                }
            });
            true
        } else {
            false
        };
        
        // Log summary at the end
        tracing::info!(
            component = "ep_slice_store",
            add_count = add_count,
            update_count = update_count,
            remove_count = remove_count,
            arcswap_rebuilt = arcswap_rebuilt,
            "Partial update completed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::get_service_key;

    fn create_test_ep_slice(namespace: &str, name: &str, service_name: &str) -> EndpointSlice {
        let json = serde_json::json!({
            "apiVersion": "discovery.k8s.io/v1",
            "kind": "EndpointSlice",
            "metadata": {
                "namespace": namespace,
                "name": name,
                "labels": {
                    "kubernetes.io/service-name": service_name
                }
            },
            "addressType": "IPv4",
            "endpoints": [{
                "addresses": ["10.0.0.1"],
                "conditions": {
                    "ready": true
                }
            }],
            "ports": [{
                "port": 80,
                "protocol": "TCP"
            }]
        });
        serde_json::from_value(json).expect("Failed to create EndpointSlice")
    }

    #[test]
    fn test_get_service_key() {
        let ep_slice = create_test_ep_slice("default", "my-svc-abc", "my-svc");
        let key = get_service_key(&ep_slice);
        assert_eq!(key, Some("default/my-svc".to_string()));
    }

    #[test]
    fn test_full_set() {
        let store = EpSliceStore::new();
        
        let mut data = HashMap::new();
        data.insert("default/svc1-abc".to_string(), create_test_ep_slice("default", "svc1-abc", "svc1"));
        
        store.full_set(&data);
        
        assert!(store.contains("default/svc1-abc"));
    }

    #[test]
    fn test_get_by_service() {
        let store = EpSliceStore::new();
        
        let mut data = HashMap::new();
        data.insert("default/svc1-abc".to_string(), create_test_ep_slice("default", "svc1-abc", "svc1"));
        
        store.full_set(&data);
        
        let ep = store.get_by_service("default/svc1");
        assert!(ep.is_some());
    }

    #[test]
    fn test_partial_update_add() {
        let store = EpSliceStore::new();
        
        let mut add = HashMap::new();
        add.insert("default/svc1-abc".to_string(), create_test_ep_slice("default", "svc1-abc", "svc1"));
        
        store.partial_update(add, HashMap::new(), HashSet::new());
        
        assert!(store.contains("default/svc1-abc"));
    }
    
    #[test]
    fn test_partial_update() {
        let store = EpSliceStore::new();
        
        // First add an endpoint slice
        let mut data = HashMap::new();
        data.insert("default/svc1-abc".to_string(), create_test_ep_slice("default", "svc1-abc", "svc1"));
        store.full_set(&data);
        
        // Then update it
        let mut update = HashMap::new();
        update.insert("default/svc1-abc".to_string(), create_test_ep_slice("default", "svc1-abc", "svc1"));
        
        store.partial_update(HashMap::new(), update, HashSet::new());
        
        assert!(store.contains("default/svc1-abc"));
    }
}

