use std::error::Error;
use std::fmt;

use super::{Anchored, MatchKind, StartKind};

/// Error returned when a source-order index cannot fit in [`PatternId`](crate::aho_corasick::PatternId).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternIdError {
    value: usize,
}

impl PatternIdError {
    pub(crate) const fn new(value: usize) -> Self {
        Self { value }
    }

    /// Return the rejected source-order index.
    pub const fn value(self) -> usize {
        self.value
    }
}

impl fmt::Display for PatternIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "pattern index {} exceeds the supported maximum of {}; reduce the pattern group size",
            self.value,
            u32::MAX
        )
    }
}

impl Error for PatternIdError {}

/// Error returned when a byte span has its end before its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanError {
    start: usize,
    end: usize,
}

impl SpanError {
    pub(crate) const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Return the rejected start offset.
    pub const fn start(self) -> usize {
        self.start
    }

    /// Return the rejected end offset.
    pub const fn end(self) -> usize {
        self.end
    }
}

impl fmt::Display for SpanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "match span start {} exceeds end {}; provide a half-open span with start <= end",
            self.start, self.end
        )
    }
}

impl Error for SpanError {}

/// Error returned while compiling a phrase automaton.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// The pattern count exceeds its configured bound.
    PatternLimit { actual: usize, limit: usize },
    /// The combined pattern bytes exceed their configured bound.
    PatternByteLimit { actual: usize, limit: usize },
    /// The trie state count exceeds its configured bound.
    StateLimit { actual: usize, limit: usize },
    /// The trie transition count exceeds its configured bound.
    TransitionLimit { actual: usize, limit: usize },
    /// The direct output count exceeds its configured bound.
    OutputLimit { actual: usize, limit: usize },
    /// The estimated immutable representation exceeds its configured bound.
    CompiledByteLimit { actual: usize, limit: usize },
    /// The complete DFA transition table exceeds its independent cell bound.
    DfaCellLimit { actual: usize, limit: usize },
    /// A pattern index cannot fit in the compact public identifier.
    PatternId(PatternIdError),
    /// Checked size arithmetic overflowed.
    SizeOverflow { component: &'static str },
    /// A contiguous index cannot fit in its fixed-width representation.
    IndexOverflow { component: &'static str, value: usize },
    /// Dense depth exceeds the reviewed bound.
    DenseDepthLimit { actual: usize, limit: usize },
    /// A bounded vector could not reserve its required memory.
    Allocation { component: &'static str },
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PatternLimit { actual, limit } => write!(
                formatter,
                "phrase matcher has {actual} patterns, exceeding the configured limit of {limit}; split the group or raise the reviewed limit"
            ),
            Self::PatternByteLimit { actual, limit } => write!(
                formatter,
                "phrase matcher has {actual} pattern bytes, exceeding the configured limit of {limit}; reduce the group or raise the reviewed limit"
            ),
            Self::StateLimit { actual, limit } => write!(
                formatter,
                "phrase automaton requires {actual} states, exceeding the configured limit of {limit}; split the group or raise the reviewed limit"
            ),
            Self::TransitionLimit { actual, limit } => write!(
                formatter,
                "phrase automaton requires {actual} transitions, exceeding the configured limit of {limit}; split the group or raise the reviewed limit"
            ),
            Self::OutputLimit { actual, limit } => write!(
                formatter,
                "phrase automaton requires {actual} direct outputs, exceeding the configured limit of {limit}; split the group or raise the reviewed limit"
            ),
            Self::CompiledByteLimit { actual, limit } => write!(
                formatter,
                "phrase automaton requires an estimated {actual} immutable bytes, exceeding the configured limit of {limit}; select a smaller group or raise the reviewed limit"
            ),
            Self::DfaCellLimit { actual, limit } => write!(
                formatter,
                "phrase DFA requires {actual} transition cells, exceeding the configured limit of {limit}; select an NFA, split the group, or raise the reviewed limit"
            ),
            Self::PatternId(error) => error.fmt(formatter),
            Self::SizeOverflow { component } => write!(
                formatter,
                "phrase automaton size arithmetic overflowed while calculating {component}; reduce the pattern group"
            ),
            Self::IndexOverflow { component, value } => write!(
                formatter,
                "phrase automaton {component} value {value} exceeds the fixed-width index limit of {}; reduce the pattern group",
                u32::MAX - 1
            ),
            Self::DenseDepthLimit { actual, limit } => write!(
                formatter,
                "contiguous NFA dense depth {actual} exceeds the configured maximum of {limit}; select a shallower dense prefix"
            ),
            Self::Allocation { component } => write!(
                formatter,
                "phrase automaton could not reserve bounded memory for {component}; reduce the pattern group or available concurrency"
            ),
        }
    }
}

impl Error for BuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::PatternId(error) => Some(error),
            _ => None,
        }
    }
}

impl From<PatternIdError> for BuildError {
    fn from(error: PatternIdError) -> Self {
        Self::PatternId(error)
    }
}

