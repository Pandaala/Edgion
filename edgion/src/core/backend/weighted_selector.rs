//! Smooth Weighted Round Robin selector
//!
//! A thread-safe smooth weighted round-robin algorithm for backend selection.
//! This algorithm distributes requests more evenly compared to simple weighted round-robin.

use parking_lot::Mutex;

/// Smooth Weighted Round Robin selector
///
/// Uses Nginx's smooth weighted round-robin algorithm to distribute requests
/// evenly based on weights. Unlike simple weighted round-robin, this algorithm
/// avoids "burst" behavior where a high-weight backend receives many consecutive requests.
///
/// # Type Parameters
/// * `T` - The type of items to select from
///
/// # Algorithm
/// For each selection:
/// 1. Add `effective_weight` to `current_weight` for all items
/// 2. Select the item with the highest `current_weight`
/// 3. Subtract `total_weight` from the selected item's `current_weight`
///
/// # Example
/// ```ignore
/// let backends = vec!["server1", "server2", "server3"];
/// let weights = vec![5, 1, 1]; // server1 has weight 5, server2 has weight 1, etc.
/// let selector = WeightedRoundRobin::new(backends, weights);
///
/// // Selection sequence will be evenly distributed:
/// // server1, server1, server2, server1, server3, server1, server1, ...
/// // Instead of: server1, server1, server1, server1, server1, server2, server3
/// let selected = selector.select();
/// ```
pub struct WeightedRoundRobin<T> {
    /// Items to select from
    items: Vec<T>,
    /// Effective weights (configured weights)
    effective_weights: Vec<i64>,
    /// Total weight (sum of all weights)
    total_weight: i64,
    /// Current weights (mutable state, protected by mutex)
    current_weights: Mutex<Vec<i64>>,
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

        let effective_weights: Vec<i64> = weights.iter().map(|&w| w as i64).collect();
        let total_weight: i64 = effective_weights.iter().sum();

        assert!(total_weight > 0, "total weight must be greater than 0");

        let current_weights = vec![0i64; items.len()];

        Self {
            items,
            effective_weights,
            total_weight,
            current_weights: Mutex::new(current_weights),
        }
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
    ///
    /// # Returns
    /// A reference to the selected item.
    pub fn select(&self) -> &T {
        let index = self.select_index();
        &self.items[index]
    }

    /// Select the next item index based on smooth weighted round-robin.
    ///
    /// # Returns
    /// The index of the selected item (0-based).
    pub fn select_index(&self) -> usize {
        let mut current_weights = self.current_weights.lock();

        // Step 1: Add effective_weight to current_weight for all items
        for (i, cw) in current_weights.iter_mut().enumerate() {
            *cw += self.effective_weights[i];
        }

        // Step 2: Find the item with the highest current_weight
        let mut max_index = 0;
        let mut max_weight = current_weights[0];
        for (i, &cw) in current_weights.iter().enumerate().skip(1) {
            if cw > max_weight {
                max_weight = cw;
                max_index = i;
            }
        }

        // Step 3: Subtract total_weight from the selected item's current_weight
        current_weights[max_index] -= self.total_weight;

        max_index
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
        // In smooth WRR, B and C should appear before all 5 A's are used
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
}
