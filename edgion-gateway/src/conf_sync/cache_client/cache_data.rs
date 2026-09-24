use edgion_common::conf_sync::traits::{ConfHandler, ResourceChange};
use edgion_resources::ResourceMeta;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::Duration;

/// Debounce window for batching compressed events before dispatching to ConfHandler.
/// Prevents rapid config changes (e.g. rolling updates) from triggering excessive ArcSwap rebuilds.
const BATCH_DEBOUNCE: Duration = Duration::from_millis(100);

/// Internal cache data structure combining data and version under single lock.
///
/// Resources are stored as `Arc<T>` to enable zero-copy sharing with handler stores.
/// This eliminates expensive deep clones on the full_set and partial_update paths.
pub struct CacheData<T> {
    ready: bool,
    data: HashMap<String, Arc<T>>,
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
        self.data = resources
            .into_iter()
            .map(|resource| (resource.key_name(), Arc::new(resource)))
            .collect();
        self.sync_version = sync_version;
        if let Some(ref handler) = self.handler {
            handler.processor.full_set(&self.data);
        }
        if let Some(ref mut handler) = self.handler {
            handler.compressed_events.clear();
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.ready
    }

    pub(crate) fn set_ready(&mut self) {
        self.ready = true;
    }

    pub(crate) fn set_not_ready(&mut self) {
        self.ready = false;
    }

    // Data methods
    pub(crate) fn get(&self, key: &str) -> Option<&Arc<T>> {
        self.data.get(key)
    }

    pub(crate) fn insert(&mut self, key: String, value: T) {
        self.data.insert(key, Arc::new(value));
    }

    pub(crate) fn remove(&mut self, key: &str) -> Option<Arc<T>> {
        self.data.remove(key)
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &Arc<T>> {
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

    pub(crate) fn sync_version(&self) -> u64 {
        self.sync_version
    }

    #[cfg(test)]
    pub(crate) fn set_sync_version(&mut self, version: u64) {
        self.sync_version = version;
    }

    pub(crate) fn set_conf_processor(
        &mut self,
        processor: Box<dyn ConfHandler<T> + Send + Sync>,
        cache_data: Arc<RwLock<CacheData<T>>>,
    ) where
        T: Clone + ResourceMeta,
    {
        let handler_data = ConfHandlerData::new(processor);
        let notify = handler_data.notify.clone();
        self.handler = Some(handler_data);

        let cache_data_clone = cache_data.clone();

        tokio::spawn(async move {
            loop {
                notify.notified().await;
                tokio::time::sleep(BATCH_DEBOUNCE).await;

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
        let mut cache = cache_data.write();

        let Some(handler) = cache.handler.as_mut() else {
            tracing::error!(
                component = "cache_client",
                "Handler was removed, stopping compressed events processing task"
            );
            return false; // Handler removed, stop the task
        };

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

        let mut add = HashMap::new();
        let mut update = HashMap::new();
        let mut remove = HashSet::new();

        for (key, events) in event_infos {
            if let Some(resource) = cache.data.get(&key).cloned() {
                // Check event history: if any entry is InitAdd/EventAdd, classify as add; otherwise update.
                let is_add = events
                    .iter()
                    .any(|e| matches!(e, ResourceChange::InitAdd | ResourceChange::EventAdd));

                if is_add {
                    add.insert(key.clone(), resource);
                    tracing::debug!(key = %key, "Processed compressed event: add");
                } else {
                    update.insert(key.clone(), resource);
                    tracing::debug!(key = %key, "Processed compressed event: update");
                }
            } else {
                remove.insert(key.clone());
                tracing::debug!(key = %key, "Processed compressed event: remove");
            }
        }

        if !add.is_empty() || !update.is_empty() || !remove.is_empty() {
            // Info-level log keeps counts only; per-key detail stays at debug to avoid
            // log volume bloat during high-churn batches (see event_dispatch.rs:105 for
            // the established count-only / debug-detail pattern).
            tracing::info!(
                component = "cache_client",
                add_count = add.len(),
                update_count = update.len(),
                remove_count = remove.len(),
                "Processing compressed events"
            );
            tracing::debug!(
                component = "cache_client",
                add_keys = ?add.keys().collect::<Vec<_>>(),
                update_keys = ?update.keys().collect::<Vec<_>>(),
                remove_keys = ?remove.iter().collect::<Vec<_>>(),
                "Compressed event key details"
            );

            if let Some(handler) = cache.handler.as_ref() {
                handler.processor.partial_update(add, update, remove);
            } else {
                tracing::error!(
                    component = "cache_client",
                    "Handler was removed before processing events"
                );
                return false;
            }

            if let Some(handler) = cache.handler.as_mut() {
                handler.compressed_events.clear();
            }
        }

        true // Continue
    }

    pub(crate) fn add_compress_event(&mut self, key: String, change: ResourceChange) {
        // Only add event if handler exists (which means processor is set)
        // Events added before processor is set will be lost, but that's acceptable
        // as the processor will do a full_set on reset anyway
        if let Some(ref mut handler) = self.handler {
            handler.compressed_events.add_event(key, change);
            handler.notify.notify_one();
        }
    }

    #[cfg(test)]
    pub(crate) fn set_conf_processor_for_test(&mut self, processor: Box<dyn ConfHandler<T> + Send + Sync>) {
        self.handler = Some(ConfHandlerData::new(processor));
    }

    #[cfg(test)]
    pub(crate) fn compressed_event_count_for_test(&self) -> usize {
        self.handler
            .as_ref()
            .map_or(0, |handler| handler.compressed_events.events.len())
    }

    #[cfg(test)]
    pub(crate) fn flush_compressed_events_for_test(cache_data: &Arc<RwLock<CacheData<T>>>) -> bool
    where
        T: Clone + ResourceMeta,
    {
        Self::process_compressed_events(cache_data)
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
    pub fn add_event(&mut self, key: String, change: edgion_common::conf_sync::traits::ResourceChange) {
        self.events.entry(key).or_default().push(change);
    }

    /// Clear all events
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// Handler state: processor, compressed-event queue, and notify trigger.
pub struct ConfHandlerData<T> {
    processor: Box<dyn ConfHandler<T> + Send + Sync>,
    compressed_events: CompressEvent,
    notify: Arc<Notify>,
}

impl<T> ConfHandlerData<T> {
    pub(crate) fn new(processor: Box<dyn ConfHandler<T> + Send + Sync>) -> Self {
        Self {
            processor,
            compressed_events: CompressEvent::new(),
            notify: Arc::new(Notify::new()),
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_sync/cache_client/cache_data_test.rs"]
mod tests;
