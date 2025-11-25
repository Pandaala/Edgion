use crate::core::conf_sync::cache_client::ConfHandler;
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::ResourceMeta;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tokio::time::{interval, Duration};

/// Internal cache data structure combining data and version under single lock
pub struct CacheData<T> {
    ready: bool,
    data: HashMap<String, T>,
    resource_version: u64,

    // 注册到Cachedata内部，用于处理具体配置的handler
    handler: Option<ConfHandlerData<T>>,
}

impl<T: ResourceMeta> CacheData<T> {
    pub(crate) fn new() -> Self {
        Self {
            ready: false,
            data: HashMap::new(),
            resource_version: 0,
            handler: None,
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
        if let Some(ref mut handler) = self.handler {
            // Pass reference to full_build, no need to clone or move data
            handler.processor.full_build(&self.data);
            handler.compressed_events.clear();
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

    // Configuration handler methods
    pub(crate) fn set_conf_processor(
        &mut self,
        processor: Box<dyn ConfHandler<T> + Send + Sync>,
        cache_data: Arc<RwLock<CacheData<T>>>,
    ) where
        T: Clone + ResourceMeta,
    {
        // Create handler with processor (processor is required, so no Option needed)
        self.handler = Some(ConfHandlerData::new(processor));

        // Start a background task to process compressed events every 100ms
        let cache_data_clone = cache_data.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(100));
            loop {
                interval.tick().await;

                // Collect events and resources
                let (events_to_process, mut add_or_update, mut remove) = {
                    let cache = cache_data_clone.read().unwrap();
                    // Processor is guaranteed to exist at spawn time, but check handler in case it was removed
                    let handler = match cache.handler.as_ref() {
                        Some(h) => h,
                        None => {
                            tracing::error!(component = "cache_client", "Handler was removed, stopping compressed events processing task");
                            break; // Handler removed, stop the task
                        }
                    };

                    // Collect events to process
                    let events: HashMap<String, Vec<ResourceChange>> = handler
                        .compressed_events
                        .events
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();

                    // Prepare maps for add_or_update and remove
                    let mut add_or_update_map = HashMap::new();
                    let mut remove_set = HashSet::new();

                    // Classify events based on current state in cache.data
                    // If resource exists in cache.data, it's add_or_update
                    // If resource doesn't exist in cache.data, it's remove
                    for (key, _event_changes) in &events {
                        if let Some(resource) = cache.data.get(key).cloned() {
                            // Resource exists in cache, it's add_or_update
                            add_or_update_map.insert(key.clone(), resource);
                            tracing::info!(key = %key, "Processed compressed event: add_or_update");
                        } else {
                            // Resource doesn't exist in cache, it's remove
                            // For remove, we only need the key
                            remove_set.insert(key.clone());
                            tracing::info!(key = %key, "Processed compressed event: remove");
                        }
                    }

                    (events, add_or_update_map, remove_set)
                };

                // Process events if there are any
                if !events_to_process.is_empty() && (!add_or_update.is_empty() || !remove.is_empty()) {
                    // Clear processed events and call conf_change
                    let mut cache = cache_data_clone.write().unwrap();
                    if let Some(ref mut handler) = cache.handler {
                        handler.compressed_events.clear();
                        handler.processor.conf_change(add_or_update, remove);
                        handler.processor.update_rebuild();
                    } else {
                        // Handler was removed, stop the task
                        tracing::error!(component = "cache_client", "Handler was removed during event processing, stopping compressed events processing task");
                        break;
                    }
                }
            }
        });
    }

    // Compress events methods
    pub(crate) fn add_compress_event(&mut self, key: String, change: ResourceChange) {
        // Only add event if handler exists (which means processor is set)
        // Events added before processor is set will be lost, but that's acceptable
        // as the processor will do a full_build on reset anyway
        if let Some(ref mut handler) = self.handler {
            handler.compressed_events.add_event(key, change);
        }
    }

    pub(crate) fn clear_compress_events(&mut self) {
        if let Some(ref mut handler) = self.handler {
            handler.compressed_events.clear();
        }
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

/// Configuration handler data structure
/// Contains handler state, processor, and compressed events
pub struct ConfHandlerData<T> {
    processor: Box<dyn ConfHandler<T> + Send + Sync>,
    compressed_events: CompressEvent,
}

impl<T> ConfHandlerData<T> {
    pub(crate) fn new(processor: Box<dyn ConfHandler<T> + Send + Sync>) -> Self {
        Self {
            processor,
            compressed_events: CompressEvent::new(),
        }
    }
}

