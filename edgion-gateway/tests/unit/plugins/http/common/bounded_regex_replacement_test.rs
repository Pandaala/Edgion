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
