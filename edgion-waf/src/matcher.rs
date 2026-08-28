//! Bounded multi-pattern matching primitives.
//!
//! The phrase matcher is an in-crate Aho-Corasick implementation. Regular
//! expressions use the workspace's general-purpose `regex-automata` engine;
//! no external WAF engine is linked or embedded.

use crate::aho_corasick::{
    AhoCorasick, BuildError as AcBuildError, BuildLimits as AcBuildLimits, Builder as AcBuilder,
    MatchError as AcMatchError, PatternSet as AcPatternSet,
};
use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use regex_automata::{Input, MatchKind, PatternSet as RegexPatternSet};
use thiserror::Error;

/// Default maximum number of patterns in one compiled matcher.
pub const DEFAULT_MAX_PATTERNS: usize = 4_096;
/// Default maximum combined source bytes in one compiled matcher.
pub const DEFAULT_MAX_PATTERN_BYTES: usize = 4 * 1024 * 1024;
/// Default maximum number of states in one phrase automaton.
pub const DEFAULT_MAX_PHRASE_STATES: usize = 1_000_000;
/// Default maximum number of transitions in one phrase automaton.
pub const DEFAULT_MAX_PHRASE_TRANSITIONS: usize = 1_000_000;
/// Default maximum number of direct outputs in one phrase automaton.
pub const DEFAULT_MAX_PHRASE_OUTPUTS: usize = DEFAULT_MAX_PATTERNS;
/// Default maximum estimated immutable bytes in one phrase automaton.
pub const DEFAULT_MAX_PHRASE_COMPILED_BYTES: usize = 256 * 1024 * 1024;
/// Default maximum complete-DFA cells available to a phrase automaton.
pub const DEFAULT_MAX_PHRASE_DFA_CELLS: usize = 16 * 1024 * 1024;
/// Default maximum Thompson NFA size for one multi-regex matcher.
pub const DEFAULT_MAX_REGEX_NFA_BYTES: usize = 32 * 1024 * 1024;
/// Default maximum fully compiled DFA size for one multi-regex matcher.
pub const DEFAULT_MAX_REGEX_DFA_BYTES: usize = 64 * 1024 * 1024;

/// Compilation limits for one matching group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatcherLimits {
    /// Maximum number of phrase and regex patterns combined.
    pub max_patterns: usize,
    /// Maximum combined bytes across all pattern source strings.
    pub max_pattern_bytes: usize,
    /// Maximum states in either case-sensitive phrase automaton.
    pub max_phrase_states: usize,
    /// Maximum transitions in either case-sensitive phrase automaton.
    pub max_phrase_transitions: usize,
    /// Maximum direct outputs in either case-sensitive phrase automaton.
    pub max_phrase_outputs: usize,
    /// Maximum estimated immutable bytes in either phrase automaton.
    pub max_phrase_compiled_bytes: usize,
    /// Maximum complete-DFA transition cells in either phrase automaton.
    pub max_phrase_dfa_cells: usize,
    /// Maximum bytes used by the compiled regex Thompson NFA.
    pub max_regex_nfa_bytes: usize,
    /// Maximum bytes used by a fully compiled regex DFA.
    pub max_regex_dfa_bytes: usize,
}

impl Default for MatcherLimits {
    fn default() -> Self {
        Self {
            max_patterns: DEFAULT_MAX_PATTERNS,
            max_pattern_bytes: DEFAULT_MAX_PATTERN_BYTES,
            max_phrase_states: DEFAULT_MAX_PHRASE_STATES,
            max_phrase_transitions: DEFAULT_MAX_PHRASE_TRANSITIONS,
            max_phrase_outputs: DEFAULT_MAX_PHRASE_OUTPUTS,
            max_phrase_compiled_bytes: DEFAULT_MAX_PHRASE_COMPILED_BYTES,
            max_phrase_dfa_cells: DEFAULT_MAX_PHRASE_DFA_CELLS,
            max_regex_nfa_bytes: DEFAULT_MAX_REGEX_NFA_BYTES,
            max_regex_dfa_bytes: DEFAULT_MAX_REGEX_DFA_BYTES,
        }
    }
}

