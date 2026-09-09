//! Bounded `regex` replacement-string interpolation shared by HTTP plugins.

use std::ops::Range;

use regex::{Captures, Match, Regex};

/// A failure while constructing the helper-owned replacement output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundedRegexReplacementError {
    /// The first logical output prefix that exceeded the inclusive limit.
    OutputTooLong { len: usize },
    /// Reserving capacity for an already-admitted output prefix failed.
    AllocationFailed,
    /// A regex iterator violated an invariant required by replacement.
    InvariantViolation,
}

/// A replacement template compiled into the interpolation operations used by
/// `regex` 1.13.
pub(crate) struct CompiledRegexReplacement {
    replacement: Box<str>,
    tokens: Vec<ReplacementToken>,
    needs_captures: bool,
}

enum ReplacementToken {
    Literal(Range<usize>),
    Capture(usize),
}

impl CompiledRegexReplacement {
    /// Compile an owned template once for repeated replacement operations.
    pub(crate) fn compile(regex: &Regex, replacement: String) -> Self {
        let needs_captures = replacement.contains('$');
        let replacement = replacement.into_boxed_str();
        let tokens = if needs_captures {
            compile_tokens(regex, &replacement)
        } else {
            Vec::new()
        };
        Self {
            replacement,
            tokens,
            needs_captures,
        }
    }

    /// Replace every match while enforcing an inclusive logical output limit.
    ///
    /// `None` means that the input did not match and no helper-owned output
    /// string was constructed.
    pub(crate) fn replace_all_bounded(
        &self,
        regex: &Regex,
        current: &str,
        limit: usize,
    ) -> Result<Option<String>, BoundedRegexReplacementError> {
        if self.needs_captures {
            replace_captures_bounded(regex, current, &self.replacement, &self.tokens, limit)
        } else {
            replace_literal_bounded(regex, current, &self.replacement, limit)
        }
    }

    #[cfg(test)]
    pub(crate) fn needs_captures(&self) -> bool {
        self.needs_captures
    }
}

/// Replace using a dynamic owned template.
///
/// Exact no-`$` classification keeps literal templates on `find_iter` and
/// avoids compiling capture tokens. Templates containing `$` use the same
/// compiled interpolation path as immutable callers.
pub(crate) fn replace_all_bounded(
    regex: &Regex,
    current: &str,
    replacement: String,
    limit: usize,
) -> Result<Option<String>, BoundedRegexReplacementError> {
    if !replacement.contains('$') {
        return replace_literal_bounded(regex, current, &replacement, limit);
    }
    CompiledRegexReplacement::compile(regex, replacement).replace_all_bounded(regex, current, limit)
}

fn compile_tokens(regex: &Regex, replacement: &str) -> Vec<ReplacementToken> {
    let mut tokens = Vec::new();
    let mut cursor = 0;

    while let Some(relative_dollar) = replacement[cursor..].find('$') {
        let dollar = cursor + relative_dollar;
        if cursor < dollar {
            tokens.push(ReplacementToken::Literal(cursor..dollar));
        }

        if replacement.as_bytes().get(dollar + 1) == Some(&b'$') {
            tokens.push(ReplacementToken::Literal(dollar..dollar + 1));
            cursor = dollar + 2;
            continue;
        }

        let Some((reference, end)) = capture_reference(&replacement[dollar..]) else {
            tokens.push(ReplacementToken::Literal(dollar..dollar + 1));
            cursor = dollar + 1;
            continue;
        };
        cursor = dollar + end;

        let capture = reference
            .parse::<usize>()
            .ok()
            .or_else(|| regex.capture_names().position(|name| name == Some(reference)));
        if let Some(capture) = capture {
            tokens.push(ReplacementToken::Capture(capture));
        }
    }

    if cursor < replacement.len() {
        tokens.push(ReplacementToken::Literal(cursor..replacement.len()));
    }
    tokens
}

