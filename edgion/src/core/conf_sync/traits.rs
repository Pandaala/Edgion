use crate::types::ResourceKind;

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
