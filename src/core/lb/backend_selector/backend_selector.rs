//! Backend selector
//!
//! Provides a unified backend selection strategy that handles configuration validation
//! and selection with lazy initialization.

use super::weighted_selector::WeightedRoundRobin;
use arc_swap::ArcSwap;
use std::sync::Arc;

/// Error codes for backend selection configuration
pub const ERR_NO_BACKEND_REFS: u32 = 1;
pub const ERR_INCONSISTENT_WEIGHT: u32 = 2;

/// Internal selector state
enum SelectorState<T> {
    /// Configuration error - store the error code for reporting
    Error(u32),
    /// Single valid backend - no selection needed, return directly
    Single(T),
    /// Multiple valid backends - use weighted round-robin selection
    Multiple(WeightedRoundRobin<T>),
}

/// Backend selector with lazy initialization
///
/// Wraps backend configuration and lazily initializes the actual selector
/// on first use. Thread-safe using ArcSwap internally.
///
/// # Type Parameters
/// * `T` - The type of backend items (must implement Clone)
pub struct BackendSelector<T> {
    /// Cached selector state (lazily initialized)
    state: ArcSwap<Option<SelectorState<T>>>,
}

impl<T> Default for BackendSelector<T> {
    fn default() -> Self {
        Self {
            state: ArcSwap::from_pointee(None),
        }
    }
}

impl<T: Clone> BackendSelector<T> {
    /// Create a new uninitialized BackendSelector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the selector is initialized.
    pub fn is_initialized(&self) -> bool {
        self.state.load().is_some()
    }

    /// Initialize the selector from items and their optional weights.
    ///
    /// This should be called once when the route is first used.
    /// If already initialized, this is a no-op.
    ///
    /// # Arguments
    /// * `items` - A vector of backend items
    /// * `weights` - A vector of optional weights for each item
    pub fn init(&self, items: Vec<T>, weights: Vec<Option<i32>>) {
        // Already initialized, skip
        if self.state.load().is_some() {
            return;
        }

        let state = Self::create_state(items, weights);
        self.state.store(Arc::new(Some(state)));
    }

    /// Create selector state from items and weights.
    fn create_state(items: Vec<T>, weights: Vec<Option<i32>>) -> SelectorState<T> {
        if items.is_empty() {
            return SelectorState::Error(ERR_NO_BACKEND_REFS);
        }

        // Validate weight configuration: either all have weight or none have weight
        let has_any_weight = weights.iter().any(|w| w.is_some());
        let all_have_weight = weights.iter().all(|w| w.is_some());
        if has_any_weight && !all_have_weight {
            return SelectorState::Error(ERR_INCONSISTENT_WEIGHT);
        }

        // Filter out items with weight = 0 and collect valid items with their weights
        let valid_items: Vec<(T, usize)> = items
            .into_iter()
            .zip(weights.into_iter())
            .filter_map(|(item, weight)| {
                let w = weight.unwrap_or(1);
                if w > 0 {
                    Some((item, w as usize))
                } else {
                    None
                }
            })
            .collect();

        // Check if we have any valid items after filtering
        if valid_items.is_empty() {
            return SelectorState::Error(ERR_NO_BACKEND_REFS);
        }

        // If only one valid item, return Single
        if valid_items.len() == 1 {
            let (item, _) = valid_items.into_iter().next().unwrap();
            return SelectorState::Single(item);
        }

        // Multiple valid items - create weighted round-robin selector
        let (items, weights): (Vec<T>, Vec<usize>) = valid_items.into_iter().unzip();
        SelectorState::Multiple(WeightedRoundRobin::new(items, weights))
    }

    /// Select a backend.
    ///
    /// Must be called after init(). Returns Ok(T) on success, or Err(error_code) on error.
    pub fn select(&self) -> Result<T, u32> {
        let guard = self.state.load();
        match &**guard {
            Some(state) => match state {
                SelectorState::Error(err) => Err(*err),
                SelectorState::Single(backend) => Ok(backend.clone()),
                SelectorState::Multiple(selector) => Ok(selector.select().clone()),
            },
            None => Err(ERR_NO_BACKEND_REFS),
        }
    }
}
