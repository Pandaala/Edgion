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
        let close = replacement[2..].find('}')? + 2;
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

fn output_growth_target(capacity: usize, next: usize, limit: usize) -> usize {
    next.max(capacity.saturating_mul(2)).min(limit)
}

fn append_bounded(output: &mut String, value: &str, limit: usize) -> Result<(), BoundedRegexReplacementError> {
    let next = checked_output_len(output.len(), value.len(), limit)?;
    if next > output.capacity() {
        // Retain amortized growth without requesting capacity beyond the limit.
        let target = output_growth_target(output.capacity(), next, limit);
        output
            .try_reserve_exact(target - output.len())
            .map_err(|_| BoundedRegexReplacementError::AllocationFailed)?;
    }
    output.push_str(value);
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/common/bounded_regex_replacement_test.rs"]
mod tests;