/// Stable identifier assigned from a pattern's source-order position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PatternId(usize);

impl PatternId {
    /// Return the zero-based source-order position.
    pub const fn as_usize(self) -> usize {
        self.0
    }
}

/// One pattern in a matching group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternSpec<'a> {
    /// Exact substring phrase matching, corresponding to the matching core of
    /// SecLang `@pm`.
    Phrase {
        /// Phrase bytes. Empty phrases are rejected.
        value: &'a str,
        /// Whether ASCII letters compare without case distinctions.
        ascii_case_insensitive: bool,
    },
    /// Exact regular-expression matching for the supported Rust-regex subset.
    Regex(&'a str),
}

impl PatternSpec<'_> {
    fn source_len(self) -> usize {
        match self {
            Self::Phrase { value, .. } | Self::Regex(value) => value.len(),
        }
    }
}

/// Error returned while compiling a matching group.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MatcherCompileError {
    /// The group exceeds its configured pattern-count bound.
    #[error("matcher has {actual} patterns, exceeding the configured limit of {limit}; split the rule group or raise the compile limit")]
    PatternLimit { actual: usize, limit: usize },
    /// The group exceeds its configured combined source-byte bound.
    #[error("matcher has {actual} pattern bytes, exceeding the configured limit of {limit}; reduce the rule set or raise the compile limit")]
    PatternByteLimit { actual: usize, limit: usize },
    /// An empty phrase was supplied.
    #[error("phrase pattern {pattern_id} is empty; remove it or use an operator with explicit empty-input semantics")]
    EmptyPhrase { pattern_id: usize },
    /// A phrase automaton exceeds its state bound.
    #[error("phrase automaton requires more than {limit} states; split the matching group or raise the compile limit")]
    PhraseStateLimit { limit: usize },
    /// A phrase automaton exceeds its transition bound.
    #[error(
        "phrase automaton requires more than {limit} transitions; split the matching group or raise the compile limit"
    )]
    PhraseTransitionLimit { limit: usize },
    /// A phrase automaton exceeds its direct-output bound.
    #[error("phrase automaton requires more than {limit} direct outputs; split the matching group or raise the compile limit")]
    PhraseOutputLimit { limit: usize },
    /// A phrase automaton exceeds its estimated immutable-byte bound.
    #[error("phrase automaton requires more than {limit} immutable bytes; split the matching group or raise the compile limit")]
    PhraseCompiledByteLimit { limit: usize },
    /// A phrase automaton failed for another bounded compilation reason.
    #[error("phrase automaton cannot be compiled: {message}")]
    PhraseBuild { message: String },
    /// One or more regular expressions cannot be compiled.
    #[error("regex pattern {pattern_id:?} cannot be compiled for the supported profile: {message}")]
    Regex { pattern_id: Option<usize>, message: String },
}

/// Error returned when scratch memory belongs to a different matcher shape.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MatcherScanError {
    /// The compiled phrase search mode cannot serve an all-pattern scan.
    #[error("compiled phrase matcher mode does not support the requested scan; rebuild the rule generation")]
    PhraseSearchMode,
    /// Scratch memory was created for another compiled matcher.
    #[error(
        "matcher scratch shape does not match the compiled matcher; create scratch from the active rule generation"
    )]
    ScratchShape,
    /// A compiled matcher contained an invalid internal pattern identifier.
    #[error("compiled matcher contains an invalid pattern identifier; discard the rule generation and recompile it")]
    InvalidPatternId,
}

#[derive(Debug)]
struct PhraseGroup {
    matcher: AhoCorasick,
    pattern_ids: Vec<PatternId>,
}

