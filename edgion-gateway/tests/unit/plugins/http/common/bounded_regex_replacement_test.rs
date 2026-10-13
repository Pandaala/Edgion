use super::*;

#[test]
fn exact_dollar_classification_selects_the_iterator_branch() {
    let regex = Regex::new("(a)").expect("valid regex");
    assert!(!CompiledRegexReplacement::compile(&regex, "literal".to_string()).needs_captures());
    assert!(CompiledRegexReplacement::compile(&regex, "$1".to_string()).needs_captures());
    assert!(CompiledRegexReplacement::compile(&regex, "$!".to_string()).needs_captures());

    assert_eq!(
        replace_all_bounded(&regex, "aa", "x".to_string(), 2),
        Ok(Some("xx".to_string()))
    );
    assert_eq!(
        replace_all_bounded(&regex, "aa", "$1".to_string(), 2),
        Ok(Some("aa".to_string()))
    );
}

#[test]
fn compiled_replacement_matches_regex_interpolation_grammar() {
    let regex = Regex::new(r"(?P<name>a)(?P<π>c)?").expect("valid regex");
    let input = "ac a";
    let replacements = [
        "$0",
        "$1",
        "$10",
        "$name",
        "${name}",
        "$namesuffix",
        "${name}suffix",
        "$missing",
        "${missing}",
        "$2",
        "${+0}",
        "${+1}",
        "${01}",
        "${184467440737095516160}",
        "$$",
        "$$$1",
        "$",
        "$!",
        "$é",
        "${}",
        "${ }",
        "${π}",
        "${wat $name ok",
        "pre-$name-post",
    ];

    for replacement in replacements {
        let actual = replace_all_bounded(&regex, input, replacement.to_string(), usize::MAX)
            .expect("bounded expansion succeeds")
            .expect("input matches");
        let expected = regex.replace_all(input, replacement);
        assert_eq!(actual, expected.as_ref(), "replacement {replacement:?}");
    }
}

#[test]
fn zero_width_replacement_matches_regex() {
    for (pattern, input, replacement) in [(r"", "ab", "-"), (r"^|$", "é", "$0-")] {
        let regex = Regex::new(pattern).expect("valid regex");
        let actual = replace_all_bounded(&regex, input, replacement.to_string(), usize::MAX)
            .expect("bounded expansion succeeds")
            .expect("input matches");
        assert_eq!(actual, regex.replace_all(input, replacement));
    }
}

#[test]
fn no_match_returns_none_without_constructing_output() {
    let regex = Regex::new("needle").expect("valid regex");
    assert_eq!(
        replace_all_bounded(&regex, &"haystack".repeat(1024), "x".to_string(), 0),
        Ok(None)
    );
}

#[test]
fn output_limit_reports_the_first_crossing_prefix() {
    let regex = Regex::new("(a)").expect("valid regex");
    assert_eq!(
        replace_all_bounded(&regex, "aa", "$1$1".to_string(), 4),
        Ok(Some("aaaa".to_string()))
    );
    assert_eq!(
        replace_all_bounded(&regex, "aa", "$1$1".to_string(), 3),
        Err(BoundedRegexReplacementError::OutputTooLong { len: 4 })
    );
}

#[test]
fn repeated_capture_amplification_is_bounded_during_construction() {
    let regex = Regex::new("(a)").expect("valid regex");
    assert_eq!(
        replace_all_bounded(&regex, "aaaaaaaa", "$1$1$1$1".to_string(), 31),
        Err(BoundedRegexReplacementError::OutputTooLong { len: 32 })
    );
}

#[test]
fn checked_length_overflow_reports_maximum_length() {
    assert_eq!(
        checked_output_len(usize::MAX, 1, usize::MAX),
        Err(BoundedRegexReplacementError::OutputTooLong { len: usize::MAX })
    );
}

#[test]
fn output_growth_target_handles_large_appends_clamping_and_saturation() {
    assert_eq!(output_growth_target(0, 1, 1024), 1);
    assert_eq!(output_growth_target(8, 9, 1024), 16);
    assert_eq!(output_growth_target(8, 24, 1024), 24);
    assert_eq!(output_growth_target(8, 9, 12), 12);
    assert_eq!(output_growth_target(0, 0, 0), 0);
    let overflowing_capacity = usize::MAX / 2 + 1;
    assert_eq!(
        output_growth_target(overflowing_capacity, overflowing_capacity + 1, usize::MAX),
        usize::MAX
    );
    assert_eq!(
        output_growth_target(usize::MAX / 2 + 1, usize::MAX, usize::MAX),
        usize::MAX
    );
    assert_eq!(
        output_growth_target(usize::MAX / 2 + 1, usize::MAX - 1, usize::MAX - 1),
        usize::MAX - 1
    );
}

#[test]
fn small_appends_reuse_geometric_capacity() {
    let limit = 1024;
    let mut output = String::new();
    let mut growths = 0;
    for next in 1..=limit {
        let capacity = output.capacity();
        let pointer = output.as_ptr();
        append_bounded(&mut output, "x", limit).expect("prefix fits the limit");
        assert_eq!(output.len(), next);
        if next > capacity {
            growths += 1;
            // Allocators may grant more capacity than the requested target.
            assert!(output.capacity() >= next.max(capacity.saturating_mul(2)).min(limit));
        } else {
            assert_eq!(output.capacity(), capacity);
            assert_eq!(output.as_ptr(), pointer);
        }
    }
    assert!(growths <= 11, "1024 one-byte appends need at most 11 growths");
    assert_eq!(output, "x".repeat(limit));
}

#[test]
fn rejected_prefix_preserves_output_and_capacity() {
    let mut output = String::from("x");
    let capacity = output.capacity();
    let pointer = output.as_ptr();
    let value = "y".repeat(capacity + 1);
    assert_eq!(
        append_bounded(&mut output, &value, 1),
        Err(BoundedRegexReplacementError::OutputTooLong { len: capacity + 2 })
    );
    assert_eq!(output, "x");
    assert_eq!(output.capacity(), capacity);
    assert_eq!(output.as_ptr(), pointer);
}

#[test]
fn empty_append_at_zero_limit_keeps_output_unallocated() {
    let mut output = String::new();
    assert_eq!(append_bounded(&mut output, "", 0), Ok(()));
    assert_eq!(output.len(), 0);
    assert_eq!(output.capacity(), 0);
}
