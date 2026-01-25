use kube::{Resource, ResourceExt};
use serde::Serialize;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use tokio::sync::{mpsc, Notify};

use super::store::EventStore;
use super::types::{EventType, ListData, WatchClient, WatchResponse};
use crate::core::conf_sync::conf_server::{WatchObj, WatchResponseSimple};
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::core::utils;
use crate::types::ResourceMeta;

pub struct ServerCache<T: ResourceMeta + Resource + Send + Sync> {
    // wait for init complete
    ready: RwLock<bool>,

    // Event storage
    store: Arc<RwLock<EventStore<T>>>,

    // pending watch requests
    watchers: RwLock<Vec<WatchClient<T>>>,

    // shared notify for broadcasting events to all watchers
    notify: Arc<Notify>,
}

impl<T: ResourceMeta + Resource + Send + Sync> ServerCache<T> {
    pub fn new(capacity: u32) -> Self
    where
        T: Clone,
    {
        Self {
            ready: RwLock::new(false),
            store: Arc::new(RwLock::new(EventStore::new(capacity as usize))),
            watchers: RwLock::new(Vec::new()),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Get a clone of the shared notify for watchers
    pub fn get_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Get a clone of the store for watchers
    pub fn get_store(&self) -> Arc<RwLock<EventStore<T>>> {
        self.store.clone()
    }

    pub fn is_ready(&self) -> bool {
        *self.ready.read().unwrap()
    }

    /// Set cache to not ready state
    /// Used during relink to prevent serving stale data
    pub fn set_not_ready(&self) {
        *self.ready.write().unwrap() = false;
    }

    /// Clear all data from the cache
    /// Used during relink to remove stale data
    pub fn clear(&self)
    where
        T: Clone,
    {
        let mut store_guard = self.store.write().unwrap();
        store_guard.clear();
        // Notify watchers that data has changed (they will get error on next fetch)
        self.notify.notify_waiters();
    }

    /// List all data - returns all resources in the cache with resource version
    /// This is typically called by clients to get the full snapshot of data
    pub fn list(&self) -> ListData<T>
    where
        T: Clone,
    {
        self.list_owned()
    }

    /// List all data as owned values (cloned)
    /// Useful when clients need owned data instead of references
    pub fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let store_guard = self.store.read().unwrap();
        let (data, sync_version) = store_guard.snapshot_owned();
        ListData::new(data, sync_version)
    }

    /// Get a single resource by key
    /// Key format: "namespace/name" for namespaced resources, or just "name" for cluster-scoped
    /// Used by Worker to get "previous state" from conf_server_old cache
    pub fn get_by_key(&self, key: &str) -> Option<T>
    where
        T: Clone,
    {
        let store_guard = self.store.read().unwrap();
        store_guard.get_by_key(key)
    }

    /// Start a watcher task that listens for notifications and sends data
    /// Only needs the store to access data
    pub fn start_watcher_task(store: Arc<RwLock<EventStore<T>>>, notify: Arc<Notify>, watcher: WatchClient<T>)
    where
        T: Clone + Send + Sync + 'static,
    {
        tokio::spawn(async move {
            let mut from_version = watcher.from_version;
            let sender = watcher.sender;
            let send_count = watcher.send_count;
            let last_send_time = watcher.last_send_time;

            loop {
                let result = {
                    let store_guard = store.read().unwrap();
                    store_guard.get_events_from_sync_version(from_version)
                };

                match result {
                    Ok((current_version, maybe_events)) => match maybe_events {
                        Some(events) if !events.is_empty() => {
                            let response = WatchResponse::new(events, current_version);

                            if sender.send(response).await.is_err() {
                                tracing::info!("CenterCache receiver dropped, conf_client disconnected");
                                break;
                            }

                            from_version = current_version;

                            send_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if let Ok(mut last_time) = last_send_time.write() {
                                *last_time = Some(SystemTime::now());
                            }
                        }
                        _ => {
                            if current_version > from_version {
                                // Version jumped but no events = events lost (likely due to server restart)
                                // Send error to trigger client relist
                                tracing::warn!(
                                    client_id = %watcher.client_id,
                                    from_version = from_version,
                                    current_version = current_version,
                                    "Events lost detected: version jumped without events, triggering client relist"
                                );
                                let response = WatchResponse::from_error(
                                    crate::types::WATCH_ERR_EVENTS_LOST.to_string(),
                                    current_version,
                                );
                                let _ = sender.send(response).await;
                                break;
                            } else {
                                notify.notified().await;
                            }
                        }
                    },
                    Err(err) => {
                        eprintln!(
                            "[CenterCache] watcher {} error fetching events: {}",
                            watcher.client_id, err
                        );

                        let response = WatchResponse::from_error(err.clone(), from_version);
                        let _ = sender.send(response).await;
                        break;
                    }
                }
            }
        });
    }

    /// Watch for changes starting from a specific version
    /// Returns a receiver that continuously receives WatchResponse updates
    ///
    /// This method will automatically start a watcher task that:
    /// 1. First checks if conf_client is behind and sends initial data
    /// 2. Then loops waiting for notifications and sends updates
    pub fn watch(&self, client_id: String, client_name: String, from_version: u64) -> mpsc::Receiver<WatchResponse<T>>
    where
        T: Clone + Send + Sync + 'static,
    {
        let watchers_len = {
            let watchers = self.watchers.read().unwrap();
            watchers.len()
        };
        tracing::info!(
            component = "cache_server",
            client_id = %client_id,
            client_name = %client_name,
            from_version = from_version,
            ready = *self.ready.read().unwrap(),
            pending_watchers = watchers_len,
            "New watch request"
        );
        // Use bounded channel for data to provide backpressure
        let (data_tx, data_rx) = mpsc::channel(100);

        // Get the shared notify and store
        let notify = self.get_notify();
        let store = self.get_store();

        let watcher = WatchClient {
            client_id: client_id.clone(),
            client_name: client_name.clone(),
            from_version,
            sender: data_tx,
            watch_start_time: SystemTime::now(),
            send_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_send_time: Arc::new(RwLock::new(None)),
        };

        let watcher_clone = watcher.clone();
        {
            let mut watchers = self.watchers.write().unwrap();

            // Lazy cleanup: remove watchers whose receivers have been dropped
            // TODO: need more check on this lazy cleanup logic
            let len_before = watchers.len();
            watchers.retain(|w| !w.sender.is_closed());
            let len_after = watchers.len();

            if len_before > len_after {
                tracing::info!(
                    component = "cache_server",
                    cleaned_count = len_before - len_after,
                    len_before = len_before,
                    len_after = len_after,
                    "Cleaned up disconnected watchers"
                );
            }

            watchers.push(watcher_clone.clone());
        }

        tracing::info!(
            component = "cache_server",
            client_id = %watcher.client_id,
            total_watchers = {
                let watchers = self.watchers.read().unwrap();
                watchers.len()
            },
            "Watcher registered"
        );

        // Start the watcher task - only needs store
        Self::start_watcher_task(store, notify, watcher);

        data_rx
    }

    /// Add event to the circular queue
    fn push_event(&self, event_type: EventType, resource: T, sync_version: u64)
    where
        T: Clone + Send + 'static,
    {
        if !self.is_ready() {
            // This is expected during initial sync - watchers start receiving events before
            // the cache is marked as ready. Use trace level to avoid log spam during startup.
            tracing::trace!("Pushing event while cache not yet ready (expected during initial sync)");
        }

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let store = self.store.clone();
                let notify = self.notify.clone();
                handle.spawn(async move {
                    {
                        let mut store_guard = store.write().unwrap();
                        store_guard.apply_event(event_type, resource, sync_version);
                    }
                    notify.notify_waiters();
                });
            }
            Err(_) => {
                {
                    let mut store_guard = self.store.write().unwrap();
                    store_guard.apply_event(event_type, resource, sync_version);
                }
                self.notify.notify_waiters();
            }
        }
    }
}

