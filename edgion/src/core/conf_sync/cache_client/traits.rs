use std::collections::{HashMap, HashSet};

/// Configuration ConfHandler trait for handling resource lifecycle operations
/// 
/// This trait must be Send + Sync to allow safe sharing across threads
pub trait ConfHandler<T>: Send + Sync {
    /// Full rebuild with a complete set of resources
    /// This is typically called during initial sync or when a complete refresh is needed
    fn full_build(&mut self, data: &HashMap<String, T>);

    /// Add a new resource
    /// For remove, only the key is needed since the resource is already deleted
    fn conf_change(&mut self, add_or_update: HashMap<String, T>, remove: HashSet<String>);

    /// Trigger a rebuild/refresh of the configuration
    /// This is called when the processor needs to update its internal state
    fn update_rebuild(&mut self);
}

