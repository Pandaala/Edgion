use crate::core::conf_sync::traits::ResourceChange;

// Re-export ResourceMeta from types
pub use crate::types::ResourceMeta;

/// Trait for handling resource events
pub trait EventDispatch<T> {
    /// Apply a change to the resource cache
    fn apply_change(&self, change: ResourceChange, resource: T)
    where
        T: Send + 'static;

    /// Set the dispatcher as ready
    fn set_ready(&self);
}
