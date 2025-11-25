use crate::core::conf_sync::cache_client::ConfProcessor;
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::ResourceMeta;
use std::collections::HashMap;

use crate::core::conf_sync::cache_client::cache::CompressEvent;

/// Internal cache data structure combining data and version under single lock
pub struct CacheData<T> {
    ready: bool,
    data: HashMap<String, T>,
    resource_version: u64,
    // configuration processor (optional)
    conf_processor: Option<Box<dyn ConfProcessor<T> + Send + Sync>>,
    // compressed events storage
    compress_events: CompressEvent,
}

impl<T: ResourceMeta> CacheData<T> {
    pub(crate) fn new() -> Self {
        Self {
            ready: false,
            data: HashMap::new(),
            resource_version: 0,
            conf_processor: None,
            compress_events: CompressEvent::new(),
        }
    }

    /// Reset cache with a complete set of resources
    /// Uses resource.key_name() (namespace/name) as the key for each resource
    pub(crate) fn reset(&mut self, resources: Vec<T>, resource_version: u64) {
        self.data.clear();
        for resource in resources {
            let key = resource.key_name();
            self.data.insert(key, resource);
        }
        self.resource_version = resource_version;
    }

    // Ready state methods
    pub(crate) fn is_ready(&self) -> bool {
        self.ready
    }

    pub(crate) fn set_ready(&mut self) {
        self.ready = true;
    }

    // Data methods
    pub(crate) fn get(&self, key: &str) -> Option<&T> {
        self.data.get(key)
    }

    /// Insert a resource and automatically update resource version if newer
    pub(crate) fn insert(&mut self, key: String, value: T)
    where
        T: ResourceMeta,
    {
        let version = value.get_version();
        self.data.insert(key, value);
        self.update_resource_version_if_newer(version);
    }

    /// Remove a resource and automatically update resource version if newer
    pub(crate) fn remove(&mut self, key: &str) -> Option<T>
    where
        T: ResourceMeta,
    {
        if let Some(removed) = self.data.remove(key) {
            let version = removed.get_version();
            self.update_resource_version_if_newer(version);
            Some(removed)
        } else {
            None
        }
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &T> {
        self.data.values()
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &String> {
        self.data.keys()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    // Resource version methods
    pub(crate) fn resource_version(&self) -> u64 {
        self.resource_version
    }

    pub(crate) fn set_resource_version(&mut self, version: u64) {
        self.resource_version = version;
    }

    pub(crate) fn update_resource_version_if_newer(&mut self, version: u64) {
        if version > self.resource_version {
            self.resource_version = version;
        }
    }

    // Configuration processor methods
    pub(crate) fn set_conf_processor(&mut self, processor: Box<dyn ConfProcessor<T> + Send + Sync>) {
        self.conf_processor = Some(processor);
    }

    pub(crate) fn conf_processor(&self) -> Option<&(dyn ConfProcessor<T> + Send + Sync)> {
        self.conf_processor.as_deref()
    }

    // Compress events methods
    pub(crate) fn add_compress_event(&mut self, key: String, change: ResourceChange) {
        self.compress_events.add_event(key, change);
    }

    pub(crate) fn get_compress_events(&self, key: &str) -> Option<&Vec<ResourceChange>> {
        self.compress_events.get_events(key)
    }

    pub(crate) fn clear_compress_events(&mut self) {
        self.compress_events.clear();
    }
}

