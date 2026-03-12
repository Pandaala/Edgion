use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;

/// List data response structure
#[derive(Debug, Clone)]
pub struct ListData<T> {
    pub data: Vec<T>,
    pub sync_version: u64,
}

impl<T> ListData<T> {
    pub fn new(data: Vec<T>, sync_version: u64) -> Self {
        Self { data, sync_version }
    }
}

impl<T: serde::Serialize> ListData<T> {
    /// Serialize the list data to JSON and return (json, sync_version)
    /// Helper to reduce repetitive code in list() methods
    pub fn to_json(&self, type_name: &str) -> Result<(String, u64), String> {
        serde_json::to_string(&self.data)
            .map(|json| (json, self.sync_version))
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
    pub sync_version: u64,
    pub data: T,
}

/// Watch response structure containing events and current version
#[derive(Debug, Clone)]
pub struct WatchResponse<T> {
    pub events: Vec<WatcherEvent<T>>,
    pub sync_version: u64,
    pub err: Option<String>,
}

impl<T> WatchResponse<T> {
    pub fn new(events: Vec<WatcherEvent<T>>, sync_version: u64) -> Self {
        Self {
            events,
            sync_version,
            err: None,
        }
    }

    pub fn from_error(error: String, sync_version: u64) -> Self {
        Self {
            events: Vec::new(),
            sync_version,
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
