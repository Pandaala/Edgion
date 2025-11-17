use crate::core::conf_sync::cache_server::{EventDispatch, ListData, Versionable};
use crate::core::conf_sync::traits::ResourceChange;
use std::collections::HashMap;
use kube::{Resource, ResourceExt};


pub struct ClientCache<T> where T: kube::Resource {
    // data
    data: HashMap<String, T>,

    // version
    resource_version: u64,
}

impl<T: Versionable + Resource> ClientCache<T> {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            resource_version: 0,
        }
    }

    /// Get current resource version
    pub fn get_resource_version(&self) -> u64 {
        self.resource_version
    }

    /// Set current resource version
    pub fn set_resource_version(&mut self, version: u64) {
        self.resource_version = version;
    }

    /// List all data - returns all resources in the cache with resource version
    pub fn list(&self) -> ListData<&T> {
        let data: Vec<&T> = self.data.values().collect();
        ListData::new(data, self.resource_version)
    }

    /// List all data as owned values (cloned)
    pub fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let data: Vec<T> = self.data.values().cloned().collect();
        ListData::new(data, self.resource_version)
    }

    /// Get a resource by key
    pub fn get(&self, key: &str) -> Option<&T> {
        self.data.get(key)
    }

    /// Get all keys
    pub fn keys(&self) -> Vec<String> {
        self.data.keys().cloned().collect()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Get the number of resources in the cache
    pub fn len(&self) -> usize {
        self.data.len()
    }
}

impl<T: Versionable + Resource +  Clone + Send + 'static> EventDispatch<T> for ClientCache<T> {
    fn apply_change(&mut self, change: ResourceChange, resource: T)
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
                self.data.insert(version.to_string(), resource);
                if version > self.resource_version {
                    self.resource_version = version;
                }
            }
            ResourceChange::EventDelete => {
                self.data.remove(&version.to_string());
                if version > self.resource_version {
                    self.resource_version = version;
                }
                drop(resource);
            }
        }
    }

    fn set_ready(&mut self) {
        // HubCache doesn't need ready state, but we keep the method for trait compatibility
    }
}

impl<T: Versionable + Resource> Default for ClientCache<T> {
    fn default() -> Self {
        Self::new()
    }
}
