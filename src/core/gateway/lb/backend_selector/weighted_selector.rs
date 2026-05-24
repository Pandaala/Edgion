//! Lock-free Smooth Weighted Round Robin selector
//!
//! A thread-safe, lock-free smooth weighted round-robin algorithm for backend selection.
//! Uses pre-computed smooth sequence + atomic counter for O(1) selection.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Upper bound on the precomputed smooth-sequence length.
///
/// The sequence is a `Box<[u16]>` (~2 bytes per entry) whose length equals the
/// sum of the (normalized) weights, so this caps both the persistent allocation
/// (~20 KB worst case) and the O(n) construction cost. 10_000 gives a weight-ratio
/// resolution of 1/10_000 (0.01%), far finer than any realistic backend split.
const MAX_SMOOTH_SEQUENCE_LEN: usize = 10_000;

/// Greatest common divisor (Euclid's algorithm). `gcd(x, 0) == x`.
fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

/// Normalize weights so the precomputed smooth sequence stays bounded while
/// preserving the traffic ratio. Smooth-WRR fairness depends only on the ratio
/// between weights, not their absolute magnitude — without this an unclamped
/// `BackendRef.weight` (up to `i32::MAX`) would make `generate_smooth_sequence`
/// allocate O(sum of weights) memory, an OOM/DoS vector.
///
/// 1. Reduce by the greatest common divisor — lossless, collapses e.g. `[N, N]`
///    to `[1, 1]` and `[100, 200]` to `[1, 2]`, so common-factor weights cost
///    almost nothing regardless of magnitude.
/// 2. If the total still exceeds `MAX_SMOOTH_SEQUENCE_LEN` (only reachable with
///    adversarial coprime large weights), scale every positive weight down
///    proportionally, keeping at least 1 so no backend drops out of the rotation.
///
/// Zero weights are left as zero (callers filter them out before selection).
fn normalize_weights(mut weights: Vec<usize>) -> Vec<usize> {
    let divisor = weights.iter().copied().filter(|&w| w > 0).reduce(gcd).unwrap_or(1).max(1);
    if divisor > 1 {
        for w in weights.iter_mut() {
            *w /= divisor;
        }
    }

    let total: usize = weights.iter().sum();
    if total > MAX_SMOOTH_SEQUENCE_LEN {
        for w in weights.iter_mut().filter(|w| **w > 0) {
            *w = ((*w as u128 * MAX_SMOOTH_SEQUENCE_LEN as u128) / total as u128).max(1) as usize;
        }
    }

    weights
}

/// Lock-free Smooth Weighted Round Robin selector
///
/// Pre-computes a smooth sequence at construction time, then uses atomic counter
/// for lock-free selection. Requests are evenly distributed across backends
/// according to their weights.
///
/// # Type Parameters
/// * `T` - The type of items to select from
///
/// # Example
/// ```ignore
/// let backends = vec!["server1", "server2", "server3"];
/// let weights = vec![3, 1, 2]; // server1 has weight 3, etc.
/// let selector = WeightedRoundRobin::new(backends, weights);
///
/// // Selection sequence will be smoothly distributed:
/// // server1, server3, server1, server2, server3, server1, ...
/// // Instead of: server1, server1, server1, server2, server3, server3
/// let selected = selector.select();
/// ```
pub struct WeightedRoundRobin<T> {
    /// Items to select from
    items: Box<[T]>,
    /// Pre-computed smooth sequence (indices into items)
    /// Uses u16 to save memory, supports up to 65535 items
    sequence: Box<[u16]>,
    /// Atomic counter for lock-free round-robin
    counter: AtomicUsize,
}

impl<T> WeightedRoundRobin<T> {
    /// Create a new WeightedRoundRobin selector from items and their weights.
    ///
    /// # Arguments
    /// * `items` - A vector of items to select from
    /// * `weights` - A vector of weights for each item. Must have same length as items.
    ///
    /// # Panics
    /// Panics if items is empty, weights length doesn't match items, or total weight is 0.
    pub fn new(items: Vec<T>, weights: Vec<usize>) -> Self {
        assert!(!items.is_empty(), "items must not be empty");
        assert_eq!(items.len(), weights.len(), "items and weights must have same length");
        assert!(items.len() <= u16::MAX as usize, "supports up to 65535 items");

        let total_weight: usize = weights.iter().sum();
        assert!(total_weight > 0, "total weight must be greater than 0");

        // Bound the precomputed sequence before expansion so a large (or
        // unclamped attacker-controlled) weight cannot blow up the allocation.
        let weights = normalize_weights(weights);

        // Pre-compute smooth sequence using Smooth WRR algorithm
        let sequence = Self::generate_smooth_sequence(&weights);

        Self {
            items: items.into_boxed_slice(),
            sequence: sequence.into_boxed_slice(),
            counter: AtomicUsize::new(0),
        }
    }

