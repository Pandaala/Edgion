use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Multi-source ConfHandler for Center.
/// Each method receives controller_id so the handler knows which controller the data is from.
/// Mirrors Gateway's ConfHandler<T> but adapted for Center's per-controller aggregation.
pub trait CenterConfHandler<T>: Send + Sync {
    /// Full set for a specific controller (called after list response).
    fn full_set(&self, controller_id: &str, data: &HashMap<String, Arc<T>>);

    /// Partial update for a specific controller (called after watch events).
    fn partial_update(
        &self,
        controller_id: &str,
        add: HashMap<String, Arc<T>>,
        update: HashMap<String, Arc<T>>,
        remove: HashSet<String>,
    );

    /// Controller disconnected (keep data, mark offline).
    fn controller_offline(&self, controller_id: &str);

    /// Controller removed (clear all data for this controller).
    fn controller_removed(&self, controller_id: &str);
}
