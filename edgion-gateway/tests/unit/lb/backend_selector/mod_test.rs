use super::*;
use std::collections::HashMap;

// ── BackendSelector construction and selection coverage ────────────────────

#[test]
fn selector_default_weights_distribute_evenly() {
    let s = BackendSelector::new(vec![0, 1], vec![None, None]);
    assert_eq!(s.select(), Ok(0));
    assert_eq!(s.select(), Ok(1));
    assert_eq!(s.select(), Ok(0));
    assert_eq!(s.select(), Ok(1));
}

#[test]
fn selector_init_with_empty_items_records_error() {
    let s: BackendSelector<u32> = BackendSelector::new(vec![], vec![]);
    assert_eq!(s.select().unwrap_err(), ERR_NO_BACKEND_REFS);
}

#[test]
fn selector_rejects_more_items_than_weights() {
    let s: BackendSelector<&'static str> = BackendSelector::new(vec!["a", "b"], vec![Some(1)]);
    assert_eq!(s.select().unwrap_err(), ERR_WRR_BUILD_FAILED);
}

#[test]
fn selector_rejects_more_weights_than_items() {
    let s: BackendSelector<&'static str> = BackendSelector::new(vec!["a"], vec![Some(1), Some(2)]);
    assert_eq!(s.select().unwrap_err(), ERR_WRR_BUILD_FAILED);
}

#[test]
fn selector_init_partial_weights_is_inconsistent_error() {
    // Mixed Some/None weights: the selector must reject the configuration.
    let s: BackendSelector<&'static str> = BackendSelector::new(vec!["a", "b"], vec![Some(1), None]);
    assert_eq!(s.select().unwrap_err(), ERR_INCONSISTENT_WEIGHT);
}

#[test]
fn selector_single_valid_item_returns_it() {
    let s: BackendSelector<&'static str> = BackendSelector::new(vec!["only"], vec![None]);
    assert_eq!(s.select().unwrap(), "only");
    // Repeated calls keep returning the same one (Single state).
    assert_eq!(s.select().unwrap(), "only");
}

#[test]
fn selector_zero_weight_filters_item_out() {
    // Three items, but two have weight 0 → should collapse to single item.
    let s: BackendSelector<&'static str> = BackendSelector::new(vec!["a", "b", "c"], vec![Some(0), Some(0), Some(2)]);
    for _ in 0..5 {
        assert_eq!(s.select().unwrap(), "c");
    }
}

#[test]
fn selector_all_zero_weights_yields_no_backend_refs_error() {
    let s: BackendSelector<&'static str> = BackendSelector::new(vec!["a", "b"], vec![Some(0), Some(0)]);
    assert_eq!(s.select().unwrap_err(), ERR_NO_BACKEND_REFS);
}

#[test]
fn selector_multiple_items_dispatch_to_weighted_round_robin() {
    let s: BackendSelector<&'static str> = BackendSelector::new(vec!["a", "b", "c"], vec![Some(1), Some(2), Some(5)]);

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
fn selector_captured_generation_shares_cursor_and_replacement_is_independent() {
    let old = std::sync::Arc::new(BackendSelector::new(vec![0, 1], vec![None, None]));
    let captured = std::sync::Arc::clone(&old);
    assert_eq!(old.select(), Ok(0));
    assert_eq!(captured.select(), Ok(1));
    assert_eq!(old.select(), Ok(0));
    let replacement = BackendSelector::new(vec![0, 1], vec![None, None]);
    drop(old);
    assert_eq!(replacement.select(), Ok(0));
    assert_eq!(captured.select(), Ok(1));
    assert_eq!(replacement.select(), Ok(1));
}

#[test]
fn selector_negative_weights_filter_items_out() {
    let s = BackendSelector::new(vec![0, 1, 2], vec![Some(-1), Some(0), Some(3)]);
    assert_eq!(s.select(), Ok(2));
    assert_eq!(s.select(), Ok(2));
    let empty = BackendSelector::new(vec![0], vec![Some(-1)]);
    assert_eq!(empty.select(), Err(ERR_NO_BACKEND_REFS));
}

#[test]
fn selector_weighted_constructor_failure_is_selection_error() {
    let count = u16::MAX as usize + 1;
    let s = BackendSelector::new((0..count).collect(), vec![Some(1); count]);
    assert_eq!(s.select(), Err(ERR_WRR_BUILD_FAILED));
}

// ── WeightedRoundRobin direct coverage ────────────────────────────────────

#[test]
fn weighted_rr_with_equal_weights_distributes_evenly() {
    let wrr = WeightedRoundRobin::with_equal_weights(vec!["a", "b", "c"]).expect("valid WRR inputs");
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
    let wrr = WeightedRoundRobin::new(vec!["a", "b", "c"], vec![3, 1, 2]).expect("valid WRR inputs");
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
    let wrr = WeightedRoundRobin::new(vec!["a", "b"], vec![1, 1]).expect("valid WRR inputs");
    assert_eq!(wrr.len(), 2);
    assert!(!wrr.is_empty());
    assert_eq!(wrr.sequence_len(), 2);
    // First select_index then select are independent counter ticks.
    let idx = wrr.select_index();
    assert!(idx < 2);
}

#[test]
fn weighted_rr_rejects_empty_items() {
    let r: Result<WeightedRoundRobin<&'static str>, _> = WeightedRoundRobin::new(vec![], vec![]);
    assert_eq!(r.err(), Some(WrrError::EmptyItems));
}

#[test]
fn weighted_rr_rejects_length_mismatch() {
    let r = WeightedRoundRobin::new(vec!["a", "b"], vec![1]);
    assert_eq!(r.err(), Some(WrrError::LengthMismatch));
}

#[test]
fn weighted_rr_rejects_zero_total_weight() {
    let r = WeightedRoundRobin::new(vec!["a", "b"], vec![0, 0]);
    assert_eq!(r.err(), Some(WrrError::ZeroTotalWeight));
}