/// Error returned when caller-owned match storage has the wrong shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchError {
    /// Overlapping search is only defined for standard match semantics.
    UnsupportedOverlapping { match_kind: MatchKind },
    /// The output bitset was sized for a different pattern count.
    PatternSetShape { expected: usize, actual: usize },
    /// A compiled output contained an identifier outside the matcher pattern range.
    InvalidPatternId { pattern_id: usize, pattern_count: usize },
    /// A requested block range is outside its haystack.
    InvalidRange {
        start: usize,
        end: usize,
        haystack_len: usize,
    },
    /// The compiled matcher does not support the requested anchor mode.
    UnsupportedAnchored { start_kind: StartKind, anchored: Anchored },
    /// Anchored overlapping iteration has no defined contract.
    UnsupportedAnchoredOverlapping,
    /// Streaming is available only for standard semantics.
    UnsupportedStream { match_kind: MatchKind },
    /// Streaming rejects automatons containing an empty pattern.
    UnsupportedStreamEmpty,
    /// Caller-owned streaming state belongs to another matcher generation.
    StreamStateShape { expected: u64, actual: u64 },
    /// A stream state was used after it reached end-of-stream.
    StreamFinished,
    /// An absolute streaming byte offset overflowed.
    StreamOffsetOverflow,
    /// A reader buffer is outside its reviewed bounds.
    StreamBufferSize { actual: usize, min: usize, max: usize },
    /// A fixed streaming buffer could not be allocated.
    StreamBufferAllocation { requested: usize },
    /// A streaming scratch-size calculation overflowed.
    StreamMemoryOverflow,
    /// Replacement count differs from the pattern count.
    ReplacementCount { expected: usize, actual: usize },
    /// Replacement output exceeds its explicit byte limit.
    ReplacementByteLimit { actual: usize, limit: usize },
    /// A bounded replacement allocation failed.
    ReplacementAllocation,
    /// A string match split a UTF-8 code point boundary.
    InvalidUtf8Boundary { start: usize, end: usize },
}

impl fmt::Display for MatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedOverlapping { match_kind } => write!(
                formatter,
                "match kind {match_kind:?} does not support overlapping searches; rebuild with MatchKind::Standard or use find_iter"
            ),
            Self::PatternSetShape { expected, actual } => write!(
                formatter,
                "pattern set has capacity for {actual} patterns but the matcher requires {expected}; create the set from the active matcher shape"
            ),
            Self::InvalidPatternId {
                pattern_id,
                pattern_count,
            } => write!(
                formatter,
                "compiled phrase matcher contains pattern ID {pattern_id} outside its pattern count of {pattern_count}; discard and rebuild the matcher"
            ),
            Self::InvalidRange { start, end, haystack_len } => write!(
                formatter,
                "search range {start}..{end} is invalid for a haystack of {haystack_len} bytes; require start <= end <= haystack length"
            ),
            Self::UnsupportedAnchored { start_kind, anchored } => write!(
                formatter,
                "matcher start kind {start_kind:?} does not support {anchored:?} search mode; rebuild with a compatible StartKind"
            ),
            Self::UnsupportedAnchoredOverlapping => write!(
                formatter,
                "anchored overlapping iteration is unsupported; use anchored non-overlapping iteration or an unanchored overlapping search"
            ),
            Self::UnsupportedStream { match_kind } => write!(
                formatter,
                "match kind {match_kind:?} does not support streaming; rebuild with MatchKind::Standard"
            ),
            Self::UnsupportedStreamEmpty => write!(
                formatter,
                "streaming does not support empty patterns; remove empty patterns or use block search"
            ),
            Self::StreamStateShape { expected, actual } => write!(
                formatter,
                "stream state generation {actual} does not match matcher generation {expected}; create state from the active matcher"
            ),
            Self::StreamFinished => write!(formatter, "stream state is already finished; create a new state for another stream"),
            Self::StreamOffsetOverflow => write!(formatter, "absolute stream offset overflowed; terminate the stream search"),
            Self::StreamBufferSize { actual, min, max } => write!(
                formatter,
                "stream buffer size {actual} is outside the supported range {min}..={max} bytes"
            ),
            Self::StreamBufferAllocation { requested } => write!(
                formatter,
                "stream reader could not reserve its bounded {requested}-byte buffer"
            ),
            Self::StreamMemoryOverflow => write!(formatter, "streaming scratch-size calculation overflowed"),
            Self::ReplacementCount { expected, actual } => write!(
                formatter,
                "replacement table has {actual} entries but matcher requires exactly {expected}"
            ),
            Self::ReplacementByteLimit { actual, limit } => write!(
                formatter,
                "replacement output requires {actual} bytes, exceeding the configured limit of {limit}"
            ),
            Self::ReplacementAllocation => write!(formatter, "replacement output could not reserve bounded memory"),
            Self::InvalidUtf8Boundary { start, end } => write!(
                formatter,
                "match span {start}..{end} splits a UTF-8 code point; use byte replacement for byte-oriented patterns"
            ),
        }
    }
}

impl Error for MatchError {}