impl PhraseGroup {
    fn compile(
        patterns: &[(PatternId, &str)],
        ascii_case_insensitive: bool,
        limits: MatcherLimits,
    ) -> Result<Option<Self>, MatcherCompileError> {
        if patterns.is_empty() {
            return Ok(None);
        }

        let mut builder = AcBuilder::new();
        builder
            .ascii_case_insensitive(ascii_case_insensitive)
            .limits(AcBuildLimits {
                max_patterns: limits.max_patterns,
                max_pattern_bytes: limits.max_pattern_bytes,
                max_states: limits.max_phrase_states,
                max_transitions: limits.max_phrase_transitions,
                max_outputs: limits.max_phrase_outputs,
                max_compiled_bytes: limits.max_phrase_compiled_bytes,
                max_dfa_cells: limits.max_phrase_dfa_cells,
            });
        let matcher = builder
            .build(patterns.iter().map(|(_, pattern)| pattern.as_bytes()))
            .map_err(map_phrase_build_error)?;
        Ok(Some(Self {
            matcher,
            pattern_ids: patterns.iter().map(|(pattern_id, _)| *pattern_id).collect(),
        }))
    }

    fn create_pattern_set(&self) -> AcPatternSet {
        self.matcher.create_pattern_set()
    }

    fn scan(
        &self,
        haystack: &[u8],
        phrase_matches: &mut AcPatternSet,
        matched: &mut [u64],
    ) -> Result<(), MatcherScanError> {
        self.matcher
            .scan_pattern_set(haystack, phrase_matches)
            .map_err(map_phrase_scan_error)?;
        for local_id in phrase_matches.iter() {
            let Some(pattern_id) = self.pattern_ids.get(local_id.as_usize()) else {
                return Err(MatcherScanError::InvalidPatternId);
            };
            set_match(matched, *pattern_id)?;
        }
        Ok(())
    }
}

fn map_phrase_build_error(error: AcBuildError) -> MatcherCompileError {
    match error {
        AcBuildError::PatternLimit { actual, limit } => MatcherCompileError::PatternLimit { actual, limit },
        AcBuildError::PatternByteLimit { actual, limit } => MatcherCompileError::PatternByteLimit { actual, limit },
        AcBuildError::StateLimit { limit, .. } => MatcherCompileError::PhraseStateLimit { limit },
        AcBuildError::TransitionLimit { limit, .. } => MatcherCompileError::PhraseTransitionLimit { limit },
        AcBuildError::OutputLimit { limit, .. } => MatcherCompileError::PhraseOutputLimit { limit },
        AcBuildError::CompiledByteLimit { limit, .. } => MatcherCompileError::PhraseCompiledByteLimit { limit },
        other => MatcherCompileError::PhraseBuild {
            message: other.to_string(),
        },
    }
}

fn map_phrase_scan_error(error: AcMatchError) -> MatcherScanError {
    match error {
        AcMatchError::UnsupportedOverlapping { .. } => MatcherScanError::PhraseSearchMode,
        AcMatchError::PatternSetShape { .. } => MatcherScanError::ScratchShape,
        AcMatchError::InvalidPatternId { .. } => MatcherScanError::InvalidPatternId,
        _ => MatcherScanError::PhraseSearchMode,
    }
}

fn set_match(words: &mut [u64], pattern_id: PatternId) -> Result<(), MatcherScanError> {
    let word_index = pattern_id.0 / u64::BITS as usize;
    let bit_index = pattern_id.0 % u64::BITS as usize;
    let Some(word) = words.get_mut(word_index) else {
        return Err(MatcherScanError::InvalidPatternId);
    };
    *word |= 1_u64 << bit_index;
    Ok(())
}

/// Immutable compiled matcher for one semantically equivalent input group.
#[derive(Debug)]
pub struct CompiledMatcher {
    pattern_count: usize,
    sensitive_phrases: Option<PhraseGroup>,
    insensitive_phrases: Option<PhraseGroup>,
    regex: Option<Regex>,
    regex_pattern_ids: Vec<PatternId>,
}

