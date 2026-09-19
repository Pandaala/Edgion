//! Backend selector
//!
//! Provides a unified backend selection strategy that handles configuration validation
//! and selection with lazy initialization.

use super::weighted_selector::WeightedRoundRobin;
use arc_swap::ArcSwapOption;
use std::sync::Arc;

/// Error codes for backend selection configuration
pub const ERR_NO_BACKEND_REFS: u32 = 1;
pub const ERR_INCONSISTENT_WEIGHT: u32 = 2;
/// The weighted round-robin selector could not be constructed (invariant violation).
/// Currently unreachable because `create_state` pre-validates all inputs; kept as a
/// fail-close path so a future change cannot turn a `WrrError` into a panic.
pub const ERR_WRR_BUILD_FAILED: u32 = 3;

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
    state: ArcSwapOption<SelectorState<T>>,
}

impl<T> Default for BackendSelector<T> {
    fn default() -> Self {
        Self {
            state: ArcSwapOption::empty(),
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
    /// Idempotent: if already initialized, does nothing.
    ///
    /// # Arguments
    /// * `items` - A vector of backend items
    /// * `weights` - A vector of optional weights for each item
    pub fn init(&self, items: Vec<T>, weights: Vec<Option<i32>>) {
        if self.state.load().is_some() {
            return;
        }

        let state = Self::create_state(items, weights);
        self.state.store(Some(Arc::new(state)));
    }

    /// Create selector state from items and weights.
    fn create_state(items: Vec<T>, weights: Vec<Option<i32>>) -> SelectorState<T> {
        if items.len() != weights.len() {
            return SelectorState::Error(ERR_WRR_BUILD_FAILED);
        }

        if items.is_empty() {
            return SelectorState::Error(ERR_NO_BACKEND_REFS);
        }

        let has_any_weight = weights.iter().any(|w| w.is_some());
        let all_have_weight = weights.iter().all(|w| w.is_some());
        if has_any_weight && !all_have_weight {
            return SelectorState::Error(ERR_INCONSISTENT_WEIGHT);
        }

        let valid_items: Vec<(T, usize)> = items
            .into_iter()
            .zip(weights)
            .filter_map(|(item, weight)| {
                let w = weight.unwrap_or(1);
                if w > 0 {
                    Some((item, w as usize))
                } else {
                    None
                }
            })
            .collect();

        if valid_items.is_empty() {
            return SelectorState::Error(ERR_NO_BACKEND_REFS);
        }

        if valid_items.len() == 1 {
            let (item, _) = valid_items
                .into_iter()
                .next()
                .expect("valid_items.len() == 1 checked above");
            return SelectorState::Single(item);
        }

        // Inputs are validated above (len >= 2, lengths match, all weights > 0)
        // so `new` cannot fail; map any error to a fail-close code rather than
        // unwrapping, in case future pre-validation changes regress.
        let (items, weights): (Vec<T>, Vec<usize>) = valid_items.into_iter().unzip();
        match WeightedRoundRobin::new(items, weights) {
            Ok(wrr) => SelectorState::Multiple(wrr),
            Err(_) => SelectorState::Error(ERR_WRR_BUILD_FAILED),
        }
    }

    /// Select a backend.
    ///
    /// Must be called after init(). Returns Ok(T) on success, or Err(error_code) on error.
    pub fn select(&self) -> Result<T, u32> {
        let guard = self.state.load();
        match guard.as_deref() {
            Some(SelectorState::Error(err)) => Err(*err),
            Some(SelectorState::Single(backend)) => Ok(backend.clone()),
            Some(SelectorState::Multiple(selector)) => Ok(selector.select().clone()),
            None => Err(ERR_NO_BACKEND_REFS),
        }
    }
}
