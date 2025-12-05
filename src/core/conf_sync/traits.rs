use crate::types::ResourceKind;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResourceChange {
    InitAdd,
    EventAdd,
    EventUpdate,
    EventDelete,
}

/// Trait for handling resource events in cache
pub trait CacheEventDispatch<T> {
    /// Apply a change to the resource cache
    fn apply_change(&self, change: ResourceChange, resource: T)
    where
        T: Send + 'static;

    /// Set the dispatcher as ready
    fn set_ready(&self);
}

/// Configuration handler trait for handling resource lifecycle operations
/// 
/// This trait must be Send + Sync to allow safe sharing across threads
pub trait ConfHandler<T>: Send + Sync {
    /// Full set with a complete set of resources
    /// This is typically called during initial sync or when a complete refresh is needed
    fn full_set(&self, data: &HashMap<String, T>);

    /// Handle partial configuration updates (add/update/remove)
    /// - add: new resources that didn't exist before
    /// - update: existing resources that are being modified
    /// - remove: keys of resources to be removed
    fn partial_update(&self, add: HashMap<String, T>, update: HashMap<String, T>, remove: HashSet<String>);
}

pub trait ConfigServerEventDispatcher: Send + Sync {
    fn apply_resource_change(&self, change: ResourceChange, resource_type: Option<ResourceKind>, data: String);

    fn enable_version_fix_mode(&self);

    fn set_ready(&self);
}

pub trait ConfigClientEventDispatcher: Send + Sync {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    );
}