impl CompiledMatcher {
    /// Compile a bounded group and assign stable IDs in source order.
    pub fn compile(patterns: &[PatternSpec<'_>], limits: MatcherLimits) -> Result<Self, MatcherCompileError> {
        if patterns.len() > limits.max_patterns {
            return Err(MatcherCompileError::PatternLimit {
                actual: patterns.len(),
                limit: limits.max_patterns,
            });
        }
        let pattern_bytes = patterns
            .iter()
            .try_fold(0_usize, |total, pattern| total.checked_add(pattern.source_len()));
        let Some(pattern_bytes) = pattern_bytes else {
            return Err(MatcherCompileError::PatternByteLimit {
                actual: usize::MAX,
                limit: limits.max_pattern_bytes,
            });
        };
        if pattern_bytes > limits.max_pattern_bytes {
            return Err(MatcherCompileError::PatternByteLimit {
                actual: pattern_bytes,
                limit: limits.max_pattern_bytes,
            });
        }

        let mut sensitive_phrases = Vec::new();
        let mut insensitive_phrases = Vec::new();
        let mut regex_sources = Vec::new();
        let mut regex_pattern_ids = Vec::new();
        for (index, pattern) in patterns.iter().enumerate() {
            let pattern_id = PatternId(index);
            match pattern {
                PatternSpec::Phrase {
                    value,
                    ascii_case_insensitive,
                } => {
                    if value.is_empty() {
                        return Err(MatcherCompileError::EmptyPhrase { pattern_id: index });
                    }
                    if *ascii_case_insensitive {
                        insensitive_phrases.push((pattern_id, *value));
                    } else {
                        sensitive_phrases.push((pattern_id, *value));
                    }
                }
                PatternSpec::Regex(source) => {
                    regex_sources.push(*source);
                    regex_pattern_ids.push(pattern_id);
                }
            }
        }

        let sensitive_phrases = PhraseGroup::compile(&sensitive_phrases, false, limits)?;
        let insensitive_phrases = PhraseGroup::compile(&insensitive_phrases, true, limits)?;
        let regex = compile_regex(&regex_sources, &regex_pattern_ids, limits)?;

        Ok(Self {
            pattern_count: patterns.len(),
            sensitive_phrases,
            insensitive_phrases,
            regex,
            regex_pattern_ids,
        })
    }

    /// Return the number of patterns and therefore the valid ID range.
    pub const fn pattern_count(&self) -> usize {
        self.pattern_count
    }

    /// Allocate reusable scratch sized for this compiled generation.
    pub fn create_scratch(&self) -> MatchScratch {
        MatchScratch {
            pattern_count: self.pattern_count,
            matched_words: vec![0; self.pattern_count.div_ceil(u64::BITS as usize)],
            sensitive_phrase_matches: self.sensitive_phrases.as_ref().map(PhraseGroup::create_pattern_set),
            insensitive_phrase_matches: self.insensitive_phrases.as_ref().map(PhraseGroup::create_pattern_set),
            regex_matches: RegexPatternSet::new(self.regex_pattern_ids.len()),
        }
    }

    /// Match all patterns against one byte-oriented value.
    ///
    /// Results are a source-order bitset and remain valid until the scratch is
    /// reused. No pattern is compiled on this path.
    pub fn scan<'a>(
        &self,
        haystack: &[u8],
        scratch: &'a mut MatchScratch,
    ) -> Result<MatchResults<'a>, MatcherScanError> {
        if scratch.pattern_count != self.pattern_count
            || scratch.regex_matches.capacity() != self.regex_pattern_ids.len()
        {
            return Err(MatcherScanError::ScratchShape);
        }
        scratch.matched_words.fill(0);
        scratch.regex_matches.clear();

        scan_phrase_group(
            self.sensitive_phrases.as_ref(),
            scratch.sensitive_phrase_matches.as_mut(),
            haystack,
            &mut scratch.matched_words,
        )?;
        scan_phrase_group(
            self.insensitive_phrases.as_ref(),
            scratch.insensitive_phrase_matches.as_mut(),
            haystack,
            &mut scratch.matched_words,
        )?;
        if let Some(regex) = &self.regex {
            regex.which_overlapping_matches(&Input::new(haystack), &mut scratch.regex_matches);
            for local_id in scratch.regex_matches.iter() {
                let Some(pattern_id) = self.regex_pattern_ids.get(local_id.as_usize()) else {
                    return Err(MatcherScanError::InvalidPatternId);
                };
                set_match(&mut scratch.matched_words, *pattern_id)?;
            }
        }

