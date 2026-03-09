//! Lock-free Smooth Weighted Round Robin selector
//!
//! A thread-safe, lock-free smooth weighted round-robin algorithm for backend selection.
//! Uses pre-computed smooth sequence + atomic counter for O(1) selection.

use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// Get the pre-computed sequence length (equals total weight).
    pub fn sequence_len(&self) -> usize {
        self.sequence.len()
    }
}
