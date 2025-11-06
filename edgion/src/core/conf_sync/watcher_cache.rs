use std::collections::HashMap;
use tokio::sync::mpsc;

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
pub struct PendingWatch {
    pub client_id: String,
    pub from_version: u64,
    pub sender: tokio::sync::mpsc::UnboundedSender<()>,
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
    watchers: Vec<PendingWatch>,
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
        }
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
    /// Only sends a signal, watchers will read data themselves
    fn notify_watchers(&mut self) {
        // Keep only watchers that are still connected
        self.watchers.retain(|watcher| {
            // Try to send notification signal (non-blocking)
            // If send fails, the receiver has been dropped, so we remove it
            watcher.sender.send(()).is_ok()
        });
    }

    /// Register a watcher and return a receiver for notifications
    /// The caller should loop on the receiver and read data when notified
    pub fn register_watcher(
        &mut self,
        client_id: String,
        from_version: u64,
    ) -> mpsc::UnboundedReceiver<()> {
        let (sender, receiver) = mpsc::unbounded_channel();

        let pending_watch = PendingWatch {
            client_id,
            from_version,
            sender,
        };

        self.watchers.push(pending_watch);
        receiver
    }

    /// Watch for changes starting from a specific version
    /// Returns a stream-like interface where the caller can continuously receive updates
    pub async fn watch(&mut self, client_id: String, from_version: u64) -> WatchResponse<T>
    where
        T: Clone,
    {
        if from_version < self.resource_version {
            // Client is behind, return all events since their version
            let events = self.get_events_since(from_version);
            WatchResponse::new(events, self.resource_version)
        } else {
            // Client is up to date, register and wait for notification
            let mut receiver = self.register_watcher(client_id, from_version);

            // Wait for notification signal
            if receiver.recv().await.is_some() {
                // Got notification, read latest events
                let events = self.get_events_since(from_version);
                WatchResponse::new(events, self.resource_version)
            } else {
                // Channel closed, return empty response
                WatchResponse::new(Vec::new(), self.resource_version)
            }
        }
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
