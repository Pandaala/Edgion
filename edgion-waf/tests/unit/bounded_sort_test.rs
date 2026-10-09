use super::*;

#[test]
fn sorts_preserve_duplicate_items_and_stop_at_first_comparison_error() {
    for input in [vec![], vec![3], vec![4, 1, 4, 2, 0, 1], (0..257).rev().collect()] {
        let mut expected = input.clone();
        expected.sort_unstable();
        let mut actual = input.clone();
        heapsort_by(&mut actual, |a, b| Ok::<_, ()>(a.cmp(&b))).unwrap();
        assert_eq!(actual, expected);
    }
    for allowed in 0..20 {
        let mut values = [9, 4, 1, 6, 0, 7, 5, 3, 2, 8];
        let mut calls = 0;
        let result = heapsort_by(&mut values, |a, b| {
            calls += 1;
            if calls > allowed {
                Err("comparison budget")
            } else {
                Ok(a.cmp(&b))
            }
        });
        assert_eq!(result, Err("comparison budget"));
        assert_eq!(calls, allowed + 1);
        values.sort_unstable();
        assert_eq!(values, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }
}
