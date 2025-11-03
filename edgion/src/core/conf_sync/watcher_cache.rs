use std::sync::RwLock;

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
    capacity: u32,
    cache: Vec<WatcherEvent<T>>,
    start_index: u32,
    end_index: u32,
    resource_version: u64,
}

pub struct WatcherCacheSafe<T> {
    cache: RwLock<WatcherCache<T>>,
}