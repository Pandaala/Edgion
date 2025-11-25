use std::collections::HashMap;

/// Configuration ConfHandler trait for handling resource lifecycle operations
/// 
/// This trait must be Send + Sync to allow safe sharing across threads
pub trait ConfHandler<T>: Send + Sync {
    /// Full rebuild with a complete set of resources
    /// This is typically called during initial sync or when a complete refresh is needed
    fn full_build(&mut self, data: &HashMap<String, T>);

    /// Add a new resource
    fn add(&mut self, item: T);

    /// Update an existing resource
    fn update(&mut self, item: T);

    /// Delete a resource
    fn del(&mut self, item: T);

    /// Trigger a rebuild/refresh of the configuration
    /// This is called when the processor needs to update its internal state
    fn update_rebuild(&mut self);
}

