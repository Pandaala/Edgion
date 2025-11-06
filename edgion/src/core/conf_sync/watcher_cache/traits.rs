use tokio::sync::mpsc;

use super::types::{ListData, WatchResponse};

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

/// Trait for cache operations (list and watch)
pub trait CacheOps<T> {
    /// List all data with resource version
    fn list(&self, key: &str) -> Option<ListData<&T>>;

    /// Watch for changes starting from a specific version
    fn watch(
        &mut self,
        key: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponse<T>>>;
}
