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
    let wrr = WeightedRoundRobin::new(vec!["a", "b"], vec![a, b]).expect("valid WRR inputs");
    assert!(
        wrr.sequence_len() <= MAX_SMOOTH_SEQUENCE_LEN + 2,
        "sequence must stay bounded, got {}",
        wrr.sequence_len()
    );
}

#[test]
fn new_handles_near_usize_max_weights_without_overflow() {
    let wrr = WeightedRoundRobin::new(vec!["a", "b"], vec![usize::MAX, usize::MAX - 1])
        .expect("near-maximum weights must normalize safely");
    assert!(
        wrr.sequence_len() <= MAX_SMOOTH_SEQUENCE_LEN + 2,
        "sequence must stay bounded, got {}",
        wrr.sequence_len()
    );

    let mut a = 0usize;
    let mut b = 0usize;
    for _ in 0..wrr.sequence_len() {
        match *wrr.select() {
            "a" => a += 1,
            "b" => b += 1,
            _ => unreachable!(),
        }
    }
    assert!(a > 0 && b > 0, "both backends must remain represented");
    assert!(
        a.abs_diff(b) <= 1,
        "near-equal weights must remain distributed: {a}:{b}"
    );
}

#[test]
fn new_huge_equal_weights_still_distribute_evenly() {
    // After normalization the ratio is intact: a huge 1:1 split is still 1:1.
    let wrr =
        WeightedRoundRobin::new(vec!["a", "b"], vec![i32::MAX as usize, i32::MAX as usize]).expect("valid WRR inputs");
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

#[test]
fn normalized_unit_weights_preserve_exact_order_across_cycles() {
    for count in [1, 2, 3, 17, 128] {
        for weight in [1, 7, usize::MAX] {
            let wrr = WeightedRoundRobin::new((0..count).collect(), vec![weight; count]).unwrap();
            assert_eq!(wrr.sequence_len(), count);
            for expected in (0..count).cycle().take(count * 3) {
                assert_eq!(*wrr.select(), expected);
            }
        }
    }
    for (weights, expected) in [
        (vec![3, 1, 2], vec![0, 2, 0, 1, 2, 0]),
        (vec![1, 1, 2], vec![2, 0, 1, 2]),
        (vec![0, 2, 0, 2], vec![1, 3]),
    ] {
        let wrr = WeightedRoundRobin::new((0..weights.len()).collect(), weights).unwrap();
        assert_eq!(wrr.sequence.as_ref(), expected.as_slice());
        for expected in expected.iter().cycle().take(expected.len() * 3) {
            assert_eq!(*wrr.select(), usize::from(*expected));
        }
    }
}

#[test]
fn unit_weight_sequence_accepts_maximum_item_count() {
    let count = u16::MAX as usize;
    let wrr = WeightedRoundRobin::with_equal_weights((0..count).collect()).unwrap();
    assert_eq!(wrr.sequence_len(), count);
    for expected in 0..count {
        assert_eq!(wrr.select_index(), expected);
    }
    assert_eq!(wrr.select_index(), 0);
}