        Ok(MatchResults {
            matched_words: &scratch.matched_words,
            pattern_count: self.pattern_count,
        })
    }
}

fn scan_phrase_group(
    group: Option<&PhraseGroup>,
    phrase_matches: Option<&mut AcPatternSet>,
    haystack: &[u8],
    matched: &mut [u64],
) -> Result<(), MatcherScanError> {
    match (group, phrase_matches) {
        (Some(group), Some(phrase_matches)) => group.scan(haystack, phrase_matches, matched),
        (None, None) => Ok(()),
        _ => Err(MatcherScanError::ScratchShape),
    }
}

fn compile_regex(
    patterns: &[&str],
    pattern_ids: &[PatternId],
    limits: MatcherLimits,
) -> Result<Option<Regex>, MatcherCompileError> {
    if patterns.is_empty() {
        return Ok(None);
    }
    Regex::builder()
        .configure(
            Regex::config()
                .match_kind(MatchKind::All)
                .nfa_size_limit(Some(limits.max_regex_nfa_bytes))
                .dfa_size_limit(Some(limits.max_regex_dfa_bytes)),
        )
        .syntax(syntax::Config::new().utf8(false))
        .build_many(patterns)
        .map(Some)
        .map_err(|error| MatcherCompileError::Regex {
            pattern_id: error
                .pattern()
                .and_then(|id| pattern_ids.get(id.as_usize()))
                .map(|id| id.as_usize()),
            message: error.to_string(),
        })
}

/// Reusable, matcher-generation-specific scan memory.
#[derive(Debug)]
pub struct MatchScratch {
    pattern_count: usize,
    matched_words: Vec<u64>,
    sensitive_phrase_matches: Option<AcPatternSet>,
    insensitive_phrase_matches: Option<AcPatternSet>,
    regex_matches: RegexPatternSet,
}

/// Borrowed result view in stable source order.
#[derive(Debug, Clone, Copy)]
pub struct MatchResults<'a> {
    matched_words: &'a [u64],
    pattern_count: usize,
}

