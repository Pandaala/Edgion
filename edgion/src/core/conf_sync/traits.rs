pub trait EventDispatcher {
    /// Initialize by adding a resource
    fn init_add(&mut self, data: String, resource_version: Option<u64>);

    /// Set the dispatcher as ready
    fn set_ready(&mut self, data: String, resource_version: Option<u64>);

    /// Handle add event
    fn event_add(&mut self, data: String, resource_version: Option<u64>);

    /// Handle update event
    fn event_update(&mut self, data: String, resource_version: Option<u64>);

    /// Handle delete event
    fn event_del(&mut self, data: String, resource_version: Option<u64>);
}