    /// Generate smooth sequence using Smooth Weighted Round Robin algorithm.
    ///
    /// For weights [3, 1, 2], generates sequence like [0, 2, 0, 1, 2, 0]
    /// instead of simple [0, 0, 0, 1, 2, 2].
    fn generate_smooth_sequence(weights: &[usize]) -> Vec<u16> {
        let total: usize = weights.iter().sum();
        let total_weight = total as i64;

        // Current weights for smooth selection
        let mut current_weights: Vec<i64> = vec![0; weights.len()];
        let effective_weights: Vec<i64> = weights.iter().map(|&w| w as i64).collect();

        let mut sequence = Vec::with_capacity(total);

        for _ in 0..total {
            // Step 1: Add effective_weight to current_weight for all items
            for (i, cw) in current_weights.iter_mut().enumerate() {
                *cw += effective_weights[i];
            }

            // Step 2: Find the item with the highest current_weight
            // When equal, prefer the first one (lower index)
            let mut max_idx = 0;
            let mut max_weight = current_weights[0];
            for (i, &cw) in current_weights.iter().enumerate().skip(1) {
                if cw > max_weight {
                    max_weight = cw;
                    max_idx = i;
                }
            }

            sequence.push(max_idx as u16);

            // Step 3: Subtract total_weight from the selected item's current_weight
            current_weights[max_idx] -= total_weight;
        }

        sequence
    }

    /// Create a new WeightedRoundRobin selector with default weight of 1 for all items.
    ///
    /// # Arguments
    /// * `items` - A vector of items to select from
    pub fn with_equal_weights(items: Vec<T>) -> Self {
        let len = items.len();
        let weights = vec![1; len];
        Self::new(items, weights)
    }

    /// Select the next item based on smooth weighted round-robin.
    /// This is a lock-free O(1) operation.
    ///
    /// # Returns
    /// A reference to the selected item.
    #[inline]
    pub fn select(&self) -> &T {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        let seq_idx = idx % self.sequence.len();
        let item_idx = self.sequence[seq_idx] as usize;
        &self.items[item_idx]
    }

    /// Select the next item index based on smooth weighted round-robin.
    /// This is a lock-free O(1) operation.
    ///
    /// # Returns
    /// The index of the selected item (0-based).
    #[inline]
    pub fn select_index(&self) -> usize {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        let seq_idx = idx % self.sequence.len();
        self.sequence[seq_idx] as usize
    }

    /// Get the number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if the selector is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Get all items.
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Get the pre-computed sequence length (equals the sum of the normalized weights).
    pub fn sequence_len(&self) -> usize {
        self.sequence.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcd_basic() {
        assert_eq!(gcd(12, 8), 4);
        assert_eq!(gcd(7, 0), 7);
        assert_eq!(gcd(0, 5), 5);
        assert_eq!(gcd(17, 13), 1); // coprime
    }

    #[test]
    fn normalize_reduces_by_gcd_losslessly() {
        // Common factor collapses to lowest terms; the ratio is preserved exactly.
        assert_eq!(normalize_weights(vec![2, 4, 6]), vec![1, 2, 3]);
        assert_eq!(normalize_weights(vec![100, 200]), vec![1, 2]);
    }

    #[test]
    fn normalize_small_weights_unchanged() {
        assert_eq!(normalize_weights(vec![3, 1, 2]), vec![3, 1, 2]);
        assert_eq!(normalize_weights(vec![1, 1]), vec![1, 1]);
    }

    #[test]
    fn normalize_preserves_zero_weights() {
        // Zeros stay in place (callers filter them); GCD ignores them.
        assert_eq!(normalize_weights(vec![0, 2, 4]), vec![0, 1, 2]);
    }

    #[test]
    fn normalize_equal_huge_weights_collapse_to_ones() {
        // The OOM attack input: two equal near-i32::MAX weights are a 1:1 split.
        // GCD reduction alone bounds it to a 2-entry sequence (no scaling needed).
        let w = normalize_weights(vec![i32::MAX as usize, i32::MAX as usize]);
        assert_eq!(w, vec![1, 1]);
    }

    #[test]
    fn normalize_coprime_huge_weights_scale_within_budget() {
        // Coprime large weights cannot be GCD-reduced, so proportional scaling
        // caps the total at the budget while keeping every backend present.
        let a = 1_000_000_007usize; // prime
        let b = 1_000_000_009usize; // prime, coprime with a
        let w = normalize_weights(vec![a, b]);
        let total: usize = w.iter().sum();
        assert!(total <= MAX_SMOOTH_SEQUENCE_LEN + 2, "total {total} must be bounded");
        assert!(w.iter().all(|&x| x >= 1), "no backend may drop out: {w:?}");
        // Near-equal split: both land close to half the budget.
        assert!(w[0] >= 4000 && w[1] >= 4000, "near-equal split preserved: {w:?}");
    }

    #[test]
    fn new_bounds_sequence_for_coprime_huge_weights() {
        // End-to-end: constructing a WRR with adversarial coprime weights must
        // not allocate an O(sum-of-weights) sequence.
        let a = 1_000_000_007usize;
        let b = 1_000_000_009usize;
        let wrr = WeightedRoundRobin::new(vec!["a", "b"], vec![a, b]);
        assert!(
            wrr.sequence_len() <= MAX_SMOOTH_SEQUENCE_LEN + 2,
            "sequence must stay bounded, got {}",
            wrr.sequence_len()
        );
    }

    #[test]
    fn new_huge_equal_weights_still_distribute_evenly() {
        // After normalization the ratio is intact: a huge 1:1 split is still 1:1.
        let wrr = WeightedRoundRobin::new(vec!["a", "b"], vec![i32::MAX as usize, i32::MAX as usize]);
        let mut a = 0;
        let mut b = 0;
        for _ in 0..10 {
            match *wrr.select() {
                "a" => a += 1,
                "b" => b += 1,
                _ => unreachable!(),
            }
        }
        assert_eq!(a, 5);
        assert_eq!(b, 5);
    }
}
