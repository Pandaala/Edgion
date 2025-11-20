use crate::core::conf_sync::cache_server::{EventDispatch, ListData, Versionable};
use crate::core::conf_sync::traits::ResourceChange;
use kube::{Resource, ResourceExt};
use std::collections::HashMap;
use std::sync::RwLock;

pub struct ClientCache<T>
where
    T: kube::Resource,
{
    // data
    data: RwLock<HashMap<String, T>>,

    // version
    resource_version: RwLock<u64>,
}

impl<T: Versionable + Resource> ClientCache<T> {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            resource_version: RwLock::new(0),
        }
    }

    /// Get current resource version
    pub fn get_resource_version(&self) -> u64 {
        *self.resource_version.read().unwrap()
    }

    /// Set current resource version
    pub fn set_resource_version(&self, version: u64) {
        *self.resource_version.write().unwrap() = version;
    }

    /// List all data - returns all resources in the cache with resource version
    pub fn list(&self) -> ListData<T>
    where
        T: Clone,
    {
        self.list_owned()
    }

    /// List all data as owned values (cloned)
    pub fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let data = {
            let data_guard = self.data.read().unwrap();
            data_guard.values().cloned().collect()
        };
        let resource_version = *self.resource_version.read().unwrap();
        ListData::new(data, resource_version)
    }

    /// Get a resource by key
    pub fn get(&self, key: &str) -> Option<T>
    where
        T: Clone,
    {
        let data = self.data.read().unwrap();
        data.get(key).cloned()
    }

    /// Get all keys
    pub fn keys(&self) -> Vec<String> {
        let data = self.data.read().unwrap();
        data.keys().cloned().collect()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        let data = self.data.read().unwrap();
        data.is_empty()
    }

    /// Get the number of resources in the cache
    pub fn len(&self) -> usize {
        let data = self.data.read().unwrap();
        data.len()
    }
}

impl<T: Versionable + Resource + Clone + Send + 'static> EventDispatch<T> for ClientCache<T> {
    fn apply_change(&self, change: ResourceChange, resource: T)
    where
        T: Send + 'static,
    {
        let version = resource.get_version();
        if resource.get_version() == 0 {
            tracing::warn!(
                component = "cache_client",
                event = "apply_change",
                change = ?change,
                kind = std::any::type_name::<T>(),
                name = ?resource.name_any(),
                namespace = ?resource.namespace(),
                version = 0,
                "Applying change to cache with version 0"
            );
        } else {
            tracing::info!(
                component = "cache_client",
                event = "apply_change",
                change = ?change,
                kind = std::any::type_name::<T>(),
                name = ?resource.name_any(),
                namespace = ?resource.namespace(),
                version = resource.get_version(),
                "Applying change to cache"
            );
        }

        match change {
            ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                let mut data = self.data.write().unwrap();
                data.insert(version.to_string(), resource);
                let mut resource_version = self.resource_version.write().unwrap();
                if version > *resource_version {
                    *resource_version = version;
                }
            }
            ResourceChange::EventDelete => {
                let mut data = self.data.write().unwrap();
                data.remove(&version.to_string());
                let mut resource_version = self.resource_version.write().unwrap();
                if version > *resource_version {
                    *resource_version = version;
                }
            }
        }
    }

    fn set_ready(&self) {
        // HubCache doesn't need ready state, but we keep the method for trait compatibility
    }
}

impl<T: Versionable + Resource> Default for ClientCache<T> {
    fn default() -> Self {
        Self::new()
    }
}
