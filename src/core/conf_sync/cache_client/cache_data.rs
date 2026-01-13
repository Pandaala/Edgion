use crate::core::conf_sync::traits::{ConfHandler, ResourceChange};
use crate::types::ResourceMeta;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tokio::time::{interval, Duration};

/// Internal cache data structure combining data and version under single lock
pub struct CacheData<T> {
    ready: bool,
    data: HashMap<String, T>,
    sync_version: u64,

    // Register handlers to CacheData for processing specific configurations
    handler: Option<ConfHandlerData<T>>,
}

impl<T: ResourceMeta> CacheData<T> {
    pub(crate) fn new() -> Self {
        Self {
            ready: false,
            data: HashMap::new(),
            sync_version: 0,
            handler: None,
        }
    }

    /// Reset cache with a complete set of resources
    /// Uses resource.key_name() (namespace/name) as the key for each resource
    pub(crate) fn reset(&mut self, resources: Vec<T>, sync_version: u64) {
        // Build new HashMap directly from resources
        self.data = resources
            .into_iter()
            .map(|resource| (resource.key_name(), resource))
            .collect();
        self.sync_version = sync_version;
        if let Some(ref handler) = self.handler {
            // Pass reference to full_set, no need to clone or move data
            handler.processor.full_set(&self.data);
        }
        if let Some(ref mut handler) = self.handler {
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

    // Sync version methods
    pub(crate) fn sync_version(&self) -> u64 {
        self.sync_version
    }

    pub(crate) fn set_sync_version(&mut self, version: u64) {
        self.sync_version = version;
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

                // Process compressed events, return false if should stop
                if !Self::process_compressed_events(&cache_data_clone) {
                    break;
                }
            }
        });
    }

    /// Process compressed events in a single write lock
    /// Returns true if should continue, false if should stop
    fn process_compressed_events(cache_data: &Arc<RwLock<CacheData<T>>>) -> bool
    where
        T: Clone + ResourceMeta,
    {
        let mut cache = cache_data.write().unwrap();

        // Get handler reference at the beginning
        let handler = match cache.handler.as_mut() {
            Some(h) => h,
            None => {
                tracing::error!(
                    component = "cache_client",
                    "Handler was removed, stopping compressed events processing task"
                );
                return false; // Handler removed, stop the task
            }
        };

        // If no events, skip this iteration
        if handler.compressed_events.events.is_empty() {
            return true; // Continue
        }

        // Collect event keys and their event types first (to avoid borrowing conflicts)
        let event_infos: Vec<(String, Vec<ResourceChange>)> = handler
            .compressed_events
            .events
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // The mutable borrow of handler ends here, allowing us to access cache.data below

        // Classify events based on ResourceChange type and current state in cache.data
        let mut add = HashMap::new();
        let mut update = HashMap::new();
        let mut remove = HashSet::new();

        for (key, events) in event_infos {
            if let Some(resource) = cache.data.get(&key).cloned() {
                // Resource exists in cache - check event history to determine if it's add or update
                // Check if any event in the history is InitAdd or EventAdd
                let is_add = events
                    .iter()
                    .any(|e| matches!(e, ResourceChange::InitAdd | ResourceChange::EventAdd));

                if is_add {
                    add.insert(key.clone(), resource);
                    tracing::info!(key = %key, "Processed compressed event: add");
                } else {
                    update.insert(key.clone(), resource);
                    tracing::info!(key = %key, "Processed compressed event: update");
                }
            } else {
                remove.insert(key.clone());
                tracing::info!(key = %key, "Processed compressed event: remove");
            }
        }

        // Process events if there are any
        if !add.is_empty() || !update.is_empty() || !remove.is_empty() {
            let add_keys: Vec<&String> = add.keys().collect();
            let update_keys: Vec<&String> = update.keys().collect();
            let remove_keys: Vec<&String> = remove.iter().collect();
            tracing::info!(
                component = "cache_client",
                add_keys = ?add_keys,
                update_keys = ?update_keys,
                remove_keys = ?remove_keys,
                "Processing compressed events"
            );

            // Process events with immutable borrow
            if let Some(handler) = cache.handler.as_ref() {
                handler.processor.partial_update(add, update, remove);
            } else {
                tracing::error!(
                    component = "cache_client",
                    "Handler was removed before processing events"
                );
                return false;
            }

            // Clear events with mutable borrow
            if let Some(handler) = cache.handler.as_mut() {
                handler.compressed_events.clear();
            }
        }

        true // Continue
    }

    // Compress events methods
    pub(crate) fn add_compress_event(&mut self, key: String, change: ResourceChange) {
        // Only add event if handler exists (which means processor is set)
        // Events added before processor is set will be lost, but that's acceptable
        // as the processor will do a full_set on reset anyway
        if let Some(ref mut handler) = self.handler {
            handler.compressed_events.add_event(key, change);
        }
    }
}

pub struct CompressEvent {
    events: HashMap<String, Vec<ResourceChange>>,
}

impl CompressEvent {
    pub fn new() -> Self {
        Self { events: HashMap::new() }
    }

    /// Add an event for a resource key
    pub fn add_event(&mut self, key: String, change: crate::core::conf_sync::traits::ResourceChange) {
        self.events.entry(key).or_default().push(change);
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
