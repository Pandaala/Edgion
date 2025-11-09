use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{mpsc, Notify};

use super::store::{EventStore, WatchEventError};
use super::traits::{EventDispatch, Versionable};
use super::types::{EventType, ListData, WatchClient, WatchResponse};
use crate::core::conf_sync::traits::ResourceChange;

pub struct CenterCache<T> {
    // wait for init complete
    ready: bool,

    // Event storage
    store: Arc<tokio::sync::RwLock<EventStore<T>>>,

    // pending watch requests
    watchers: Vec<WatchClient<T>>,

    // shared notify for broadcasting events to all watchers
    notify: Arc<Notify>,
}

impl<T: Versionable + Send + Sync> CenterCache<T> {
    pub fn new(capacity: u32) -> Self
    where
        T: Clone,
    {
        Self {
            ready: false,
            store: Arc::new(tokio::sync::RwLock::new(EventStore::new(capacity))),
            watchers: Vec::new(),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Get a clone of the shared notify for watchers
    pub fn get_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Get a clone of the store for watchers
    pub fn get_store(&self) -> Arc<tokio::sync::RwLock<EventStore<T>>> {
        self.store.clone()
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// List all data - returns all resources in the cache with resource version
    /// This is typically called by clients to get the full snapshot of data
    pub async fn list(&self) -> ListData<T>
    where
        T: Clone,
    {
        self.list_owned().await
    }

    /// List all data as owned values (cloned)
    /// Useful when clients need owned data instead of references
    pub async fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let store_guard = self.store.read().await;
        let (data, resource_version) = store_guard.snapshot_owned();
        ListData::new(data, resource_version)
    }

    /// Start a watcher task that listens for notifications and sends data
    /// Only needs the store to access data
    pub fn start_watcher_task(
        store: Arc<tokio::sync::RwLock<EventStore<T>>>,
        notify: Arc<Notify>,
        watcher: WatchClient<T>,
    ) where
        T: Clone + Send + Sync + 'static,
    {
        tokio::spawn(async move {
            let mut from_version = watcher.from_version;
            let sender = watcher.sender;
            let send_count = watcher.send_count;
            let last_send_time = watcher.last_send_time;

            loop {
                let result = {
                    let store_guard = store.read().await;
                    store_guard.get_events_from_resource_version(from_version)
                };

                match result {
                    Ok((current_version, events)) => {
                        if !events.is_empty() {
                            from_version = current_version;
                            let response = WatchResponse::new(events, current_version);

                            if sender.send(response).await.is_err() {
                                // Client disconnected, exit loop
                                break;
                            }

                            // Update send count and time
                            send_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if let Ok(mut last_time) = last_send_time.write() {
                                *last_time = Some(SystemTime::now());
                            }
                        } else if current_version > from_version {
                            from_version = current_version;
                            continue;
                        } else {
                            // Version is up-to-date, wait for next notification
                            notify.notified().await;
                        }
                    }
                    Err(WatchEventError::StaleResourceVersion {
                        requested,
                        oldest_available,
                    }) => {
                        eprintln!(
                            "[CenterCache] watcher {} requested stale version {} (oldest available {}), stopping watcher",
                            watcher.client_id, requested, oldest_available
                        );
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
    /// 1. First checks if client is behind and sends initial data
    /// 2. Then loops waiting for notifications and sends updates
    pub fn watch(
        &mut self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<T>>
    where
        T: Clone + Send + Sync + 'static,
    {
        println!(
            "[CenterCache] new watch request client_id={} client_name={} from_version={} ready={} pending_watchers={}",
            client_id,
            client_name,
            from_version,
            self.ready,
            self.watchers.len()
        );
        // Use bounded channel for data to provide backpressure
        let (data_tx, data_rx) = mpsc::channel(100);

        // Get the shared notify and store
        let notify = self.get_notify();
        let store = self.get_store();

        let watcher = WatchClient {
            client_id,
            client_name,
            from_version,
            sender: data_tx,
            watch_start_time: SystemTime::now(),
            send_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            last_send_time: Arc::new(std::sync::RwLock::new(None)),
        };
        self.watchers.push(watcher.clone());

        println!(
            "[CenterCache] watcher registered client_id={} total_watchers={}",
            watcher.client_id,
            self.watchers.len()
        );

        // Start the watcher task - only needs store
        Self::start_watcher_task(store, notify, watcher);

        data_rx
    }

    /// Add event to the circular queue
    fn push_event(&self, event_type: EventType, resource: T, resource_version: u64)
    where
        T: Clone + Send + 'static,
    {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let store = self.store.clone();
                let notify = self.notify.clone();
                handle.spawn(async move {
                    {
                        let mut store_guard = store.write().await;
                        store_guard.apply_event(event_type, resource, resource_version);
                    }
                    notify.notify_waiters();
                });
            }
            Err(_) => {
                {
                    let mut store_guard = self.store.blocking_write();
                    store_guard.apply_event(event_type, resource, resource_version);
                }
                self.notify.notify_waiters();
            }
        }
    }
}

impl<T: Versionable + Clone + Send + Sync + 'static> EventDispatch<T> for CenterCache<T> {
    fn apply_change(&mut self, change: ResourceChange, resource: T, resource_version: Option<u64>)
    where
        T: Send + 'static,
    {
        let version = resource_version.unwrap_or_else(|| resource.get_version());
        match change {
            ResourceChange::InitAdd => {
                let mut store_guard = self.store.blocking_write();
                store_guard.init_add(version, resource);
            }
            ResourceChange::EventAdd => {
                self.push_event(EventType::Add, resource, version);
            }
            ResourceChange::EventUpdate => {
                self.push_event(EventType::Update, resource, version);
            }
            ResourceChange::EventDelete => {
                self.push_event(EventType::Delete, resource, version);
            }
        }
    }

    fn set_ready(&mut self) {
        self.ready = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::conf_sync::traits::ResourceChange;
    use crate::core::conf_sync::EventDispatch;
    use tokio::task::yield_now;
    use tokio::time::{sleep, Duration};

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestResource {
        name: &'static str,
        version: u64,
    }

    impl Versionable for TestResource {
        fn get_version(&self) -> u64 {
            self.version
        }
    }

    async fn wait_for_async_store_update() {
        // Ensure spawned tasks have a chance to persist events into the store
        yield_now().await;
        sleep(Duration::from_millis(5)).await;
    }

    #[tokio::test]
    async fn event_add_stores_resource_and_updates_version() {
        let mut cache = CenterCache::<TestResource>::new(10);
        let resource = TestResource {
            name: "foo",
            version: 1,
        };

        cache.apply_change(
            ResourceChange::EventAdd,
            resource.clone(),
            Some(resource.version),
        );
        wait_for_async_store_update().await;

        let snapshot = cache.list_owned().await;
        assert_eq!(snapshot.data.len(), 1);
        assert_eq!(snapshot.data[0], resource);
        assert_eq!(snapshot.resource_version, resource.version);
    }

    #[tokio::test]
    async fn event_update_replaces_existing_resource() {
        let mut cache = CenterCache::<TestResource>::new(10);
        let original = TestResource {
            name: "foo",
            version: 1,
        };
        let updated = TestResource {
            name: "foo-updated",
            version: 1,
        };

        cache.apply_change(
            ResourceChange::EventAdd,
            original.clone(),
            Some(original.version),
        );
        wait_for_async_store_update().await;

        cache.apply_change(
            ResourceChange::EventUpdate,
            updated.clone(),
            Some(updated.version),
        );
        wait_for_async_store_update().await;

        let snapshot = cache.list_owned().await;
        assert_eq!(snapshot.data.len(), 1);
        assert_eq!(snapshot.data[0], updated);
        assert_eq!(snapshot.resource_version, updated.version);
    }

    #[tokio::test]
    async fn event_delete_removes_resource() {
        let mut cache = CenterCache::<TestResource>::new(10);
        let resource = TestResource {
            name: "foo",
            version: 42,
        };

        cache.apply_change(
            ResourceChange::EventAdd,
            resource.clone(),
            Some(resource.version),
        );
        wait_for_async_store_update().await;

        cache.apply_change(
            ResourceChange::EventDelete,
            resource.clone(),
            Some(resource.version),
        );
        wait_for_async_store_update().await;

        let snapshot = cache.list_owned().await;
        assert!(snapshot.data.is_empty());
        assert_eq!(snapshot.resource_version, resource.version);
    }
}