/// Implementation of WatchObj trait for ServerCache<T>
/// This allows using ServerCache with the new ConfigSyncServer
impl<T> WatchObj for ServerCache<T>
where
    T: ResourceMeta + Resource + Clone + Send + Sync + Serialize + 'static,
{
    fn kind_name(&self) -> &'static str {
        T::kind_name()
    }

    fn list_json(&self) -> Result<(String, u64), String> {
        let list_data = self.list_owned();
        serde_json::to_string(&list_data.data)
            .map(|json| (json, list_data.sync_version))
            .map_err(|e| format!("Failed to serialize {} data: {}", T::kind_name(), e))
    }

    fn watch_json(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponseSimple> {
        let typed_rx = self.watch(client_id, client_name, from_version);

        // Create a new channel for the simplified response
        let (simple_tx, simple_rx) = mpsc::channel(100);

        // Spawn a task to convert WatchResponse<T> to WatchResponseSimple
        tokio::spawn(async move {
            let mut typed_rx = typed_rx;
            while let Some(response) = typed_rx.recv().await {
                let simple_response = match response.err {
                    Some(err) => WatchResponseSimple::from_error(err, response.sync_version),
                    None => match serde_json::to_string(&response.events) {
                        Ok(json) => WatchResponseSimple::new(json, response.sync_version),
                        Err(e) => WatchResponseSimple::from_error(
                            format!("Serialization error: {}", e),
                            response.sync_version,
                        ),
                    },
                };

                if simple_tx.send(simple_response).await.is_err() {
                    break;
                }
            }
        });

        simple_rx
    }

    fn is_ready(&self) -> bool {
        *self.ready.read().unwrap()
    }

    fn set_ready(&self) {
        *self.ready.write().unwrap() = true;
    }

    fn set_not_ready(&self) {
        *self.ready.write().unwrap() = false;
    }

    fn clear(&self) {
        let mut store_guard = self.store.write().unwrap();
        store_guard.clear();
        self.notify.notify_waiters();
    }
}

