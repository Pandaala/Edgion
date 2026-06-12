use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResourceChange {
    /// Signal: initialization started (log only, no resource data)
    InitStart,
    /// Initial sync: adding resource during init
    InitAdd,
    /// Signal: initialization done, cache can be marked as ready
    InitDone,
    /// Runtime event: resource added
    EventAdd,
    /// Runtime event: resource updated
    EventUpdate,
    /// Runtime event: resource deleted
    EventDelete,
}

/// Trait for handling resource events in cache
pub trait CacheEventDispatch<T> {
    /// Apply a change to the resource cache.
    /// When `sv` is `Some`, use the provided sync_version; otherwise generate one internally.
    fn apply_change(&self, change: ResourceChange, resource: T, sv: Option<u64>)
    where
        T: Send + 'static;

    /// Set the dispatcher as ready
    fn set_ready(&self);
}

/// Configuration handler trait for handling resource lifecycle operations
///
/// Resources are wrapped in `Arc<T>` to enable zero-copy sharing between
/// the cache layer and handler stores. This eliminates expensive deep clones
/// on the full_set (relist/reconnect) and partial_update (compressed events) paths.
///
/// This trait must be Send + Sync to allow safe sharing across threads.
pub trait ConfHandler<T>: Send + Sync {
    /// Full set with a complete set of resources
    /// This is typically called during initial sync or when a complete refresh is needed
    fn full_set(&self, data: &HashMap<String, Arc<T>>);

    /// Handle partial configuration updates (add/update/remove)
    /// - add: new resources that didn't exist before
    /// - update: existing resources that are being modified
    /// - remove: keys of resources to be removed
    fn partial_update(&self, add: HashMap<String, Arc<T>>, update: HashMap<String, Arc<T>>, remove: HashSet<String>);
}

impl<T, U, D> ConfHandler<T> for D
where
    D: std::ops::Deref<Target = U> + Send + Sync,
    U: ConfHandler<T> + ?Sized,
{
    fn full_set(&self, data: &HashMap<String, Arc<T>>) {
        self.deref().full_set(data)
    }

    fn partial_update(&self, add: HashMap<String, Arc<T>>, update: HashMap<String, Arc<T>>, remove: HashSet<String>) {
        self.deref().partial_update(add, update, remove)
    }
}

/// Clone `&HashMap<String, Arc<T>>` into `HashMap<String, T>`.
/// Used in `full_set` implementations whose stores need owned values.
pub fn arc_map_to_owned<T: Clone>(data: &HashMap<String, Arc<T>>) -> HashMap<String, T> {
    data.iter().map(|(k, v)| (k.clone(), (**v).clone())).collect()
}

/// Consume `HashMap<String, Arc<T>>` into `HashMap<String, T>`.
/// Used in `partial_update` implementations whose stores need owned values.
pub fn arc_map_into_owned<T: Clone>(data: HashMap<String, Arc<T>>) -> HashMap<String, T> {
    data.into_iter().map(|(k, v)| (k, (*v).clone())).collect()
}
