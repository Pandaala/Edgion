use super::*;

fn summarize(lengths: &[(usize, usize)], entry_bytes: usize) -> JsonMatchSummary {
    let mut summary = JsonMatchSummary::new(entry_bytes);
    for &(name, value) in lengths {
        summary.append(name, value).unwrap();
    }
    summary
}

// Model actual retained records and a replaced singular match independently of
// the summary's aggregate/composition formula. Small inputs cannot overflow.
fn replay(lengths: &[(usize, usize)], entry_bytes: usize, base: usize) -> JsonMatchRetention {
    let mut records = Vec::new();
    let mut final_bytes = base;
    let mut peak_bytes = base;
    for &(name, value) in lengths {
        records.push((name, value));
        final_bytes = base;
        for &(stored_name, stored_value) in &records {
            final_bytes += entry_bytes + stored_name + stored_value;
        }
        final_bytes += name + value;
        peak_bytes = peak_bytes.max(final_bytes);
    }
    JsonMatchRetention {
        peak_bytes,
        final_bytes,
    }
}

#[test]
fn exhaustive_ordered_partitions_match_retained_record_replay() {
    const VALUES: [(usize, usize); 4] = [(0, 0), (1, 0), (0, 3), (2, 7)];
    for length in 0..=6u32 {
        for mut encoding in 0..VALUES.len().pow(length) {
            let mut values = Vec::new();
            for _ in 0..length {
                values.push(VALUES[encoding % VALUES.len()]);
                encoding /= VALUES.len();
            }
            for entry_bytes in [0, 1, 8] {
                let expected = summarize(&values, entry_bytes);
                assert_eq!(expected.count(), values.len());
                assert_eq!(
                    expected.total_lengths(),
                    values.iter().map(|(n, v)| n + v).sum::<usize>()
                );
                assert_eq!(expected.last_length(), values.last().map_or(0, |(n, v)| n + v));
                for base in [0, 17] {
                    assert_eq!(expected.retention(base).unwrap(), replay(&values, entry_bytes, base));
                }
                for first in 0..=values.len() {
                    for second in first..=values.len() {
                        let a = summarize(&values[..first], entry_bytes);
                        let b = summarize(&values[first..second], entry_bytes);
                        let c = summarize(&values[second..], entry_bytes);
                        let mut left = a;
                        left.concat(b).unwrap();
                        left.concat(c).unwrap();
                        let mut right_tail = b;
                        right_tail.concat(c).unwrap();
                        let mut right = a;
                        right.concat(right_tail).unwrap();
                        assert_eq!(left, expected);
                        assert_eq!(right, expected);
                    }
                }
            }
        }
    }
}

#[test]
fn earlier_large_match_controls_peak_and_order_matters() {
    let forward = summarize(&[(4, 100), (1, 0)], 8);
    let reverse = summarize(&[(1, 0), (4, 100)], 8);
    assert_eq!(
        forward.retention(5).unwrap(),
        JsonMatchRetention {
            peak_bytes: 221,
            final_bytes: 127
        }
    );
    assert_eq!(
        reverse.retention(5).unwrap(),
        JsonMatchRetention {
            peak_bytes: 230,
            final_bytes: 230
        }
    );
    assert_eq!(forward.count(), reverse.count());
    assert_eq!(forward.total_lengths(), reverse.total_lengths());
    assert_ne!(forward, reverse);
}

#[test]
fn empty_identity_and_zero_length_matches_are_distinct() {
    for entry_bytes in [0, 1, usize::MAX] {
        let empty = JsonMatchSummary::new(entry_bytes);
        let mut joined = empty;
        joined.concat(empty).unwrap();
        assert_eq!(joined, empty);
        assert_eq!(
            empty.retention(usize::MAX).unwrap(),
            JsonMatchRetention {
                peak_bytes: usize::MAX,
                final_bytes: usize::MAX,
            }
        );
        let mut matched = empty;
        matched.append(0, 0).unwrap();
        assert_eq!(matched.count(), 1);
        assert_eq!(
            matched.retention(0).unwrap(),
            JsonMatchRetention {
                peak_bytes: entry_bytes,
                final_bytes: entry_bytes,
            }
        );
        let mut joined = empty;
        joined.concat(matched).unwrap();
        assert_eq!(joined, matched);
        joined.concat(empty).unwrap();
        assert_eq!(joined, matched);
    }
}

#[test]
fn descriptor_mismatch_is_atomic_even_for_empty_operands() {
    for values in [vec![], vec![(1, 2)]] {
        let mut summary = summarize(&values, 8);
        let original = summary;
        for later in [JsonMatchSummary::new(9), summarize(&[(3, 4)], 9)] {
            assert_eq!(summary.concat(later), Err(JsonMatchSummaryError::EntryBytesMismatch));
            assert_eq!(summary, original);
        }
    }
}

#[test]
fn append_and_concat_overflow_preserve_operands() {
    // Name/value addition, doubled last length, and descriptor multiplication.
    for (mut summary, name, value) in [
        (JsonMatchSummary::new(0), usize::MAX, 1),
        (JsonMatchSummary::new(0), 0, usize::MAX),
        (summarize(&[(0, 0)], usize::MAX), 0, 0),
        (summarize(&[(0, usize::MAX / 2)], 0), 2, usize::MAX / 2),
    ] {
        let before = summary;
        assert_eq!(summary.append(name, value), Err(JsonMatchSummaryError::Overflow));
        assert_eq!(summary, before);
    }

    // Build usize::MAX zero-length matches without iterating over them.
    let mut count_only = JsonMatchSummary::new(0);
    for _ in 0..usize::BITS {
        count_only.concat(count_only).unwrap();
        count_only.append(0, 0).unwrap();
    }
    assert_eq!(count_only.count(), usize::MAX);
    let before = count_only;
    assert_eq!(count_only.append(0, 0), Err(JsonMatchSummaryError::Overflow));
    assert_eq!(count_only, before);
    assert_eq!(
        count_only.concat(summarize(&[(0, 0)], 0)),
        Err(JsonMatchSummaryError::Overflow)
    );
    assert_eq!(count_only, before);

    for (mut first, second) in [
        (summarize(&[(0, 0)], usize::MAX), summarize(&[(0, 0)], usize::MAX)),
        (
            summarize(&[(0, usize::MAX / 2)], 0),
            summarize(&[(0, usize::MAX / 2)], 0),
        ),
    ] {
        let before = first;
        let later_before = second;
        assert_eq!(first.concat(second), Err(JsonMatchSummaryError::Overflow));
        assert_eq!(first, before);
        assert_eq!(second, later_before);
    }
}

#[test]
fn exact_maximum_and_base_addition_are_checked() {
    let summary = summarize(&[(0, usize::MAX / 2)], 1);
    assert_eq!(
        summary.retention(0).unwrap(),
        JsonMatchRetention {
            peak_bytes: usize::MAX,
            final_bytes: usize::MAX,
        }
    );
    assert_eq!(summary.retention(1), Err(JsonMatchSummaryError::Overflow));
    // Final charge fits with the base, but an earlier peak does not.
    let mut summary = summarize(&[(0, usize::MAX / 2)], 0);
    summary.append(0, 0).unwrap();
    assert_eq!(summary.retention(1).unwrap().peak_bytes, usize::MAX);
    assert_eq!(summary.retention(2), Err(JsonMatchSummaryError::Overflow));
}
