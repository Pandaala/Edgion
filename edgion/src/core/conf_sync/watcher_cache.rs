use std::collections::HashMap;
use std::sync::RwLock;

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
}

impl<T: Versionable> WatcherCache<T> {
    pub fn new(capacity: u32) -> Self {
        Self {
            capacity,
            cache: Vec::new(),
            start_index: 0,
            end_index: 0,
            resource_version: 0,
            ready: false,
            data: HashMap::new(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn set_ready(&mut self) {
        self.ready = true;
    }

    pub fn init_add(&mut self, resource: T) {
        self.data
            .insert(resource.get_version().to_string(), resource);
    }
}
