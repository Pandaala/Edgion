use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Notify};

/// Watcher client information
#[derive(Debug, Clone)]
pub struct WatcherClient {
    pub client_id: String,
    pub client_name: String,
    pub current_resource_version: u64,
}

impl WatcherClient {
    pub fn new(client_id: String, client_name: String) -> Self {
        Self {
            client_id,
            client_name,
            current_resource_version: 0,
        }
    }
}

/// List data response structure
#[derive(Debug, Clone)]
pub struct ListData<T> {
    pub data: Vec<T>,
    pub resource_version: u64,
}

impl<T> ListData<T> {
    pub fn new(data: Vec<T>, resource_version: u64) -> Self {
        Self {
            data,
            resource_version,
        }
    }
}

/// Watch response structure containing events and current version
#[derive(Debug, Clone)]
pub struct WatchResponse<T> {
    pub events: Vec<WatcherEvent<T>>,
    pub resource_version: u64,
}

impl<T> WatchResponse<T> {
    pub fn new(events: Vec<WatcherEvent<T>>, resource_version: u64) -> Self {
        Self {
            events,
            resource_version,
        }
    }
}

/// Pending watch request waiting for notification
pub struct PendingWatch<T> {
    pub client_id: String,
    pub from_version: u64,
    pub sender: mpsc::Sender<WatchResponse<T>>, // Bounded for data
}

/// Trait for resources that have a version
pub trait Versionable {
    /// Get the resource version
    fn get_version(&self) -> u64;
}

/// Trait for handling resource events
pub trait EventDispatch<T> {
    /// Initialize by adding a resource
    fn init_add(&mut self, resource: T);

    /// Set the dispatcher as ready
    fn set_ready(&mut self);

    /// Handle add event
    fn event_add(&mut self, resource: T);

    /// Handle update event
    fn event_update(&mut self, resource: T);

    /// Handle delete event
    fn event_del(&mut self, resource: T);
}

#[derive(Debug, Clone)]
pub enum EventType {
    Update,
    Delete,
    Add,
}

#[derive(Debug, Clone)]
pub struct WatcherEvent<T> {
    pub event_type: EventType,
    pub resource_version: u64,
    pub data: T,
}

pub struct WatcherCache<T> {
    // data
    data: HashMap<String, T>,

    // event queue
    capacity: u32,
    cache: Vec<WatcherEvent<T>>,
    start_index: u32,
    end_index: u32,
    resource_version: u64,
    ready: bool,

    // pending watch requests
    watchers: Vec<PendingWatch<T>>,

    // shared notify for broadcasting events to all watchers
    notify: Arc<Notify>,
}

impl<T: Versionable> WatcherCache<T> {
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity,
            cache: Vec::with_capacity(capacity as usize),
            start_index: 0,
            end_index: 0,
            resource_version: 0,
            ready: false,
            data: HashMap::new(),
            watchers: Vec::new(),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Get a clone of the shared notify for watchers
    pub fn get_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// List all data - returns all resources in the cache with resource version
    /// This is typically called by clients to get the full snapshot of data
    pub fn list(&self) -> ListData<&T> {
        let data: Vec<&T> = self.data.values().collect();
        ListData::new(data, self.resource_version)
    }

    /// List all data as owned values (cloned)
    /// Useful when clients need owned data instead of references
    pub fn list_owned(&self) -> ListData<T>
    where
        T: Clone,
    {
        let data: Vec<T> = self.data.values().cloned().collect();
        ListData::new(data, self.resource_version)
    }

    /// Get all events since a specific version from the cache
    /// Returns events where resource_version > from_version
    fn get_events_since(&self, from_version: u64) -> Vec<WatcherEvent<T>>
    where
        T: Clone,
    {
        let mut events = Vec::new();

        // Calculate the number of events in the circular queue
        let count = if (self.cache.len() as u32) < self.capacity {
            self.cache.len() as u32
        } else {
            self.capacity
        };

        // Iterate through the circular queue
        for i in 0..count {
            let index = ((self.start_index + i) % self.capacity) as usize;
            if index < self.cache.len() {
                let event = &self.cache[index];
                if event.resource_version > from_version {
                    events.push(event.clone());
                }
            }
        }

        events
    }

