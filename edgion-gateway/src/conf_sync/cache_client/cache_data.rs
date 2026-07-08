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
mod tests {
    //! Unit tests for `CacheData<T>` data-structure operations and `CompressEvent`
    //! event accumulation. These tests intentionally avoid the
    //! `set_conf_processor` path (which spawns a tokio background task and is
    //! deferred to integration coverage).
    use super::*;
    use edgion_common::conf_sync::traits::ResourceChange;
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::core::ObjectMeta;

    fn make_cm(namespace: &str, name: &str) -> ConfigMap {
        ConfigMap {
            metadata: ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn new_creates_empty_unready_cache() {
        let cache: CacheData<ConfigMap> = CacheData::new();
        assert!(!cache.is_ready());
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.sync_version(), 0);
        assert!(cache.handler.is_none());
    }

    #[test]
    fn insert_and_get_round_trip() {
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        let cm = make_cm("ns1", "cm-a");
        cache.insert("ns1/cm-a".to_string(), cm);
        assert_eq!(cache.len(), 1);
        let got = cache.get("ns1/cm-a").expect("present");
        assert_eq!(got.metadata.name.as_deref(), Some("cm-a"));
        assert_eq!(got.metadata.namespace.as_deref(), Some("ns1"));
    }

    #[test]
    fn insert_duplicate_key_replaces_value() {
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        let mut cm1 = make_cm("ns1", "cm-a");
        cm1.metadata.resource_version = Some("1".to_string());
        cache.insert("ns1/cm-a".to_string(), cm1);

        let mut cm2 = make_cm("ns1", "cm-a");
        cm2.metadata.resource_version = Some("2".to_string());
        cache.insert("ns1/cm-a".to_string(), cm2);

        // Still a single entry, but with the newer resource_version.
        assert_eq!(cache.len(), 1);
        let got = cache.get("ns1/cm-a").expect("present");
        assert_eq!(got.metadata.resource_version.as_deref(), Some("2"));
    }

    #[test]
    fn remove_existing_returns_some_and_drops_entry() {
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        cache.insert("ns1/cm-a".to_string(), make_cm("ns1", "cm-a"));
        let removed = cache.remove("ns1/cm-a");
        assert!(removed.is_some());
        assert!(cache.is_empty());
        assert!(cache.get("ns1/cm-a").is_none());
    }

    #[test]
    fn remove_missing_returns_none() {
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        assert!(cache.remove("ns1/missing").is_none());
    }

    #[test]
    fn get_missing_returns_none() {
        let cache: CacheData<ConfigMap> = CacheData::new();
        assert!(cache.get("ns1/missing").is_none());
    }

    #[test]
    fn reset_replaces_all_entries_and_updates_sync_version() {
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        cache.insert("ns1/old".to_string(), make_cm("ns1", "old"));
        cache.set_sync_version(7);

        let new_set = vec![make_cm("ns1", "a"), make_cm("ns2", "b"), make_cm("ns2", "c")];
        cache.reset(new_set, 42);

        assert_eq!(cache.len(), 3);
        assert!(cache.get("ns1/old").is_none(), "old key must be evicted");
        assert!(cache.get("ns1/a").is_some());
        assert!(cache.get("ns2/b").is_some());
        assert!(cache.get("ns2/c").is_some());
        assert_eq!(cache.sync_version(), 42);
    }

    #[test]
    fn reset_with_empty_list_clears_everything() {
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        cache.insert("ns1/a".to_string(), make_cm("ns1", "a"));
        cache.insert("ns1/b".to_string(), make_cm("ns1", "b"));
        cache.reset(Vec::new(), 100);
        assert!(cache.is_empty());
        assert_eq!(cache.sync_version(), 100);
    }

    #[test]
    fn ready_state_toggle() {
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        assert!(!cache.is_ready());
        cache.set_ready();
        assert!(cache.is_ready());
        // set_ready is idempotent — second call must not flip back to false.
        cache.set_ready();
        assert!(cache.is_ready());
    }

    #[test]
    fn sync_version_round_trip() {
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        assert_eq!(cache.sync_version(), 0);
        cache.set_sync_version(5);
        assert_eq!(cache.sync_version(), 5);
        cache.set_sync_version(0);
        assert_eq!(cache.sync_version(), 0);
    }

    #[test]
    fn keys_and_values_iterate_inserted_entries() {
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        cache.insert("ns1/a".to_string(), make_cm("ns1", "a"));
        cache.insert("ns1/b".to_string(), make_cm("ns1", "b"));

        let mut keys: Vec<String> = cache.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["ns1/a".to_string(), "ns1/b".to_string()]);

        let value_count = cache.values().count();
        assert_eq!(value_count, 2);
    }

    #[test]
    fn arc_get_returns_strong_reference_into_storage() {
        // Inserted resources are wrapped in `Arc<T>` for zero-copy sharing.
        // `get` must hand out a clone of that Arc, so the strong count grows
        // while the returned reference is alive.
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        cache.insert("ns1/a".to_string(), make_cm("ns1", "a"));
        let arc_in_cache = cache.get("ns1/a").expect("present").clone();
        // One ref is held by the cache, one by `arc_in_cache`.
        assert_eq!(Arc::strong_count(&arc_in_cache), 2);
        // Removing the entry drops the cache's ref; `arc_in_cache` keeps the value alive.
        let _ = cache.remove("ns1/a");
        assert_eq!(Arc::strong_count(&arc_in_cache), 1);
    }

    #[test]
    fn add_compress_event_without_handler_is_no_op() {
        // Without a registered processor, compression events are dropped on
        // the floor. The expected behavior — see `set_conf_processor` doc.
        let mut cache: CacheData<ConfigMap> = CacheData::new();
        cache.add_compress_event("ns1/a".to_string(), ResourceChange::EventAdd);
        // Nothing observable should change.
        assert!(cache.is_empty());
        assert!(cache.handler.is_none());
    }

    #[test]
    fn compress_event_accumulates_per_key() {
        let mut events = CompressEvent::new();
        events.add_event("ns1/a".to_string(), ResourceChange::EventAdd);
        events.add_event("ns1/a".to_string(), ResourceChange::EventUpdate);
        events.add_event("ns1/b".to_string(), ResourceChange::EventDelete);

        assert_eq!(events.events.len(), 2);
        let series_a = events.events.get("ns1/a").expect("a present");
        assert_eq!(series_a.len(), 2);
        assert_eq!(series_a[0], ResourceChange::EventAdd);
        assert_eq!(series_a[1], ResourceChange::EventUpdate);
        let series_b = events.events.get("ns1/b").expect("b present");
        assert_eq!(series_b.len(), 1);
        assert_eq!(series_b[0], ResourceChange::EventDelete);
    }

    #[test]
    fn compress_event_clear_empties_state() {
        let mut events = CompressEvent::new();
        events.add_event("k".to_string(), ResourceChange::EventAdd);
        assert!(!events.events.is_empty());
        events.clear();
        assert!(events.events.is_empty());
    }
}
