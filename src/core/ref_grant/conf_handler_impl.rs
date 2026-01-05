//! ConfHandler implementation for ReferenceGrant
//!
//! Handles configuration synchronization for ReferenceGrant resources.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::store::ReferenceGrantStore;
use crate::core::conf_sync::traits::ConfHandler;
use crate::types::resources::ReferenceGrant;

/// Create a handler for ReferenceGrant configuration updates
pub fn create_reference_grant_handler() -> Box<dyn ConfHandler<ReferenceGrant> + Send + Sync> {
    Box::new(super::store::get_global_reference_grant_store())
}

impl ConfHandler<ReferenceGrant> for Arc<ReferenceGrantStore> {
    fn full_set(&self, data: &HashMap<String, ReferenceGrant>) {
        tracing::info!(component = "ref_grant_store", cnt = data.len(), "full set");

        // Convert to Arc-wrapped grants
        let grants: HashMap<String, Arc<ReferenceGrant>> =
            data.iter().map(|(k, v)| (k.clone(), Arc::new(v.clone()))).collect();

        // Replace all and rebuild all indexes
        self.replace_all(grants);
    }

    fn partial_update(
        &self,
        add: HashMap<String, ReferenceGrant>,
        update: HashMap<String, ReferenceGrant>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "ref_grant_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        // 1. Identify affected namespaces before updating
        let affected_namespaces = self.identify_affected_namespaces(&add, &update, &remove);

        // 2. Combine add and update into a single map
        let mut add_or_update = HashMap::new();

        for (k, v) in add {
            tracing::debug!(key = %k, "Adding ReferenceGrant");
            add_or_update.insert(k, Arc::new(v));
        }

        for (k, v) in update {
            tracing::debug!(key = %k, "Updating ReferenceGrant");
            add_or_update.insert(k, Arc::new(v));
        }

        // Log removals
        for key in &remove {
            tracing::debug!(key = %key, "Removing ReferenceGrant");
        }

        // 3. Perform incremental update
        self.update_incremental(add_or_update, &remove);

        // 4. Dispatch event to trigger revalidation
        if !affected_namespaces.is_empty() {
            let event = super::events::ReferenceGrantChangedEvent {
                affected_namespaces: affected_namespaces.clone(),
            };
            super::events::get_global_dispatcher().dispatch(&event);

            tracing::info!(
                component = "ref_grant_store",
                affected_ns = ?affected_namespaces,
                "ReferenceGrant changed, event dispatched"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::{ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn create_test_grant(
        namespace: &str,
        name: &str,
        from_namespace: &str,
        from_kind: &str,
        to_kind: &str,
    ) -> ReferenceGrant {
        ReferenceGrant {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantSpec {
                from: vec![ReferenceGrantFrom {
                    group: "gateway.networking.k8s.io".to_string(),
                    kind: from_kind.to_string(),
                    namespace: from_namespace.to_string(),
                }],
                to: vec![ReferenceGrantTo {
                    group: "".to_string(),
                    kind: to_kind.to_string(),
                    name: None,
                }],
            },
        }
    }

    #[test]
    fn test_full_set() {
        // Create a local store instance to avoid global state pollution
        let store = Arc::new(ReferenceGrantStore::new());

        let grant1 = create_test_grant("ns1", "grant1", "ns-source", "HTTPRoute", "Service");
        let grant2 = create_test_grant("ns2", "grant2", "ns-source", "TCPRoute", "Secret");

        let mut data = HashMap::new();
        data.insert("ns1/grant1".to_string(), grant1);
        data.insert("ns2/grant2".to_string(), grant2);

        let handler: Arc<ReferenceGrantStore> = store.clone();
        handler.full_set(&data);

        // Verify grants were stored
        assert!(store.get("ns1/grant1").is_some());
        assert!(store.get("ns2/grant2").is_some());

        // Verify indexes were built
        assert_eq!(store.get_by_to_namespace("ns1").len(), 1);
        assert_eq!(store.get_by_to_namespace("ns2").len(), 1);
    }

    #[test]
    fn test_partial_update() {
        // Create a local store instance to avoid global state pollution
        let store = Arc::new(ReferenceGrantStore::new());

        // Initial state
        let grant1 = create_test_grant("ns1", "grant1", "ns-source", "HTTPRoute", "Service");
        let mut data = HashMap::new();
        data.insert("ns1/grant1".to_string(), grant1);

        let handler: Arc<ReferenceGrantStore> = store.clone();
        handler.full_set(&data);

        // Add a new grant
        let grant2 = create_test_grant("ns1", "grant2", "ns-source2", "TCPRoute", "Service");
        let mut add = HashMap::new();
        add.insert("ns1/grant2".to_string(), grant2);

        handler.partial_update(add, HashMap::new(), HashSet::new());

        // Verify new grant was added
        assert!(store.get("ns1/grant2").is_some());
        assert_eq!(store.get_by_to_namespace("ns1").len(), 2);

        // Update an existing grant
        let grant1_updated = create_test_grant("ns1", "grant1", "ns-source-new", "HTTPRoute", "Service");
        let mut update = HashMap::new();
        update.insert("ns1/grant1".to_string(), grant1_updated);

        handler.partial_update(HashMap::new(), update, HashSet::new());

        // Verify grant was updated
        let updated_grant = store.get("ns1/grant1").unwrap();
        assert_eq!(updated_grant.spec.from[0].namespace, "ns-source-new");

        // Remove a grant
        let mut remove = HashSet::new();
        remove.insert("ns1/grant2".to_string());

        handler.partial_update(HashMap::new(), HashMap::new(), remove);

        // Verify grant was removed
        assert!(store.get("ns1/grant2").is_none());
        assert_eq!(store.get_by_to_namespace("ns1").len(), 1);
    }
}
