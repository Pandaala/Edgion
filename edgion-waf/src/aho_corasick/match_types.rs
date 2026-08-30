use super::{MatchError, PatternIdError, SpanError};

/// Search-time anchoring mode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Anchored {
    /// Permit a match to begin anywhere in the validated range.
    #[default]
    No,
    /// Require a match to begin at the current range start.
    Yes,
}

impl Anchored {
    /// Return whether this mode requires a match at the range start.
    pub const fn is_anchored(self) -> bool {
        matches!(self, Self::Yes)
    }
}

/// Anchored start modes accepted by a compiled matcher.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartKind {
    /// Accept anchored and unanchored searches.
    Both,
    /// Accept only unanchored searches.
    #[default]
    Unanchored,
    /// Accept only anchored searches.
    Anchored,
}

impl StartKind {
    pub(crate) const fn supports(self, anchored: Anchored) -> bool {
        matches!(self, Self::Both)
            || matches!(
                (self, anchored),
                (Self::Unanchored, Anchored::No) | (Self::Anchored, Anchored::Yes)
            )
    }
}

/// Physical automaton representation selected explicitly at build time.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutomatonKind {
    /// Per-state sparse transition vectors and the portable correctness oracle.
    #[default]
    SparseNfa,
    /// Flat state, transition, and output arrays with optional dense rows.
    ContiguousNfa,
    /// Complete transition table indexed by byte equivalence class.
    Dfa,
    /// Deterministically select one physical backend from compile-time shape and limits.
    Auto,
}

/// Bounded explanation of how the physical backend was selected.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionReason {
    /// The builder requested one physical backend directly.
    #[default]
    Explicit,
    /// Auto selected DFA because its complete table fits the provisional policy and caller limits.
    AutoDfaWithinPolicy,
    /// Auto declined DFA and selected contiguous NFA for a high-fanout root.
    AutoContiguousDfaTooLarge,
    /// Auto declined DFA and retained sparse NFA for a low-fanout root.
    AutoSparseDfaTooLarge,
}

/// Optional immutable acceleration selected independently of the exact backend.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccelerationKind {
    /// Exact automaton search without candidate skipping.
    #[default]
    None,
    /// Scan for one of at most three possible folded pattern-start bytes.
    StartBytes,
    /// Scan for one of at most three necessary folded bytes and shift back conservatively.
    RareBytes,
    /// Verify bounded small-pattern candidates through two-byte scalar fingerprints.
    Packed,
    /// Shift-Or candidate scan over a fixed 64 KiB two-byte table.
    TwoGram,
}

/// Match selection semantics fixed when an automaton is built.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchKind {
    /// Report matches as soon as their end boundary is reached.
    #[default]
    Standard,
    /// Select the earliest start, breaking ties by source-order pattern ID.
    LeftmostFirst,
    /// Select the earliest start, then the longest match, then source order.
    LeftmostLongest,
}

/// Stable identifier assigned from a pattern's source-order position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PatternId(u32);

impl PatternId {
    /// Construct an identifier from a source-order index.
    pub fn try_from_usize(value: usize) -> Result<Self, PatternIdError> {
        u32::try_from(value).map(Self).map_err(|_| PatternIdError::new(value))
    }

    /// Return the source-order index as `usize`.
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Return the compact source-order index.
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// Half-open byte span of one match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    start: usize,
    end: usize,
}

impl Span {
    /// Construct a validated half-open byte span.
    pub const fn try_new(start: usize, end: usize) -> Result<Self, SpanError> {
        if start > end {
            return Err(SpanError::new(start, end));
        }
        Ok(Self { start, end })
    }

