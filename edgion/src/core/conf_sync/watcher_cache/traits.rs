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
