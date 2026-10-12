//! Backend selector
//!
//! Provides a unified backend selection strategy that handles configuration validation
//! and selection from generation-owned state.

use super::weighted_selector::WeightedRoundRobin;

/// Error codes for backend selection configuration
pub const ERR_NO_BACKEND_REFS: u32 = 1;
pub const ERR_INCONSISTENT_WEIGHT: u32 = 2;
/// The weighted round-robin selector could not be constructed (invariant violation).
/// Includes the weighted selector item-count limit. Construction failures remain
/// fail-closed selection errors rather than panics.
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

/// Backend selection plan compiled once for its owning route generation.
///
/// The plan is immutable; weighted selection retains its shared atomic cursor.
///
/// # Type Parameters
/// * `T` - The type of backend items (must implement Clone)
pub struct BackendSelector<T> {
    state: SelectorState<T>,
}

impl<T: Clone> BackendSelector<T> {
    /// Compile items and optional weights into a selection plan.
    ///
    /// Invalid or empty configuration is captured as a selection error. The
    /// owning generation publishes the completed plan and never replaces it.
    pub fn new(items: Vec<T>, weights: Vec<Option<i32>>) -> Self {
        Self {
            state: Self::create_state(items, weights),
        }
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

        // Positive weights and matching lengths are validated above. The WRR
        // item-count limit can still reject construction; preserve that error
        // as a fail-closed selection outcome.
        let (items, weights): (Vec<T>, Vec<usize>) = valid_items.into_iter().unzip();
        match WeightedRoundRobin::new(items, weights) {
            Ok(wrr) => SelectorState::Multiple(wrr),
            Err(_) => SelectorState::Error(ERR_WRR_BUILD_FAILED),
        }
    }

    /// Select a backend.
    ///
    /// Returns Ok(T) on success, or Err(error_code) on error.
    pub fn select(&self) -> Result<T, u32> {
        match &self.state {
            SelectorState::Error(err) => Err(*err),
            SelectorState::Single(backend) => Ok(backend.clone()),
            SelectorState::Multiple(selector) => Ok(selector.select().clone()),
        }
    }
}
