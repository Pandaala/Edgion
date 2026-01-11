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
        Self { data, resource_version }
    }
}

impl<T: serde::Serialize> ListData<T> {
    /// Serialize the list data to JSON and return (json, resource_version)
    /// Helper to reduce repetitive code in list() methods
    pub fn to_json(&self, type_name: &str) -> Result<(String, u64), String> {
        serde_json::to_string(&self.data)
            .map(|json| (json, self.resource_version))
            .map_err(|e| format!("Failed to serialize {} data: {}", type_name, e))
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
    pub err: Option<String>,
}

impl<T> WatchResponse<T> {
    pub fn new(events: Vec<WatcherEvent<T>>, resource_version: u64) -> Self {
        Self {
            events,
            resource_version,
            err: None,
        }
    }

    pub fn from_error(error: String, resource_version: u64) -> Self {
        Self {
            events: Vec::new(),
            resource_version,
            err: Some(error),
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
