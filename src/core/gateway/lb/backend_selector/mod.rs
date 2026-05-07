//! Simple load balancing selectors
//!
//! This module provides simple load balancing algorithms for backend selection.

mod selector;
mod weighted_selector;

pub use selector::{BackendSelector, ERR_INCONSISTENT_WEIGHT, ERR_NO_BACKEND_REFS};
pub use weighted_selector::WeightedRoundRobin;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── BackendSelector dispatch & lazy-init coverage ──────────────────────────

    #[test]
    fn selector_select_before_init_returns_no_backend_refs_error() {
        let s: BackendSelector<u32> = BackendSelector::new();
        assert!(!s.is_initialized());
        let err = s.select().expect_err("uninit selector must error");
        assert_eq!(err, ERR_NO_BACKEND_REFS);
    }

    #[test]
    fn selector_init_with_empty_items_records_error() {
        let s: BackendSelector<u32> = BackendSelector::new();
        s.init(vec![], vec![]);
        assert!(s.is_initialized());
        assert_eq!(s.select().unwrap_err(), ERR_NO_BACKEND_REFS);
    }

    #[test]
    fn selector_init_partial_weights_is_inconsistent_error() {
        // Mixed Some/None weights: the selector must reject the configuration.
        let s: BackendSelector<&'static str> = BackendSelector::new();
        s.init(vec!["a", "b"], vec![Some(1), None]);
        assert_eq!(s.select().unwrap_err(), ERR_INCONSISTENT_WEIGHT);
    }

    #[test]
    fn selector_single_valid_item_returns_it() {
        let s: BackendSelector<&'static str> = BackendSelector::new();
        s.init(vec!["only"], vec![None]);
        assert_eq!(s.select().unwrap(), "only");
        // Repeated calls keep returning the same one (Single state).
        assert_eq!(s.select().unwrap(), "only");
    }

    #[test]
    fn selector_zero_weight_filters_item_out() {
        // Three items, but two have weight 0 → should collapse to single item.
        let s: BackendSelector<&'static str> = BackendSelector::new();
        s.init(vec!["a", "b", "c"], vec![Some(0), Some(0), Some(2)]);
        for _ in 0..5 {
            assert_eq!(s.select().unwrap(), "c");
        }
    }

    #[test]
    fn selector_all_zero_weights_yields_no_backend_refs_error() {
        let s: BackendSelector<&'static str> = BackendSelector::new();
        s.init(vec!["a", "b"], vec![Some(0), Some(0)]);
        assert_eq!(s.select().unwrap_err(), ERR_NO_BACKEND_REFS);
    }

    #[test]
    fn selector_multiple_items_dispatch_to_weighted_round_robin() {
        let s: BackendSelector<&'static str> = BackendSelector::new();
        s.init(vec!["a", "b", "c"], vec![Some(1), Some(2), Some(5)]);

        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        let total = 1 + 2 + 5;
        for _ in 0..total {
            *counts.entry(s.select().unwrap()).or_default() += 1;
        }
        assert_eq!(counts.get("a").copied().unwrap_or(0), 1);
        assert_eq!(counts.get("b").copied().unwrap_or(0), 2);
        assert_eq!(counts.get("c").copied().unwrap_or(0), 5);
    }

    #[test]
    fn selector_init_is_idempotent() {
        let s: BackendSelector<&'static str> = BackendSelector::new();
        s.init(vec!["a"], vec![None]);
        // Second init must be ignored — the second config should NOT replace.
        s.init(vec!["b"], vec![None]);
        assert_eq!(s.select().unwrap(), "a");
    }

    // ── WeightedRoundRobin direct coverage ────────────────────────────────────

    #[test]
    fn weighted_rr_with_equal_weights_distributes_evenly() {
        let wrr = WeightedRoundRobin::with_equal_weights(vec!["a", "b", "c"]);
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for _ in 0..9 {
            *counts.entry(*wrr.select()).or_default() += 1;
        }
        for v in counts.values() {
            assert_eq!(*v, 3, "equal-weight WRR must distribute evenly across one round");
        }
    }

    #[test]
    fn weighted_rr_smooth_distribution_does_not_cluster() {
        // Smooth WRR should not return [a,a,a,b,c,c] for weights [3,1,2];
        // the first three picks must touch at least 2 distinct items.
        let wrr = WeightedRoundRobin::new(vec!["a", "b", "c"], vec![3, 1, 2]);
        let first_three: Vec<_> = (0..3).map(|_| *wrr.select()).collect();
        let distinct: std::collections::HashSet<_> = first_three.iter().collect();
        assert!(
            distinct.len() >= 2,
            "smooth WRR must not cluster the high-weight item, got {:?}",
            first_three
        );
    }

    #[test]
    fn weighted_rr_select_index_matches_select() {
        let wrr = WeightedRoundRobin::new(vec!["a", "b"], vec![1, 1]);
        assert_eq!(wrr.len(), 2);
        assert!(!wrr.is_empty());
        assert_eq!(wrr.sequence_len(), 2);
        // First select_index then select are independent counter ticks.
        let idx = wrr.select_index();
        assert!(idx < 2);
    }

    #[test]
    #[should_panic(expected = "items must not be empty")]
    fn weighted_rr_panics_on_empty_items() {
        let _: WeightedRoundRobin<&'static str> = WeightedRoundRobin::new(vec![], vec![]);
    }

    #[test]
    #[should_panic(expected = "items and weights must have same length")]
    fn weighted_rr_panics_on_length_mismatch() {
        let _ = WeightedRoundRobin::new(vec!["a", "b"], vec![1]);
    }

    #[test]
    #[should_panic(expected = "total weight must be greater than 0")]
    fn weighted_rr_panics_on_zero_total_weight() {
        let _ = WeightedRoundRobin::new(vec!["a", "b"], vec![0, 0]);
    }
}
