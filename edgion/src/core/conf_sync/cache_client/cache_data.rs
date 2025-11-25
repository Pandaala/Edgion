use crate::core::conf_sync::cache_client::ConfHandler;
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::ResourceMeta;
use std::collections::HashMap;

/// Internal cache data structure combining data and version under single lock
pub struct CacheData<T> {
    ready: bool,
    data: HashMap<String, T>,
    resource_version: u64,

    // 注册到Cachedata内部，用于处理具体配置的handler
    handler_running: bool,
    handler_processor: Option<Box<dyn ConfHandler<T> + Send + Sync>>,
    handler_compressed_events: CompressEvent,
}

impl<T: ResourceMeta> CacheData<T> {
    pub(crate) fn new() -> Self {
        Self {
            ready: false,
            data: HashMap::new(),
            resource_version: 0,
            handler_running: false,
            handler_processor: None,
            handler_compressed_events: CompressEvent::new(),
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
        if let Some(mut processor) = self.handler_processor.take() {
            // Pass reference to full_build, no need to clone or move data
            processor.full_build(&self.data);
            // Put processor back
            self.handler_processor = Some(processor);
        }
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

    pub(crate) fn insert(&mut self, key: String, value: T) {
        self.data.insert(key, value);
    }

    pub(crate) fn remove(&mut self, key: &str) -> Option<T> {
        self.data.remove(key)
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
    pub(crate) fn set_conf_processor(&mut self, processor: Box<dyn ConfHandler<T> + Send + Sync>) {
        self.handler_processor = Some(processor);
    }

    pub(crate) fn conf_processor(&self) -> Option<&(dyn ConfHandler<T> + Send + Sync)> {
        self.handler_processor.as_deref()
    }

    // Compress events methods
    pub(crate) fn add_compress_event(&mut self, key: String, change: ResourceChange) {
        self.handler_compressed_events.add_event(key, change);
    }

    pub(crate) fn clear_compress_events(&mut self) {
        self.handler_compressed_events.clear();
    }
}


pub struct CompressEvent {
    events: HashMap<String, Vec<ResourceChange>>,
}

impl CompressEvent {
    pub fn new() -> Self {
        Self {
            events: HashMap::new(),
        }
    }

    /// Add an event for a resource key
    pub fn add_event(&mut self, key: String, change: crate::core::conf_sync::traits::ResourceChange) {
        self.events.entry(key).or_insert_with(Vec::new).push(change);
    }

    /// Get events for a resource key
    pub fn get_events(&self, key: &str) -> Option<&Vec<ResourceChange>> {
        self.events.get(key)
    }

    /// Clear all events
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