impl MatchResults<'_> {
    /// Return whether a specific source-order pattern matched.
    pub fn contains(self, pattern_id: PatternId) -> bool {
        if pattern_id.0 >= self.pattern_count {
            return false;
        }
        let word_index = pattern_id.0 / u64::BITS as usize;
        let bit_index = pattern_id.0 % u64::BITS as usize;
        self.matched_words
            .get(word_index)
            .is_some_and(|word| word & (1_u64 << bit_index) != 0)
    }

    /// Iterate over every matching ID exactly once in source order.
    pub fn iter(&self) -> impl Iterator<Item = PatternId> + '_ {
        (0..self.pattern_count)
            .map(PatternId)
            .filter(move |pattern_id| self.contains(*pattern_id))
    }

    /// Return whether no pattern matched.
    pub fn is_empty(self) -> bool {
        self.matched_words.iter().all(|word| *word == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(results: MatchResults<'_>) -> Vec<usize> {
        results.iter().map(PatternId::as_usize).collect()
    }

    #[test]
    fn overlapping_phrases_all_match() {
        let patterns = [
            PatternSpec::Phrase {
                value: "he",
                ascii_case_insensitive: false,
            },
            PatternSpec::Phrase {
                value: "she",
                ascii_case_insensitive: false,
            },
            PatternSpec::Phrase {
                value: "hers",
                ascii_case_insensitive: false,
            },
        ];
        let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
        let mut scratch = matcher.create_scratch();

        assert_eq!(ids(matcher.scan(b"ushers", &mut scratch).unwrap()), vec![0, 1, 2]);
    }

    #[test]
    fn phrase_case_policies_are_independent() {
        let patterns = [
            PatternSpec::Phrase {
                value: "Admin",
                ascii_case_insensitive: false,
            },
            PatternSpec::Phrase {
                value: "Root",
                ascii_case_insensitive: true,
            },
        ];
        let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
        let mut scratch = matcher.create_scratch();

        assert_eq!(ids(matcher.scan(b"admin ROOT", &mut scratch).unwrap()), vec![1]);
    }

    #[test]
    fn phrase_and_regex_results_keep_source_order() {
        let patterns = [
            PatternSpec::Regex(r"union\s+select"),
            PatternSpec::Phrase {
                value: "script",
                ascii_case_insensitive: true,
            },
            PatternSpec::Regex(r"(?i)select"),
        ];
        let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
        let mut scratch = matcher.create_scratch();

        assert_eq!(
            ids(matcher.scan(b"<SCRIPT> union select", &mut scratch).unwrap()),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn byte_oriented_scan_tolerates_invalid_utf8() {
        let patterns = [
            PatternSpec::Phrase {
                value: "admin",
                ascii_case_insensitive: false,
            },
            PatternSpec::Regex("admin"),
        ];
        let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
        let mut scratch = matcher.create_scratch();

        assert_eq!(ids(matcher.scan(b"\xFFadmin", &mut scratch).unwrap()), vec![0, 1]);
    }

    #[test]
    fn duplicate_patterns_keep_distinct_ids() {
        let patterns = [
            PatternSpec::Phrase {
                value: "same",
                ascii_case_insensitive: false,
            },
            PatternSpec::Phrase {
                value: "same",
                ascii_case_insensitive: false,
            },
        ];
        let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
        let mut scratch = matcher.create_scratch();

        assert_eq!(ids(matcher.scan(b"same", &mut scratch).unwrap()), vec![0, 1]);
    }

    #[test]
    fn suffix_patterns_are_reported_through_the_phrase_adapter() {
        let source_patterns = ["a", "aa", "aaa", "aaaa", "aaaaa", "aaaaaa"];
        let patterns: Vec<PatternSpec<'_>> = source_patterns
            .iter()
            .map(|value| PatternSpec::Phrase {
                value,
                ascii_case_insensitive: false,
            })
            .collect();
        let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
        let mut scratch = matcher.create_scratch();

        assert_eq!(
            ids(matcher.scan(b"aaaaaa", &mut scratch).unwrap()),
            vec![0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn scratch_reuse_clears_previous_results() {
        let patterns = [PatternSpec::Phrase {
            value: "attack",
            ascii_case_insensitive: false,
        }];
        let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
        let mut scratch = matcher.create_scratch();

        assert_eq!(ids(matcher.scan(b"attack", &mut scratch).unwrap()), vec![0]);
        assert!(matcher.scan(b"normal", &mut scratch).unwrap().is_empty());
    }

    #[test]
    fn scratch_from_another_matcher_is_rejected() {
        let first = CompiledMatcher::compile(&[PatternSpec::Regex("a")], MatcherLimits::default()).unwrap();
        let second = CompiledMatcher::compile(
            &[
                PatternSpec::Regex("a"),
                PatternSpec::Phrase {
                    value: "b",
                    ascii_case_insensitive: false,
                },
            ],
            MatcherLimits::default(),
        )
        .unwrap();
        let mut scratch = first.create_scratch();

        assert!(matches!(
            second.scan(b"ab", &mut scratch),
            Err(MatcherScanError::ScratchShape)
        ));
    }

    #[test]
    fn compile_limits_are_enforced() {
        let limits = MatcherLimits {
            max_patterns: 1,
            max_pattern_bytes: 3,
            max_phrase_states: 3,
            ..MatcherLimits::default()
        };
        let too_many = [PatternSpec::Regex("a"), PatternSpec::Regex("b")];
        assert_eq!(
            CompiledMatcher::compile(&too_many, limits).unwrap_err(),
            MatcherCompileError::PatternLimit { actual: 2, limit: 1 }
        );

        let too_large = [PatternSpec::Regex("abcd")];
        assert_eq!(
            CompiledMatcher::compile(&too_large, limits).unwrap_err(),
            MatcherCompileError::PatternByteLimit { actual: 4, limit: 3 }
        );

        let state_limits = MatcherLimits {
            max_patterns: 1,
            max_pattern_bytes: 16,
            max_phrase_states: 3,
            ..MatcherLimits::default()
        };
        let too_many_states = [PatternSpec::Phrase {
            value: "abc",
            ascii_case_insensitive: false,
        }];
        assert_eq!(
            CompiledMatcher::compile(&too_many_states, state_limits).unwrap_err(),
            MatcherCompileError::PhraseStateLimit { limit: 3 }
        );

        let transition_limits = MatcherLimits {
            max_phrase_transitions: 1,
            ..MatcherLimits::default()
        };
        let too_many_transitions = [PatternSpec::Phrase {
            value: "ab",
            ascii_case_insensitive: false,
        }];
        assert_eq!(
            CompiledMatcher::compile(&too_many_transitions, transition_limits).unwrap_err(),
            MatcherCompileError::PhraseTransitionLimit { limit: 1 }
        );

        let output_limits = MatcherLimits {
            max_phrase_outputs: 1,
            ..MatcherLimits::default()
        };
        let too_many_outputs = [
            PatternSpec::Phrase {
                value: "same",
                ascii_case_insensitive: false,
            },
            PatternSpec::Phrase {
                value: "same",
                ascii_case_insensitive: false,
            },
        ];
        assert_eq!(
            CompiledMatcher::compile(&too_many_outputs, output_limits).unwrap_err(),
            MatcherCompileError::PhraseOutputLimit { limit: 1 }
        );

        let compiled_byte_limits = MatcherLimits {
            max_phrase_compiled_bytes: 0,
            ..MatcherLimits::default()
        };
        let compiled_bytes = [PatternSpec::Phrase {
            value: "a",
            ascii_case_insensitive: false,
        }];
        assert_eq!(
            CompiledMatcher::compile(&compiled_bytes, compiled_byte_limits).unwrap_err(),
            MatcherCompileError::PhraseCompiledByteLimit { limit: 0 }
        );
    }

    #[test]
    fn empty_phrase_is_rejected() {
        let error = CompiledMatcher::compile(
            &[PatternSpec::Phrase {
                value: "",
                ascii_case_insensitive: false,
            }],
            MatcherLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error, MatcherCompileError::EmptyPhrase { pattern_id: 0 });
    }

    #[test]
    fn invalid_regex_identifies_its_source_pattern() {
        let error = CompiledMatcher::compile(
            &[
                PatternSpec::Phrase {
                    value: "valid",
                    ascii_case_insensitive: false,
                },
                PatternSpec::Regex("("),
            ],
            MatcherLimits::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            MatcherCompileError::Regex {
                pattern_id: Some(1),
                ..
            }
        ));
    }

    #[test]
    fn phrase_automaton_matches_naive_reference() {
        let source_patterns = ["a", "ab", "bab", "bc", "bca", "c", "caa"];
        let patterns: Vec<PatternSpec<'_>> = source_patterns
            .iter()
            .map(|value| PatternSpec::Phrase {
                value,
                ascii_case_insensitive: false,
            })
            .collect();
        let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
        let mut scratch = matcher.create_scratch();
        let mut state = 0xA5A5_1234_u64;

        for _ in 0..2_000 {
            let length = (next_random(&mut state) % 64) as usize;
            let haystack: Vec<u8> = (0..length)
                .map(|_| b'a' + (next_random(&mut state) % 4) as u8)
                .collect();
            let actual = ids(matcher.scan(&haystack, &mut scratch).unwrap());
            let expected: Vec<usize> = source_patterns
                .iter()
                .enumerate()
                .filter_map(|(index, pattern)| {
                    haystack
                        .windows(pattern.len())
                        .any(|window| window == pattern.as_bytes())
                        .then_some(index)
                })
                .collect();
            assert_eq!(actual, expected, "haystack={haystack:?}");
        }
    }

    fn next_random(state: &mut u64) -> u64 {
        *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        *state
    }
}
