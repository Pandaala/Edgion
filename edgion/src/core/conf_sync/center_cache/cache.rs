use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{mpsc, Notify};

use super::store::EventStore;
use super::traits::{EventDispatch, Versionable};
use super::types::{EventType, ListData, WatchClient, WatchResponse, WatcherEvent};

pub struct CenterCache<T> {
    // data
    data: HashMap<String, T>,

    // wait for init complete
    ready: bool,

    // Event storage
    store: Arc<tokio::sync::RwLock<EventStore<T>>>,

    // pending watch requests
    watchers: Vec<WatchClient<T>>,

    // shared notify for broadcasting events to all watchers
    notify: Arc<Notify>,
}

impl<T: Versionable> CenterCache<T> {
    pub fn new(capacity: u32) -> Self
    where
        T: Clone,
    {
        Self {
            data: HashMap::new(),
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
    pub async fn list(&self) -> ListData<&T> {
        let data: Vec<&T> = self.data.values().collect();
        let resource_version = self.store.read().await.get_current_version();
        ListData::new(data, resource_version)
    }

    /// List all data as owned values (cloned)
    /// Useful when clients need owned data instead of references
    pub async fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let data: Vec<T> = self.data.values().cloned().collect();
        let resource_version = self.store.read().await.get_current_version();
        ListData::new(data, resource_version)
    }

    /// Notify all pending watchers (non-blocking)
    /// Uses shared Notify to broadcast to all watchers at once
    fn notify_watchers(&mut self) {
        // Notify all waiting tasks at once - much more efficient!
        self.notify.notify_waiters();
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
                // First check if current version is different from client version
                let current_version = {
                    let store_guard = store.read().await;
                    store_guard.get_current_version()
                };

                // Only fetch events if version has changed
                if from_version < current_version {
                    let (events, current_version) = {
                        let store_guard = store.read().await;
                        store_guard.get_events_from_resource_version(from_version)
                    };

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
                    }
                } else {
                    // Version is up-to-date, wait for next notification
                    notify.notified().await;
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

        // Start the watcher task - only needs store
        Self::start_watcher_task(store, notify, watcher);

        data_rx
    }

    /// Add event to the circular queue
    async fn add_event(&mut self, event_type: EventType, resource: T, resource_version: Option<u64>)
    where
        T: Clone,
    {
        let version = resource_version.unwrap_or_else(|| resource.get_version());

        let event = WatcherEvent {
            event_type,
            resource_version: version,
            data: resource,
        };

        {
            let mut store_guard = self.store.write().await;
            store_guard.mut_update(event);
        }

        // Notify all pending watchers (non-blocking)
        if !self.watchers.is_empty() {
            self.notify_watchers();
        }
    }
}

impl<T: Versionable + Clone + Send + Sync + 'static> EventDispatch<T> for CenterCache<T> {
    fn init_add(&mut self, resource: T, resource_version: Option<u64>) {
        let version = resource_version.unwrap_or_else(|| resource.get_version());
        self.data.insert(version.to_string(), resource);
    }

    fn set_ready(&mut self) {
        self.ready = true;
    }

    fn event_add(&mut self, resource: T, resource_version: Option<u64>)
    where
        T: Send + 'static,
    {
        let version = resource_version.unwrap_or_else(|| resource.get_version());
        let resource_clone = resource.clone();
        self.data.insert(version.to_string(), resource);
        let event = WatcherEvent {
            event_type: EventType::Add,
            resource_version: version,
            data: resource_clone,
        };
        let store = self.store.clone();
        let notify = self.notify.clone();
        // Spawn async task to update store
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                {
                    let mut store_guard = store.write().await;
                    store_guard.mut_update(event);
                }
                notify.notify_waiters();
            });
        }
        // If not in async context, we can't update the store asynchronously
        // The data is already updated in self.data, so this is acceptable
    }

    fn event_update(&mut self, resource: T, resource_version: Option<u64>)
    where
        T: Send + 'static,
    {
        let version = resource_version.unwrap_or_else(|| resource.get_version());
        let resource_clone = resource.clone();
        self.data.insert(version.to_string(), resource);
        let event = WatcherEvent {
            event_type: EventType::Update,
            resource_version: version,
            data: resource_clone,
        };
        let store = self.store.clone();
        let notify = self.notify.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                {
                    let mut store_guard = store.write().await;
                    store_guard.mut_update(event);
                }
                notify.notify_waiters();
            });
        }
    }

    fn event_del(&mut self, resource: T, resource_version: Option<u64>)
    where
        T: Send + 'static,
    {
        let version = resource_version.unwrap_or_else(|| resource.get_version());
        let resource_clone = resource.clone();
        self.data.remove(&version.to_string());
        let event = WatcherEvent {
            event_type: EventType::Delete,
            resource_version: version,
            data: resource_clone,
        };
        let store = self.store.clone();
        let notify = self.notify.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                {
                    let mut store_guard = store.write().await;
                    store_guard.mut_update(event);
                }
                notify.notify_waiters();
            });
        }
    }
}
