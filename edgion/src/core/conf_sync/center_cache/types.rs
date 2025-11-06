use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;

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

/// Event type enumeration
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Update,
    Delete,
    Add,
}

/// Watcher event structure
#[derive(Debug, Clone, serde::Serialize)]
pub struct WatcherEvent<T> {
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub resource_version: u64,
    pub data: T,
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
#[derive(Clone)]
pub struct WatchClient<T> {
    pub client_id: String,
    pub client_name: String,
    pub from_version: u64,
    pub sender: mpsc::Sender<WatchResponse<T>>,
    pub watch_start_time: SystemTime,
    pub send_count: Arc<std::sync::atomic::AtomicU64>,
    pub last_send_time: Arc<std::sync::RwLock<Option<SystemTime>>>,
}