    pub(crate) const fn from_validated(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Return the inclusive start byte offset.
    pub const fn start(self) -> usize {
        self.start
    }

    /// Return the exclusive end byte offset.
    pub const fn end(self) -> usize {
        self.end
    }

    /// Return the byte length of this span.
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    /// Return whether this span contains no bytes.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// One pattern match with a stable identifier and exact byte span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Match {
    pattern_id: PatternId,
    span: Span,
}

impl Match {
    /// Construct a match from a validated pattern identifier and byte offsets.
    pub const fn try_new(pattern_id: PatternId, start: usize, end: usize) -> Result<Self, SpanError> {
        let span = match Span::try_new(start, end) {
            Ok(span) => span,
            Err(error) => return Err(error),
        };
        Ok(Self { pattern_id, span })
    }

    pub(crate) const fn from_validated(pattern_id: PatternId, start: usize, end: usize) -> Self {
        Self {
            pattern_id,
            span: Span::from_validated(start, end),
        }
    }

    /// Return the matching source-order pattern identifier.
    pub const fn pattern_id(self) -> PatternId {
        self.pattern_id
    }

    /// Return the match byte span.
    pub const fn span(self) -> Span {
        self.span
    }

    /// Return the inclusive start byte offset.
    pub const fn start(self) -> usize {
        self.span.start()
    }

    /// Return the exclusive end byte offset.
    pub const fn end(self) -> usize {
        self.span.end()
    }
}

/// Reusable bitset containing every matching pattern identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternSet {
    pattern_count: usize,
    words: Vec<u64>,
}

impl PatternSet {
    pub(crate) fn new(pattern_count: usize) -> Self {
        Self {
            pattern_count,
            words: vec![0; pattern_count.div_ceil(u64::BITS as usize)],
        }
    }

    /// Return the pattern count this set can represent.
    pub const fn pattern_count(&self) -> usize {
        self.pattern_count
    }

    /// Clear all matches without changing allocated capacity.
    pub fn clear(&mut self) {
        self.words.fill(0);
    }

    /// Return whether the given pattern identifier is present.
    pub fn contains(&self, pattern_id: PatternId) -> bool {
        let index = pattern_id.as_usize();
        if index >= self.pattern_count {
            return false;
        }
        let word_index = index / u64::BITS as usize;
        let bit_index = index % u64::BITS as usize;
        self.words
            .get(word_index)
            .is_some_and(|word| word & (1_u64 << bit_index) != 0)
    }

    /// Iterate over present identifiers exactly once in source order.
    pub fn iter(&self) -> impl Iterator<Item = PatternId> + '_ {
        (0..self.pattern_count).filter_map(|index| {
            let pattern_id = PatternId::try_from_usize(index).ok()?;
            self.contains(pattern_id).then_some(pattern_id)
        })
    }

    /// Return whether no pattern identifier is present.
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    /// Return the retained bitset bytes. Repeated scans do not grow this value.
    pub fn memory_usage(&self) -> usize {
        self.words.capacity().saturating_mul(std::mem::size_of::<u64>())
    }

    pub(crate) fn validate_shape(&self, expected: usize) -> Result<(), MatchError> {
        let expected_words = expected.div_ceil(u64::BITS as usize);
        if self.pattern_count != expected || self.words.len() != expected_words {
            return Err(MatchError::PatternSetShape {
                expected,
                actual: self.pattern_count,
            });
        }
        Ok(())
    }

    pub(crate) fn insert(&mut self, pattern_id: PatternId) -> Result<(), MatchError> {
        let index = pattern_id.as_usize();
        if index >= self.pattern_count {
            return Err(MatchError::InvalidPatternId {
                pattern_id: index,
                pattern_count: self.pattern_count,
            });
        }
        let word_index = index / u64::BITS as usize;
        let bit_index = index % u64::BITS as usize;
        let Some(word) = self.words.get_mut(word_index) else {
            return Err(MatchError::PatternSetShape {
                expected: self.pattern_count,
                actual: self.words.len().saturating_mul(u64::BITS as usize),
            });
        };
        *word |= 1_u64 << bit_index;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn word_capacity(&self) -> usize {
        self.words.capacity()
    }

    #[cfg(test)]
    pub(crate) fn word_allocation_ptr(&self) -> *const u64 {
        self.words.as_ptr()
    }
}
