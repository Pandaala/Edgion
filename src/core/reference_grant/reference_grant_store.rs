//! Global store for ReferenceGrant resources

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use arc_swap::ArcSwap;
use std::sync::LazyLock;

use crate::types::resources::ReferenceGrant;
use crate::core::conf_sync::traits::ConfHandler;

static GLOBAL_REFERENCE_GRANT_STORE: LazyLock<Arc<ReferenceGrantStore>> =
    LazyLock::new(|| Arc::new(ReferenceGrantStore::new()));

pub fn get_global_reference_grant_store() -> Arc<ReferenceGrantStore> {
    GLOBAL_REFERENCE_GRANT_STORE.clone()
}

/// Create a handler for ReferenceGrant
pub fn create_reference_grant_handler() -> Box<dyn ConfHandler<ReferenceGrant> + Send + Sync> {
    Box::new(get_global_reference_grant_store())
}

/// Type alias for the reference grant map (key: namespace/name)
type ReferenceGrantMap = HashMap<String, Arc<ReferenceGrant>>;

pub struct ReferenceGrantStore {
    grants: ArcSwap<ReferenceGrantMap>,
}

impl ReferenceGrantStore {
    pub fn new() -> Self {
        Self {
            grants: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Check if a reference grant exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.grants.load();
        map.contains_key(key)
    }

    /// Get a reference grant by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<Arc<ReferenceGrant>> {
        let map = self.grants.load();
        map.get(key).cloned()
    }

    /// Get a reference grant by namespace and name
    pub fn get_by_ns_name(&self, namespace: &str, name: &str) -> Option<Arc<ReferenceGrant>> {
        let key = format!("{}/{}", namespace, name);
        self.get(&key)
    }

    /// Execute a function with the grant reference
    pub fn with_grant<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&ReferenceGrant) -> R,
    {
        let map = self.grants.load();
        map.get(key).map(|g| f(g))
    }

    /// Replace all reference grants atomically
    pub fn replace_all(&self, grants: HashMap<String, Arc<ReferenceGrant>>) {
        self.grants.store(Arc::new(grants));
    }

    /// Update reference grants atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Arc<ReferenceGrant>>, remove: &HashSet<String>) {
        let current = self.grants.load();
        let mut new_map = (**current).clone();

        // Add or update grants
        for (key, grant) in add_or_update {
            new_map.insert(key, grant);
        }

        // Remove grants
        for key in remove {
            new_map.remove(key);
        }

        self.grants.store(Arc::new(new_map));
    }

    /// Get all grants
    pub fn get_all(&self) -> Arc<ReferenceGrantMap> {
        self.grants.load_full()
    }

    /// Get all grants in a specific namespace
    pub fn get_by_namespace(&self, namespace: &str) -> Vec<Arc<ReferenceGrant>> {
        let map = self.grants.load();
        let prefix = format!("{}/", namespace);
        map.iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(_, grant)| grant.clone())
            .collect()
    }
}

impl Default for ReferenceGrantStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfHandler<ReferenceGrant> for Arc<ReferenceGrantStore> {
    fn full_set(&self, data: &HashMap<String, ReferenceGrant>) {
        tracing::info!(
            component = "reference_grant_store",
            cnt = data.len(),
            "full set"
        );
        
        let grants: HashMap<String, Arc<ReferenceGrant>> = data
            .iter()
            .map(|(k, v)| (k.clone(), Arc::new(v.clone())))
            .collect();
        
        self.replace_all(grants);
    }

    fn partial_update(
        &self,
        add: HashMap<String, ReferenceGrant>,
        update: HashMap<String, ReferenceGrant>,
        remove: HashSet<String>
    ) {
        tracing::info!(
            component = "reference_grant_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        // Combine add and update
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

        self.update(add_or_update, &remove);
    }
}

impl ConfHandler<ReferenceGrant> for &'static ReferenceGrantStore {
    fn full_set(&self, data: &HashMap<String, ReferenceGrant>) {
        tracing::info!(
            component = "reference_grant_store",
            cnt = data.len(),
            "full set (static ref)"
        );
        
        let grants: HashMap<String, Arc<ReferenceGrant>> = data
            .iter()
            .map(|(k, v)| (k.clone(), Arc::new(v.clone())))
            .collect();
        
        self.replace_all(grants);
    }

    fn partial_update(
        &self,
        add: HashMap<String, ReferenceGrant>,
        update: HashMap<String, ReferenceGrant>,
        remove: HashSet<String>
    ) {
        tracing::info!(
            component = "reference_grant_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update (static ref)"
        );

        // Combine add and update
        let mut add_or_update = HashMap::new();
        for (k, v) in add {
            tracing::debug!(key = %k, "Adding ReferenceGrant (static ref)");
            add_or_update.insert(k, Arc::new(v));
        }
        for (k, v) in update {
            tracing::debug!(key = %k, "Updating ReferenceGrant (static ref)");
            add_or_update.insert(k, Arc::new(v));
        }

        // Log removals
        for key in &remove {
            tracing::debug!(key = %key, "Removing ReferenceGrant (static ref)");
        }

        self.update(add_or_update, &remove);
    }
}

