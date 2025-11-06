use crate::core::conf_sync::center_cache::{EventDispatch, ListData, Versionable};
use std::collections::HashMap;

pub struct HubCache<T> {
    // data
    data: HashMap<String, T>,

    // version
    resource_version: u64,
}

impl<T: Versionable> HubCache<T> {
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

impl<T: Versionable + Clone> EventDispatch<T> for HubCache<T> {
    fn init_add(&mut self, resource: T, resource_version: Option<u64>) {
        let version = resource_version.unwrap_or_else(|| resource.get_version());
        self.data.insert(version.to_string(), resource);
        if version > self.resource_version {
            self.resource_version = version;
        }
    }

    fn set_ready(&mut self) {
        // HubCache doesn't need ready state, but we keep the method for trait compatibility
    }

    fn event_add(&mut self, resource: T, resource_version: Option<u64>)
    where
        T: Send + 'static,
    {
        let version = resource_version.unwrap_or_else(|| resource.get_version());
        self.data.insert(version.to_string(), resource);
        if version > self.resource_version {
            self.resource_version = version;
        }
    }

    fn event_update(&mut self, resource: T, resource_version: Option<u64>)
    where
        T: Send + 'static,
    {
        let version = resource_version.unwrap_or_else(|| resource.get_version());
        self.data.insert(version.to_string(), resource);
        if version > self.resource_version {
            self.resource_version = version;
        }
    }

    fn event_del(&mut self, resource: T, resource_version: Option<u64>)
    where
        T: Send + 'static,
    {
        let version = resource_version.unwrap_or_else(|| resource.get_version());
        self.data.remove(&version.to_string());
        if version > self.resource_version {
            self.resource_version = version;
        }
    }
}

impl<T: Versionable> Default for HubCache<T> {
    fn default() -> Self {
        Self::new()
    }
}
