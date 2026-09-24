use super::{checked_join_output_len, RuntimeError};

#[test]
fn computes_empty_single_and_multi_item_lengths() {
    assert_eq!(checked_join_output_len(std::iter::empty(), 8, 0).unwrap(), 0);
    assert_eq!(checked_join_output_len([3].into_iter(), 8, 3).unwrap(), 3);
    assert_eq!(checked_join_output_len([1, 2, 3].into_iter(), 4, 14).unwrap(), 14);
}

#[test]
fn reports_exact_representable_length_above_limit() {
    assert!(matches!(
        checked_join_output_len([1, 1, 1].into_iter(), 4, 10),
        Err(RuntimeError::StringTooLong { len: 11, limit: 10 })
    ));
}

#[test]
fn reports_usize_max_for_arithmetic_overflow() {
    for result in [
        checked_join_output_len([0, 0, 0].into_iter(), usize::MAX, 10),
        checked_join_output_len([usize::MAX, 1].into_iter(), 0, 10),
    ] {
        assert!(matches!(
            result,
            Err(RuntimeError::StringTooLong {
                len: usize::MAX,
                limit: 10
            })
        ));
    }
}
