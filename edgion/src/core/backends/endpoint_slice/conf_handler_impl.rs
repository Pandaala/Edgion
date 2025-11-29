use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use crate::core::conf_sync::traits::ConfHandler;
use super::{EpSliceStore, get_global_ep_slice_store};

/// Implement ConfHandler for Arc<EpSliceStore>
impl ConfHandler<EndpointSlice> for Arc<EpSliceStore> {
    fn full_set(&self, data: &HashMap<String, EndpointSlice>) {
        (**self).full_set(data)
    }

    fn partial_update(&self, add_or_update: HashMap<String, EndpointSlice>, remove: HashSet<String>) {
        (**self).partial_update(add_or_update, remove)
    }
}

/// Create an EpSliceStore handler for registration with ConfigClient
pub fn create_ep_slice_handler() -> Box<dyn ConfHandler<EndpointSlice> + Send + Sync> {
    Box::new(get_global_ep_slice_store())
}

impl ConfHandler<EndpointSlice> for EpSliceStore {
    fn full_set(&self, data: &HashMap<String, EndpointSlice>) {
        tracing::info!(component = "ep_slice_store", cnt = data.len(), "full set");
        self.replace_all(data.clone());
    }

    fn partial_update(&self, add_or_update: HashMap<String, EndpointSlice>, remove: HashSet<String>) {
        tracing::info!(
            component = "ep_slice_store",
            au = add_or_update.len(),
            rm = remove.len(),
            "partial update"
        );
        self.update(add_or_update, &remove);
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
    fn test_partial_update() {
        let store = EpSliceStore::new();
        
        let mut add_or_update = HashMap::new();
        add_or_update.insert("default/svc1-abc".to_string(), create_test_ep_slice("default", "svc1-abc", "svc1"));
        
        store.partial_update(add_or_update, HashSet::new());
        
        assert!(store.contains("default/svc1-abc"));
    }
}

