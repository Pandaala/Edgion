//! ServerCacheObj trait definition
//!
//! This trait abstracts the public interface of ServerCache<T> to eliminate generic type parameters,
//! allowing us to store different cache types in a HashMap<String, Arc<dyn ServerCacheObj>>.

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

/// Trait that abstracts ServerCache's public interface
///
/// This trait is object-safe and allows us to:
/// 1. Store different ServerCache<T> types in a single HashMap
/// 2. Call list/watch without knowing the concrete type T
/// 3. Manage cache state (ready/not ready, clear)
pub trait ServerCacheObj: Send + Sync {
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

    /// Get the resource kind name (e.g., "HTTPRoute", "Gateway")
    fn kind_name(&self) -> &'static str;

    /// Set cache to ready state
    fn set_ready(&self);

    /// Set cache to not ready state
    fn set_not_ready(&self);

    /// Check if cache is ready
    fn is_ready(&self) -> bool;

    /// Clear all data from the cache
    fn clear(&self);
}