/// Parse a possible capture reference beginning at `replacement[0]`.
///
/// This mirrors regex-automata 0.4.18 interpolation, including its `usize`
/// parse followed by named-capture fallback.
fn capture_reference(replacement: &str) -> Option<(&str, usize)> {
    let bytes = replacement.as_bytes();
    if bytes.len() <= 1 || bytes[0] != b'$' {
        return None;
    }
    if bytes[1] == b'{' {
        let close = bytes[2..].iter().position(|byte| *byte == b'}')? + 2;
        return Some((&replacement[2..close], close + 1));
    }

    let mut end = 1;
    while bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        end += 1;
    }
    (end > 1).then(|| (&replacement[1..end], end))
}

fn replace_literal_bounded(
    regex: &Regex,
    current: &str,
    replacement: &str,
    limit: usize,
) -> Result<Option<String>, BoundedRegexReplacementError> {
    let mut matches = regex.find_iter(current);
    let Some(first) = matches.next() else {
        return Ok(None);
    };

    build_from_matches(
        current,
        std::iter::once(first).chain(matches),
        limit,
        |output, _matched| append_bounded(output, replacement, limit),
    )
    .map(Some)
}

fn replace_captures_bounded(
    regex: &Regex,
    current: &str,
    replacement: &str,
    tokens: &[ReplacementToken],
    limit: usize,
) -> Result<Option<String>, BoundedRegexReplacementError> {
    let mut captures = regex.captures_iter(current);
    let Some(first) = captures.next() else {
        return Ok(None);
    };

    let mut rewritten = String::new();
    let mut previous_end = 0;
    for captures in std::iter::once(first).chain(captures) {
        let matched = captures
            .get(0)
            .ok_or(BoundedRegexReplacementError::InvariantViolation)?;
        append_bounded(&mut rewritten, &current[previous_end..matched.start()], limit)?;
        append_tokens(&mut rewritten, replacement, tokens, &captures, limit)?;
        previous_end = matched.end();
    }
    append_bounded(&mut rewritten, &current[previous_end..], limit)?;
    Ok(Some(rewritten))
}

fn build_from_matches<'h>(
    current: &'h str,
    matches: impl Iterator<Item = Match<'h>>,
    limit: usize,
    mut append_replacement: impl FnMut(&mut String, Match<'h>) -> Result<(), BoundedRegexReplacementError>,
) -> Result<String, BoundedRegexReplacementError> {
    let mut rewritten = String::new();
    let mut previous_end = 0;
    for matched in matches {
        append_bounded(&mut rewritten, &current[previous_end..matched.start()], limit)?;
        append_replacement(&mut rewritten, matched)?;
        previous_end = matched.end();
    }
    append_bounded(&mut rewritten, &current[previous_end..], limit)?;
    Ok(rewritten)
}

fn append_tokens(
    output: &mut String,
    replacement: &str,
    tokens: &[ReplacementToken],
    captures: &Captures<'_>,
    limit: usize,
) -> Result<(), BoundedRegexReplacementError> {
    for token in tokens {
        match token {
            ReplacementToken::Literal(range) => append_bounded(output, &replacement[range.clone()], limit)?,
            ReplacementToken::Capture(index) => {
                if let Some(value) = captures.get(*index) {
                    append_bounded(output, value.as_str(), limit)?;
                }
            }
        }
    }
    Ok(())
}

fn checked_output_len(current: usize, additional: usize, limit: usize) -> Result<usize, BoundedRegexReplacementError> {
    let Some(len) = current.checked_add(additional) else {
        return Err(BoundedRegexReplacementError::OutputTooLong { len: usize::MAX });
    };
    if len > limit {
        return Err(BoundedRegexReplacementError::OutputTooLong { len });
    }
    Ok(len)
}

fn append_bounded(output: &mut String, value: &str, limit: usize) -> Result<(), BoundedRegexReplacementError> {
    checked_output_len(output.len(), value.len(), limit)?;
    output
        .try_reserve_exact(value.len())
        .map_err(|_| BoundedRegexReplacementError::AllocationFailed)?;
    output.push_str(value);
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
