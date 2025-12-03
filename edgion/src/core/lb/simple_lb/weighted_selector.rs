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
        assert!(
            items.len() <= u16::MAX as usize,
            "supports up to 65535 items"
        );

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_equal_weights() {
        let items = vec!["a", "b", "c"];
        let selector = WeightedRoundRobin::with_equal_weights(items);

        // With equal weights, should cycle through a, b, c, a, b, c, ...
        assert_eq!(*selector.select(), "a");
        assert_eq!(*selector.select(), "b");
        assert_eq!(*selector.select(), "c");
        assert_eq!(*selector.select(), "a");
        assert_eq!(*selector.select(), "b");
        assert_eq!(*selector.select(), "c");
    }

    #[test]
    fn test_smooth_distribution() {
        // weights: [5, 1, 1] -> total 7
        // Smooth WRR should distribute more evenly than simple WRR
        let items = vec!["A", "B", "C"];
        let weights = vec![5, 1, 1];
        let selector = WeightedRoundRobin::new(items, weights);

        // Collect 7 selections (one full cycle)
        let mut sequence = Vec::new();
        for _ in 0..7 {
            sequence.push(*selector.select());
        }

        // Count occurrences
        let count_a = sequence.iter().filter(|&&x| x == "A").count();
        let count_b = sequence.iter().filter(|&&x| x == "B").count();
        let count_c = sequence.iter().filter(|&&x| x == "C").count();

        // Should match weights: A=5, B=1, C=1
        assert_eq!(count_a, 5, "A should be selected 5 times");
        assert_eq!(count_b, 1, "B should be selected 1 time");
        assert_eq!(count_c, 1, "C should be selected 1 time");

        // Verify smooth distribution: A should NOT be the first 5 consecutive selections
        let first_five = &sequence[0..5];
        let non_a_in_first_five = first_five.iter().filter(|&&x| x != "A").count();
        assert!(non_a_in_first_five > 0, "Smooth WRR should not have 5 consecutive A's");
    }

    #[test]
    fn test_weighted_distribution_large() {
        // Test over multiple cycles
        let items = vec!["server1", "server2", "server3"];
        let weights = vec![3, 1, 2];
        let selector = WeightedRoundRobin::new(items, weights);

        let mut counts = [0usize; 3];
        for _ in 0..600 {
            let selected = selector.select();
            match *selected {
                "server1" => counts[0] += 1,
                "server2" => counts[1] += 1,
                "server3" => counts[2] += 1,
                _ => panic!("unexpected item"),
            }
        }

        // Over 600 selections (100 full cycles of weight 6), expect:
        // server1: 300, server2: 100, server3: 200
        assert_eq!(counts[0], 300);
        assert_eq!(counts[1], 100);
        assert_eq!(counts[2], 200);
    }

    #[test]
    fn test_single_item() {
        let items = vec!["only"];
        let selector = WeightedRoundRobin::new(items, vec![5]);

        for _ in 0..10 {
            assert_eq!(*selector.select(), "only");
        }
    }

    #[test]
    fn test_select_index() {
        let items = vec![100, 200, 300];
        let selector = WeightedRoundRobin::with_equal_weights(items);

        assert_eq!(selector.select_index(), 0);
        assert_eq!(selector.select_index(), 1);
        assert_eq!(selector.select_index(), 2);
        assert_eq!(selector.select_index(), 0);
    }

    #[test]
    fn test_sequence_is_smooth() {
        // Test that sequence for [3, 1, 2] is smooth (interleaved)
        let items = vec!["A", "B", "C"];
        let weights = vec![3, 1, 2];
        let selector = WeightedRoundRobin::new(items, weights);

        // Collect one full cycle
        let mut sequence = Vec::new();
        for _ in 0..6 {
            sequence.push(*selector.select());
        }

        // Check no more than 2 consecutive same items
        for i in 0..sequence.len() - 2 {
            let same_count = if sequence[i] == sequence[i + 1] && sequence[i + 1] == sequence[i + 2] {
                3
            } else {
                0
            };
            assert!(same_count < 3, "Should not have 3 consecutive same items: {:?}", sequence);
        }
    }

    #[test]
    #[should_panic(expected = "items must not be empty")]
    fn test_empty_items() {
        WeightedRoundRobin::<i32>::new(vec![], vec![]);
    }

    #[test]
    #[should_panic(expected = "items and weights must have same length")]
    fn test_mismatched_lengths() {
        WeightedRoundRobin::new(vec!["a", "b"], vec![1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "total weight must be greater than 0")]
    fn test_zero_weights() {
        WeightedRoundRobin::new(vec!["a", "b", "c"], vec![0, 0, 0]);
    }

    #[test]
    fn test_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let items = vec!["a", "b", "c"];
        let selector = Arc::new(WeightedRoundRobin::with_equal_weights(items));

        let mut handles = vec![];

        // Spawn 10 threads, each selecting 100 times
        for _ in 0..10 {
            let selector_clone = Arc::clone(&selector);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    let _ = selector_clone.select();
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // If we got here without panicking, the implementation is thread-safe
    }

    #[test]
    fn test_sequence_len() {
        let items = vec!["a", "b", "c"];
        let weights = vec![3, 1, 2];
        let selector = WeightedRoundRobin::new(items, weights);

        // Sequence length should equal total weight
        assert_eq!(selector.sequence_len(), 6);
    }
}
