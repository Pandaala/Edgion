//! WatchObj trait definition
//!
//! This trait provides an object-safe interface for list/watch operations.
//! It's designed to be implemented by ServerCache<T> and used by ConfigSyncServer
//! for gRPC list/watch services.

use tokio::sync::mpsc;

/// Simplified watch response for JSON-serialized data
#[derive(Debug, Clone)]
pub struct WatchResponseSimple {
    /// JSON-serialized events
    pub data: String,
    /// Current sync version
    pub sync_version: u64,
    /// Error message if any
    pub err: Option<String>,
}

impl WatchResponseSimple {
    pub fn new(data: String, sync_version: u64) -> Self {
        Self {
            data,
            sync_version,
            err: None,
        }
    }

    pub fn from_error(error: String, sync_version: u64) -> Self {
        Self {
            data: String::new(),
            sync_version,
            err: Some(error),
        }
    }
}

/// Object-safe trait for list/watch operations
///
/// This trait is designed to be implemented by ServerCache<T> and allows
/// ConfigSyncServer to manage different cache types uniformly.
///
/// All methods involve serialization, so using trait objects is appropriate here.
pub trait WatchObj: Send + Sync {
    /// Get the resource kind name (e.g., "HTTPRoute", "Gateway")
    fn kind_name(&self) -> &'static str;

    /// List all resources as JSON string
    ///
    /// Returns (json_data, sync_version) on success
    fn list_json(&self) -> Result<(String, u64), String>;

    /// Watch for changes starting from a specific version
    ///
    /// Returns a receiver that continuously receives JSON-serialized watch responses
    fn watch_json(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponseSimple>;

    /// Check if cache is ready
    fn is_ready(&self) -> bool;

    /// Set cache to ready state
    fn set_ready(&self);

    /// Set cache to not ready state
    fn set_not_ready(&self);

    /// Clear all data from the cache
    fn clear(&self);
}
