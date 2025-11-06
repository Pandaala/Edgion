use std::collections::HashMap;

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

pub enum EventType {
    Update,
    Delete,
    Add,
}

pub struct WatcherEvent<T> {
    pub event_type: EventType,
    resource_version: u64,
    data: T,
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

    // watcher clients
    clients: HashMap<String, WatcherClient>,
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
            clients: HashMap::new(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Add a new watcher client
    pub fn add_client(&mut self, client: WatcherClient) {
        self.clients.insert(client.client_id.clone(), client);
    }

    /// Remove a watcher client by client_id
    pub fn remove_client(&mut self, client_id: &str) -> Option<WatcherClient> {
        self.clients.remove(client_id)
    }

    /// Get a watcher client by client_id
    pub fn get_client(&self, client_id: &str) -> Option<&WatcherClient> {
        self.clients.get(client_id)
    }

    /// Get a mutable reference to a watcher client by client_id
    pub fn get_client_mut(&mut self, client_id: &str) -> Option<&mut WatcherClient> {
        self.clients.get_mut(client_id)
    }

    /// Get all clients
    pub fn get_all_clients(&self) -> Vec<&WatcherClient> {
        self.clients.values().collect()
    }

    /// Update client's current resource version
    pub fn update_client_version(&mut self, client_id: &str, version: u64) -> bool {
        if let Some(client) = self.clients.get_mut(client_id) {
            client.current_resource_version = version;
            true
        } else {
            false
        }
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

    /// Add event to the circular queue
    fn add_event(&mut self, event_type: EventType, resource: T) {
        let version = resource.get_version();
        self.resource_version = version;

        let event = WatcherEvent {
            event_type,
            resource_version: version,
            data: resource,
        };

        // Add to circular queue
        if (self.cache.len() as u32) < self.capacity {
            self.cache.push(event);
            self.end_index = self.cache.len() as u32;
        } else {
            let index = (self.end_index % self.capacity) as usize;
            self.cache[index] = event;
            self.end_index = self.end_index.wrapping_add(1);

            // Update start_index if we've overwritten the oldest event
            if self.end_index.wrapping_sub(self.start_index) > self.capacity {
                self.start_index = self.end_index.wrapping_sub(self.capacity);
            }
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