    /// Notify all pending watchers (non-blocking)
    /// Uses shared Notify to broadcast to all watchers at once
    fn notify_watchers(&mut self) {
        // Notify all waiting tasks at once - much more efficient!
        self.notify.notify_waiters();
    }

    /// Start a watcher task that listens for notifications and sends data
    /// Requires Arc<RwLock<WatcherCache>> to access cache from the spawned task
    pub fn start_watcher_task(
        cache: Arc<tokio::sync::RwLock<Self>>,
        notify: Arc<Notify>,
        sender: mpsc::Sender<WatchResponse<T>>,
        _client_id: String,
        mut from_version: u64,
    ) where
        T: Clone + Send + Sync + 'static,
    {
        tokio::spawn(async move {
            loop {
                // Check if there are new events since from_version
                let response = {
                    let cache_guard = cache.read().await;

                    // Get latest version
                    let current_version = cache_guard.resource_version;

                    // If client is behind, get events
                    if from_version < current_version {
                        let events = cache_guard.get_events_since(from_version);
                        from_version = current_version;
                        Some(WatchResponse::new(events, current_version))
                    } else {
                        // Client is up-to-date, no events to send
                        None
                    }
                };

                // Send response if there are events
                if let Some(resp) = response {
                    if sender.send(resp).await.is_err() {
                        // Client disconnected, exit loop
                        break;
                    }
                }

                // Wait for next notification
                notify.notified().await;
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
        cache: std::sync::Arc<tokio::sync::RwLock<Self>>,
        client_id: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponse<T>>
    where
        T: Clone + Send + Sync + 'static,
    {
        // Use bounded channel for data to provide backpressure
        let (data_tx, data_rx) = mpsc::channel(100);

        // Get the shared notify and register watcher
        let notify = {
            let mut cache_guard = cache.blocking_write();
            let notify = cache_guard.get_notify();
            let pending_watch = PendingWatch {
                client_id: client_id.clone(),
                from_version,
                sender: data_tx.clone(),
            };
            cache_guard.watchers.push(pending_watch);
            notify
        };

        // Start the watcher task with shared notify
        Self::start_watcher_task(cache, notify, data_tx, client_id, from_version);

        data_rx
    }

    /// Add event to the circular queue
    fn add_event(&mut self, event_type: EventType, resource: T)
    where
        T: Clone,
    {
        let version = resource.get_version();
        self.resource_version = version;

        let event = WatcherEvent {
            event_type,
            resource_version: version,
            data: resource,
        };

        // Add to circular queue
        if (self.cache.len() as u32) < self.capacity {
            self.cache.push(event.clone());
            self.end_index = self.cache.len() as u32;
        } else {
            let index = (self.end_index % self.capacity) as usize;
            self.cache[index] = event.clone();
            self.end_index = self.end_index.wrapping_add(1);

            // Update start_index if we've overwritten the oldest event
            if self.end_index.wrapping_sub(self.start_index) > self.capacity {
                self.start_index = self.end_index.wrapping_sub(self.capacity);
            }
        }

        // Notify all pending watchers (non-blocking)
        if !self.watchers.is_empty() {
            self.notify_watchers();
        }
    }
}

impl<T: Versionable + Clone> EventDispatch<T> for WatcherCache<T> {
    fn init_add(&mut self, resource: T) {
        let version = resource.get_version();
        self.data.insert(version.to_string(), resource);
    }

    fn set_ready(&mut self) {
        self.ready = true;
    }

    fn event_add(&mut self, resource: T) {
        let version = resource.get_version();
        self.data.insert(version.to_string(), resource.clone());
        self.add_event(EventType::Add, resource);
    }

    fn event_update(&mut self, resource: T) {
        let version = resource.get_version();
        self.data.insert(version.to_string(), resource.clone());
        self.add_event(EventType::Update, resource);
    }

    fn event_del(&mut self, resource: T) {
        let version = resource.get_version();
        self.data.remove(&version.to_string());
        self.add_event(EventType::Delete, resource);
    }
}
