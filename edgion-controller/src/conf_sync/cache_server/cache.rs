use kube::{Resource, ResourceExt};
use parking_lot::RwLock;
use serde::Serialize;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{mpsc, Notify};

use super::store::EventStore;
use super::types::{EventType, ListData, WatchClient, WatchResponse};
use crate::conf_mgr::sync_runtime::resource_version;
use crate::conf_sync::conf_server::{WatchObj, WatchResponseSimple};
use edgion_common::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use edgion_resources::ResourceMeta;

pub struct ServerCache<T: ResourceMeta + Resource + Send + Sync> {
    // wait for init complete
    ready: RwLock<bool>,

    store: Arc<RwLock<EventStore<T>>>,

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
            notify: Arc::new(Notify::new()),
        }
    }

    /// Get a clone of the shared notify for watchers
    fn get_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Get a clone of the store for watchers
    fn get_store(&self) -> Arc<RwLock<EventStore<T>>> {
        self.store.clone()
    }

    pub fn is_ready(&self) -> bool {
        *self.ready.read()
    }

    /// List all resources as owned (cloned) values with their sync version.
    pub fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let store_guard = self.store.read();
        let (data, sync_version) = store_guard.snapshot_owned();
        ListData::new(data, sync_version)
    }

    /// Get a single resource by key.
    /// Key format: `"namespace/name"` for namespaced resources, or just `"name"` for cluster-scoped.
    pub fn get_by_key(&self, key: &str) -> Option<T>
    where
        T: Clone,
    {
        let store_guard = self.store.read();
        store_guard.get_by_key(key)
    }

    pub fn list_keys(&self) -> Vec<String> {
        let store_guard = self.store.read();
        store_guard.keys()
    }

    /// Number of resources currently stored (O(1)).
    pub fn len(&self) -> usize {
        let store_guard = self.store.read();
        store_guard.len()
    }

    /// Start a watcher task that listens for notifications and sends data
    /// Only needs the store to access data
    ///
    /// The task will exit when:
    /// - Receiver is dropped (detected via sender.closed())
    /// - Store returns an error
    /// - Events are lost (version jumped)
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
                    let store_guard = store.read();
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
                            *last_send_time.write() = Some(SystemTime::now());
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
                                    edgion_resources::WATCH_ERR_EVENTS_LOST.to_string(),
                                    current_version,
                                );
                                let _ = sender.send(response).await;
                                break;
                            } else {
                                // Wait for notification OR receiver drop (via sender.closed())
                                // This ensures the task exits when gRPC layer detects server_id change
                                tokio::select! {
                                    _ = notify.notified() => {
                                        // New data may be available, continue loop
                                    }
                                    _ = sender.closed() => {
                                        // Receiver dropped, exit task
                                        tracing::debug!(
                                            client_id = %watcher.client_id,
                                            "Watcher task exiting: receiver dropped"
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    },
                    Err(err) => {
                        tracing::error!(
                            component = "cache_server",
                            client_id = %watcher.client_id,
                            error = %err,
                            "Watcher error fetching events"
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
        tracing::info!(
            component = "cache_server",
            client_id = %client_id,
            client_name = %client_name,
            from_version = from_version,
            ready = *self.ready.read(),
            "New watch request"
        );
        // Use bounded channel for data to provide backpressure
        let (data_tx, data_rx) = mpsc::channel(100);

        let notify = self.get_notify();
        let store = self.get_store();

        let watcher = WatchClient {
            client_id: client_id.clone(),
            client_name: client_name.clone(),
            from_version,
            sender: data_tx,
            watch_start_time: SystemTime::now(),
            send_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_send_time: Arc::new(parking_lot::RwLock::new(None)),
        };

        tracing::info!(
            component = "cache_server",
            client_id = %watcher.client_id,
            "Watcher registered"
        );

        Self::start_watcher_task(store, notify, watcher);

        data_rx
    }

    /// Add event to the circular queue.
    ///
    /// Applies the event inline (synchronously) instead of spawning a new task,
    /// guaranteeing that write order follows the caller's invocation order and
    /// eliminating the tokio-scheduler-induced reordering window that could cause
    /// stale state to overwrite newer state in the EventStore.
    fn push_event(&self, event_type: EventType, resource: T, sync_version: u64)
    where
        T: Clone + Send + 'static,
    {
        if !self.is_ready() {
            // This is expected during initial sync - watchers start receiving events before
            // the cache is marked as ready. Use trace level to avoid log spam during startup.
            tracing::trace!("Pushing event while cache not yet ready (expected during initial sync)");
        }

        {
            let mut store_guard = self.store.write();
            store_guard.apply_event(event_type, resource, sync_version);
        }
        self.notify.notify_waiters();
    }
}

/// WatchObj impl for `ServerCache<T>`.
impl<T> WatchObj for ServerCache<T>
where
    T: ResourceMeta + Resource + Clone + Send + Sync + Serialize + 'static,
{
    fn kind_name(&self) -> &'static str {
        T::kind_name()
    }

    fn list_json(&self) -> Result<(String, u64), String> {
        let store_guard = self.store.read();
        store_guard
            .serialize_snapshot_json()
            .map_err(|e| format!("Failed to serialize {} data: {}", T::kind_name(), e))
    }

    fn current_version(&self) -> u64 {
        self.store.read().sync_version()
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
            loop {
                tokio::select! {
                    // Forward data from upstream
                    response = typed_rx.recv() => {
                        match response {
                            Some(response) => {
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
                            None => break, // Upstream closed
                        }
                    }
                    // Exit when downstream receiver is dropped
                    _ = simple_tx.closed() => {
                        break;
                    }
                }
            }
        });

        simple_rx
    }

    fn is_ready(&self) -> bool {
        *self.ready.read()
    }

    fn set_ready(&self) {
        *self.ready.write() = true;
    }

    fn set_not_ready(&self) {
        *self.ready.write() = false;
    }

    fn clear(&self) {
        let mut store_guard = self.store.write();
        store_guard.clear();
        drop(store_guard);
        self.notify.notify_waiters();
    }
}

impl<T: ResourceMeta + Resource + Clone + Send + Sync + 'static> CacheEventDispatch<T> for ServerCache<T> {
    fn apply_change(&self, change: ResourceChange, resource: T, sv: Option<u64>)
    where
        T: Resource + Send + 'static,
    {
        let sync_version = sv.unwrap_or_else(resource_version::next_resource_version);

        tracing::info!(
            component = "cache_server",
            event = "apply_change",
            change = ?change,
            kind = T::kind_name(),
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
                    kind = T::kind_name(),
                    "Cache initialization started"
                );
            }
            ResourceChange::InitAdd => {
                let mut store_guard = self.store.write();
                store_guard.init_add(sync_version, resource);
            }
            ResourceChange::InitDone => {
                // Signal: initialization complete, mark cache as ready
                self.set_ready();
                tracing::info!(
                    component = "cache_server",
                    kind = T::kind_name(),
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
        *self.ready.write() = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_common::conf_sync::traits::ResourceChange;
    use edgion_common::conf_sync::CacheEventDispatch;
    use edgion_resources::{ResourceKind, ResourceMeta};
    use kube::api::ObjectMeta;
    use serde::{Deserialize, Serialize};

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
        fn get_sync_version(&self) -> u64 {
            0
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

    fn make_resource(name: &str, version: u64) -> TestResource {
        TestResource {
            name: name.to_string(),
            version,
            metadata: ObjectMeta {
                name: Some("test-resource".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn event_add_stores_resource_and_updates_version() {
        let cache = ServerCache::<TestResource>::new(10);
        let resource = make_resource("foo", 1);

        cache.apply_change(ResourceChange::EventAdd, resource.clone(), None);

        let snapshot = cache.list_owned();
        assert_eq!(snapshot.data.len(), 1);
        assert_eq!(snapshot.data[0], resource);
        // sync_version is independently generated; verify it's set.
        assert!(snapshot.sync_version > 0);
    }

    #[tokio::test]
    async fn event_update_replaces_existing_resource() {
        let cache = ServerCache::<TestResource>::new(10);
        let original = make_resource("foo", 1);
        let updated = make_resource("foo-updated", 2);

        CacheEventDispatch::set_ready(&cache);

        cache.apply_change(ResourceChange::EventAdd, original.clone(), None);
        cache.apply_change(ResourceChange::EventUpdate, updated.clone(), None);

        let snapshot = cache.list_owned();
        assert_eq!(snapshot.data.len(), 1);
        assert_eq!(snapshot.data[0], updated);
        // sync_version is independently generated; verify it's set.
        assert!(snapshot.sync_version > 0);
    }

    #[tokio::test]
    async fn event_delete_removes_resource() {
        let cache = ServerCache::<TestResource>::new(10);
        let resource = make_resource("foo", 42);
        let resource_delete = make_resource("foo", 43);

        CacheEventDispatch::set_ready(&cache);

        cache.apply_change(ResourceChange::EventAdd, resource.clone(), None);
        cache.apply_change(ResourceChange::EventDelete, resource_delete.clone(), None);

        let snapshot = cache.list_owned();
        assert!(snapshot.data.is_empty());
        // sync_version is independently generated; verify it's set.
        assert!(snapshot.sync_version > 0);
    }

    /// Rapid sequential apply_change calls must leave the cache in the state of the
    /// LAST call, not some arbitrary scheduler-determined order.
    #[tokio::test]
    async fn rapid_updates_preserve_latest_state() {
        let cache = ServerCache::<TestResource>::new(200);
        CacheEventDispatch::set_ready(&cache);

        // Issue 50 rapid updates sequentially.
        let n: u64 = 50;
        for i in 1..=n {
            let r = make_resource(&format!("state-{}", i), i);
            cache.apply_change(ResourceChange::EventUpdate, r, None);
        }

        // Final snapshot must reflect the last update
        let snapshot = cache.list_owned();
        assert_eq!(snapshot.data.len(), 1, "only one item in the store");
        assert_eq!(
            snapshot.data[0].name,
            format!("state-{}", n),
            "cache must contain the last (highest-versioned) state after rapid updates"
        );
        assert!(snapshot.sync_version > 0);
    }

    #[tokio::test]
    async fn current_version_tracks_store_version() {
        let cache = ServerCache::<TestResource>::new(10);
        assert_eq!(WatchObj::current_version(&cache), 0);

        cache.apply_change(ResourceChange::EventAdd, make_resource("foo", 1), Some(42));
        assert_eq!(WatchObj::current_version(&cache), 42);

        // Contract: same store list_json serializes.
        let (_json, v) = cache.list_json().unwrap();
        assert_eq!(v, 42);
    }
}