impl<T: ResourceMeta + Resource + Clone + Send + Sync + 'static> CacheEventDispatch<T> for ServerCache<T> {
    fn apply_change(&self, change: ResourceChange, resource: T)
    where
        T: Resource + Send + 'static,
    {
        // Generate sync_version independently, no longer dependent on resource's version
        let sync_version = utils::next_resource_version();

        tracing::info!(
            component = "cache_server",
            event = "apply_change",
            change = ?change,
            kind = std::any::type_name::<T>(),
            name = ?resource.name_any(),
            namespace = ?resource.namespace(),
            sync_version = sync_version,
            "apply change"
        );

        match change {
            ResourceChange::InitStart => {
                // Signal only: log initialization start
                tracing::info!(
                    component = "cache_server",
                    kind = std::any::type_name::<T>(),
                    "Cache initialization started"
                );
            }
            ResourceChange::InitAdd => {
                let mut store_guard = self.store.write().unwrap();
                store_guard.init_add(sync_version, resource);
            }
            ResourceChange::InitDone => {
                // Signal: initialization complete, mark cache as ready
                self.set_ready();
                tracing::info!(
                    component = "cache_server",
                    kind = std::any::type_name::<T>(),
                    "Cache initialization done, marked as ready"
                );
            }
            ResourceChange::EventAdd => {
                self.push_event(EventType::Add, resource, sync_version);
            }
            ResourceChange::EventUpdate => {
                self.push_event(EventType::Update, resource, sync_version);
            }
            ResourceChange::EventDelete => {
                self.push_event(EventType::Delete, resource, sync_version);
            }
        }
    }

    fn set_ready(&self) {
        *self.ready.write().unwrap() = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::conf_sync::traits::ResourceChange;
    use crate::core::conf_sync::CacheEventDispatch;
    use crate::types::{ResourceKind, ResourceMeta};
    use kube::api::ObjectMeta;
    use serde::{Deserialize, Serialize};
    use tokio::task::yield_now;
    use tokio::time::{sleep, Duration};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestResource {
        name: String,
        version: u64,
        metadata: ObjectMeta,
    }

    impl PartialEq for TestResource {
        fn eq(&self, other: &Self) -> bool {
            self.name == other.name && self.version == other.version
        }
    }

    impl Eq for TestResource {}

    impl ResourceMeta for TestResource {
        fn get_version(&self) -> u64 {
            self.version
        }

        fn resource_kind() -> ResourceKind {
            ResourceKind::Unspecified
        }

        fn kind_name() -> &'static str {
            "TestResource"
        }

        fn key_name(&self) -> String {
            "test".to_string()
        }
    }

    impl kube::Resource for TestResource {
        type DynamicType = ();
        type Scope = kube::core::ClusterResourceScope;

        fn kind(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
            "TestResource".into()
        }

        fn group(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
            "test.example.com".into()
        }

        fn version(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
            "v1".into()
        }

        fn plural(_: &Self::DynamicType) -> std::borrow::Cow<'static, str> {
            "testresources".into()
        }

        fn meta(&self) -> &ObjectMeta {
            &self.metadata
        }

        fn meta_mut(&mut self) -> &mut ObjectMeta {
            &mut self.metadata
        }
    }

    async fn wait_for_async_store_update() {
        // Ensure spawned tasks have a chance to persist events into the store
        yield_now().await;
        sleep(Duration::from_millis(5)).await;
    }

    #[tokio::test]
    async fn event_add_stores_resource_and_updates_version() {
        let cache = ServerCache::<TestResource>::new(10);
        let resource = TestResource {
            name: "foo".to_string(),
            version: 1,
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };

        cache.apply_change(ResourceChange::EventAdd, resource.clone());
        wait_for_async_store_update().await;

        let snapshot = cache.list_owned();
        assert_eq!(snapshot.data.len(), 1);
        assert_eq!(snapshot.data[0], resource);
        // sync_version is now independently generated, just check it's non-zero
        assert!(snapshot.sync_version > 0);
    }

    #[tokio::test]
    async fn event_update_replaces_existing_resource() {
        let cache = ServerCache::<TestResource>::new(10);
        let original = TestResource {
            name: "foo".to_string(),
            version: 1,
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };
        let updated = TestResource {
            name: "foo-updated".to_string(),
            version: 2, // version must be greater than original
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };

        // Set cache as ready before applying changes
        CacheEventDispatch::set_ready(&cache);

        cache.apply_change(ResourceChange::EventAdd, original.clone());
        wait_for_async_store_update().await;

        cache.apply_change(ResourceChange::EventUpdate, updated.clone());
        wait_for_async_store_update().await;

        let snapshot = cache.list_owned();
        assert_eq!(snapshot.data.len(), 1);
        assert_eq!(snapshot.data[0], updated);
        // sync_version is now independently generated, just check it's non-zero
        assert!(snapshot.sync_version > 0);
    }

    #[tokio::test]
    async fn event_delete_removes_resource() {
        let cache = ServerCache::<TestResource>::new(10);
        let resource = TestResource {
            name: "foo".to_string(),
            version: 42,
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };

        let resource_delete = TestResource {
            name: "foo".to_string(),
            version: 43, // version must be greater than the added resource
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        };

        // Set cache as ready before applying changes
        CacheEventDispatch::set_ready(&cache);

        cache.apply_change(ResourceChange::EventAdd, resource.clone());
        wait_for_async_store_update().await;

        cache.apply_change(ResourceChange::EventDelete, resource_delete.clone());
        wait_for_async_store_update().await;

        let snapshot = cache.list_owned();
        assert!(snapshot.data.is_empty());
        // sync_version is now independently generated, just check it's non-zero
        assert!(snapshot.sync_version > 0);
    }
}
