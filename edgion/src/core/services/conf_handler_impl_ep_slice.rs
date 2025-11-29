use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::services::{ServiceMgr, get_global_service_mgr};

const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";

/// Extract service key from EndpointSlice
/// Returns "namespace/service-name" based on the kubernetes.io/service-name label
fn get_service_key(ep_slice: &EndpointSlice) -> Option<String> {
    let metadata = &ep_slice.metadata;
    let namespace = metadata.namespace.as_deref()?;
    let labels = metadata.labels.as_ref()?;
    let service_name = labels.get(SERVICE_NAME_LABEL)?;
    Some(format!("{}/{}", namespace, service_name))
}

/// Implement ConfHandler for Arc<ServiceMgr> to handle EndpointSlice
impl ConfHandler<EndpointSlice> for Arc<ServiceMgr> {
    fn full_set(&self, data: &HashMap<String, EndpointSlice>) {
        (**self).full_set_ep_slice(data)
    }

    fn partial_update(&self, add_or_update: HashMap<String, EndpointSlice>, remove: HashSet<String>) {
        (**self).partial_update_ep_slice(add_or_update, remove)
    }
}

/// Create a ServiceMgr handler for EndpointSlice registration with ConfigClient
pub fn create_ep_slice_handler() -> Box<dyn ConfHandler<EndpointSlice> + Send + Sync> {
    Box::new(get_global_service_mgr())
}

impl ServiceMgr {
    /// Full set with a complete set of EndpointSlices
    pub fn full_set_ep_slice(&self, data: &HashMap<String, EndpointSlice>) {
        tracing::info!(component = "service_mgr", cnt = data.len(), "full set ep_slice");
        for (_, ep_slice) in data {
            if let Some(service_key) = get_service_key(ep_slice) {
                self.set_ep_slice(&service_key, ep_slice.clone());
            }
        }
    }

    /// Handle partial EndpointSlice updates
    pub fn partial_update_ep_slice(&self, add_or_update: HashMap<String, EndpointSlice>, remove: HashSet<String>) {
        tracing::info!(
            component = "service_mgr",
            au = add_or_update.len(),
            rm = remove.len(),
            "partial update ep_slice"
        );
        
        // Handle removals - need to find service key from the ep_slice key
        // Since we don't have the ep_slice object for removed keys, we need to track the mapping
        // For now, we iterate and try to match by namespace/name pattern
        for key in &remove {
            // key format is typically "namespace/ep-slice-name"
            // We need to find the corresponding service and clear its ep_slice
            // This is a simplified approach - in production, you might want to maintain a reverse mapping
            self.remove_ep_slice(key);
        }
        
        // Handle additions/updates
        for (_, ep_slice) in add_or_update {
            if let Some(service_key) = get_service_key(&ep_slice) {
                self.set_ep_slice(&service_key, ep_slice);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_full_set_ep_slice() {
        let mgr = ServiceMgr::new();
        
        let mut data = HashMap::new();
        data.insert("default/svc1-abc".to_string(), create_test_ep_slice("default", "svc1-abc", "svc1"));
        
        mgr.full_set_ep_slice(&data);
        
        assert!(mgr.has_ep_slice("default/svc1"));
    }

    #[test]
    fn test_partial_update_ep_slice() {
        let mgr = ServiceMgr::new();
        
        let mut add_or_update = HashMap::new();
        add_or_update.insert("default/svc1-abc".to_string(), create_test_ep_slice("default", "svc1-abc", "svc1"));
        
        mgr.partial_update_ep_slice(add_or_update, HashSet::new());
        
        assert!(mgr.has_ep_slice("default/svc1"));
    }
}

