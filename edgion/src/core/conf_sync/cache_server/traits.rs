use crate::core::conf_sync::traits::ResourceChange;

/// Trait for resources that have a version
pub trait Versionable {
    /// Get the resource version
    fn get_version(&self) -> u64;
}

/// Trait for handling resource events
pub trait EventDispatch<T> {
    /// Apply a change to the resource cache
    fn apply_change(&self, change: ResourceChange, resource: T)
    where
        T: Send + 'static;

    /// Set the dispatcher as ready
    fn set_ready(&self);
}
