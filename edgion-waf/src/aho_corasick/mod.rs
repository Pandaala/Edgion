//! Independent, bounded Aho-Corasick phrase matching.
//!
//! The scalar sparse NFA is the mandatory portable correctness backend. This
//! module does not depend on or wrap another matcher implementation.

mod dfa;
mod error;
mod input;
mod match_types;
mod nfa;
mod prefilter;
mod replace;
mod stream;

use std::iter::FusedIterator;
use std::sync::atomic::{AtomicU64, Ordering};

use nfa::{Backend, OutputCursor};
use prefilter::{Acceleration, Candidate, SearchState};

pub(crate) fn phrase_build_projection(patterns: usize, bytes: usize, limits: BuildLimits) -> Option<usize> {
    nfa::phrase_build_projection(patterns, bytes, limits)
}

pub use error::{BuildError, MatchError, PatternIdError, SpanError};
pub use input::SearchInput;
pub use match_types::{
    AccelerationKind, Anchored, AutomatonKind, Match, MatchKind, PatternId, PatternSet, SelectionReason, Span,
    StartKind,
};
pub use replace::{StreamReplaceError, DEFAULT_MAX_REPLACEMENT_BYTES};
pub use stream::{StreamFindIter, StreamState, DEFAULT_STREAM_BUFFER_BYTES, MAX_STREAM_BUFFER_BYTES};

/// Search-local counters for evaluating optional candidate acceleration.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SearchDiagnostics {
    candidate_attempts: u16,
    candidate_scanned_bytes: usize,
    skipped_bytes: usize,
    prefilter_disabled: bool,
}

impl SearchDiagnostics {
    /// Return the number of candidate searches, saturated at `u16::MAX`.
    pub const fn candidate_attempts(self) -> u16 {
        self.candidate_attempts
    }

    /// Return bytes examined by candidate searches.
    pub const fn candidate_scanned_bytes(self) -> usize {
        self.candidate_scanned_bytes
    }

    /// Return bytes skipped before exact automaton scanning resumed.
    pub const fn skipped_bytes(self) -> usize {
        self.skipped_bytes
    }

    /// Return whether dense candidates disabled acceleration for this search.
    pub const fn prefilter_disabled(self) -> bool {
        self.prefilter_disabled
    }
}

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Default maximum number of patterns in one automaton.
pub const DEFAULT_MAX_PATTERNS: usize = 4_096;
/// Default maximum combined pattern bytes in one automaton.
pub const DEFAULT_MAX_PATTERN_BYTES: usize = 4 * 1024 * 1024;
/// Default maximum number of trie states in one automaton.
pub const DEFAULT_MAX_STATES: usize = 1_000_000;
/// Default maximum number of trie transitions in one automaton.
pub const DEFAULT_MAX_TRANSITIONS: usize = 1_000_000;
/// Default maximum number of direct pattern outputs in one automaton.
pub const DEFAULT_MAX_OUTPUTS: usize = 4_096;
/// Default maximum owned immutable bytes in one automaton.
pub const DEFAULT_MAX_COMPILED_BYTES: usize = 256 * 1024 * 1024;
/// Default maximum number of cells in one complete DFA transition table.
pub const DEFAULT_MAX_DFA_CELLS: usize = 16 * 1024 * 1024;
/// Provisional opt-in Auto policy cap for complete DFA transition cells.
pub const AUTO_MAX_DFA_CELLS: usize = 1024 * 1024;
/// Default maximum trie depth considered for dense contiguous rows.
pub const DEFAULT_DENSE_DEPTH: usize = 2;
/// Maximum accepted dense-depth configuration.
pub const MAX_DENSE_DEPTH: usize = 16;

/// Hard compilation limits for one phrase automaton.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildLimits {
    /// Maximum number of patterns.
    pub max_patterns: usize,
    /// Maximum combined source bytes.
    pub max_pattern_bytes: usize,
    /// Maximum trie states, including the root state.
    pub max_states: usize,
    /// Maximum trie transitions.
    pub max_transitions: usize,
    /// Maximum direct pattern outputs.
    pub max_outputs: usize,
    /// Maximum owned bytes in the selected immutable representation.
    pub max_compiled_bytes: usize,
    /// Maximum transition cells in a complete DFA representation.
    pub max_dfa_cells: usize,
}

impl Default for BuildLimits {
    fn default() -> Self {
        Self {
            max_patterns: DEFAULT_MAX_PATTERNS,
            max_pattern_bytes: DEFAULT_MAX_PATTERN_BYTES,
            max_states: DEFAULT_MAX_STATES,
            max_transitions: DEFAULT_MAX_TRANSITIONS,
            max_outputs: DEFAULT_MAX_OUTPUTS,
            max_compiled_bytes: DEFAULT_MAX_COMPILED_BYTES,
            max_dfa_cells: DEFAULT_MAX_DFA_CELLS,
        }
    }
}

/// Builder for an immutable Aho-Corasick automaton.
#[derive(Debug, Clone, Copy)]
pub struct Builder {
    limits: BuildLimits,
    ascii_case_insensitive: bool,
    match_kind: MatchKind,
    automaton_kind: AutomatonKind,
    dense_depth: usize,
    prefilter: bool,
    start_kind: StartKind,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            limits: BuildLimits::default(),
            ascii_case_insensitive: false,
            match_kind: MatchKind::default(),
            automaton_kind: AutomatonKind::default(),
            dense_depth: DEFAULT_DENSE_DEPTH,
            prefilter: true,
            start_kind: StartKind::default(),
        }
    }
}

impl Builder {
    /// Create a builder with default hard limits, case-sensitive matching, and scalar acceleration enabled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace all compilation limits.
    pub fn limits(&mut self, limits: BuildLimits) -> &mut Self {
        self.limits = limits;
        self
    }

    /// Enable or disable ASCII-only case-insensitive matching.
    pub fn ascii_case_insensitive(&mut self, yes: bool) -> &mut Self {
        self.ascii_case_insensitive = yes;
        self
    }

    /// Select the match semantics fixed into the compiled matcher.
    pub fn match_kind(&mut self, match_kind: MatchKind) -> &mut Self {
        self.match_kind = match_kind;
        self
    }

    /// Select the physical automaton representation.
    pub fn automaton_kind(&mut self, automaton_kind: AutomatonKind) -> &mut Self {
        self.automaton_kind = automaton_kind;
        self
    }

    /// Set the maximum shallow depth considered for dense contiguous rows.
    pub fn dense_depth(&mut self, dense_depth: usize) -> &mut Self {
        self.dense_depth = dense_depth;
        self
    }

    /// Enable or disable bounded scalar candidate acceleration.
    pub fn prefilter(&mut self, yes: bool) -> &mut Self {
        self.prefilter = yes;
        self
    }

    /// Select which anchored search modes the compiled matcher accepts.
    pub fn start_kind(&mut self, start_kind: StartKind) -> &mut Self {
        self.start_kind = start_kind;
        self
    }

    /// Compile byte patterns in source order.
    pub fn build<I, P>(&self, patterns: I) -> Result<AhoCorasick, BuildError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        if self.dense_depth > MAX_DENSE_DEPTH {
            return Err(BuildError::DenseDepthLimit {
                actual: self.dense_depth,
                limit: MAX_DENSE_DEPTH,
            });
        }
        let (backend, acceleration, selection_reason) = match self.automaton_kind {
            AutomatonKind::SparseNfa => {
                let (backend, acceleration) = Backend::build_sparse(
                    patterns,
                    self.limits,
                    self.ascii_case_insensitive,
                    self.match_kind,
                    self.prefilter,
                )?;
                (backend, acceleration, SelectionReason::Explicit)
            }
            AutomatonKind::ContiguousNfa => {
                let (backend, acceleration) = Backend::build_contiguous(
                    patterns,
                    self.limits,
                    self.ascii_case_insensitive,
                    self.dense_depth,
                    self.match_kind,
                    self.prefilter,
                )?;
                (backend, acceleration, SelectionReason::Explicit)
            }
            AutomatonKind::Dfa => {
                let (backend, acceleration) = Backend::build_dfa(
                    patterns,
                    self.limits,
                    self.ascii_case_insensitive,
                    self.match_kind,
                    self.prefilter,
                )?;
                (backend, acceleration, SelectionReason::Explicit)
            }
            AutomatonKind::Auto => Backend::build_auto(
                patterns,
                self.limits,
                self.ascii_case_insensitive,
                self.dense_depth,
                self.match_kind,
                self.prefilter,
            )?,
        };
        let generation = next_generation()?;
        Ok(AhoCorasick {
            backend,
            acceleration,
            ascii_case_insensitive: self.ascii_case_insensitive,
            match_kind: self.match_kind,
            selection_reason,
            start_kind: self.start_kind,
            generation,
        })
    }
}

/// Immutable Aho-Corasick automaton with one selected safe-Rust backend.
#[derive(Debug)]
pub struct AhoCorasick {
    backend: Backend,
    acceleration: Acceleration,
    ascii_case_insensitive: bool,
    match_kind: MatchKind,
    selection_reason: SelectionReason,
    start_kind: StartKind,
    generation: u64,
}

impl AhoCorasick {
    /// Compile patterns with default limits and case-sensitive matching.
    pub fn new<I, P>(patterns: I) -> Result<Self, BuildError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        Builder::new().build(patterns)
    }

    /// Return the number of source patterns.
    pub fn pattern_count(&self) -> usize {
        self.backend.pattern_count()
    }

    /// Return the shortest pattern length, or `None` for an empty pattern set.
    pub fn min_pattern_len(&self) -> Option<usize> {
        self.backend.min_pattern_len()
    }

    /// Return the longest pattern length, or zero for an empty pattern set.
    pub fn max_pattern_len(&self) -> usize {
        self.backend.max_pattern_len()
    }

    /// Return owned heap bytes used by the immutable representation.
    pub fn memory_usage(&self) -> usize {
        self.backend
            .memory_usage()
            .saturating_add(self.acceleration.memory_usage())
    }

    /// Return a diagnostic estimate of peak owned bytes during construction.
    pub fn estimated_peak_build_memory_usage(&self) -> usize {
        self.backend
            .estimated_peak_build_memory_usage()
            .saturating_add(self.acceleration.memory_usage())
    }

    /// Return the selected physical automaton representation.
    pub const fn automaton_kind(&self) -> AutomatonKind {
        self.backend.automaton_kind()
    }

    /// Return the number of states using dense transition rows.
    pub fn dense_state_count(&self) -> usize {
        self.backend.dense_state_count()
    }

    /// Return the number of logical automaton states.
    pub fn state_count(&self) -> usize {
        self.backend.state_count()
    }

    /// Return the number of physical byte classes used for transition lookup.
    pub fn byte_class_count(&self) -> usize {
        self.backend.byte_class_count()
    }

    /// Return why this physical backend was selected.
    pub const fn selection_reason(&self) -> SelectionReason {
        self.selection_reason
    }

    /// Return the selected optional candidate acceleration.
    pub const fn acceleration_kind(&self) -> AccelerationKind {
        self.acceleration.kind()
    }

    /// Return heap bytes owned by candidate acceleration.
    pub const fn acceleration_memory_usage(&self) -> usize {
        self.acceleration.memory_usage()
    }

    /// Return the match semantics selected at build time.
    pub const fn match_kind(&self) -> MatchKind {
        self.match_kind
    }

    /// Return the anchored start modes accepted by this matcher.
    pub const fn start_kind(&self) -> StartKind {
        self.start_kind
    }

    /// Allocate a reusable output bitset with this matcher's shape.
    pub fn create_pattern_set(&self) -> PatternSet {
        PatternSet::new(self.pattern_count())
    }

    /// Return whether any pattern occurs in the haystack.
    pub fn is_match(&self, haystack: &[u8]) -> bool {
        self.find(haystack).is_some()
    }

    /// Return whether a validated input configuration contains a match.
    pub fn try_is_match(&self, input: SearchInput<'_>) -> Result<bool, MatchError> {
        self.try_find(input).map(|matched| matched.is_some())
    }

    /// Return the first match under the configured semantics.
    pub fn find(&self, haystack: &[u8]) -> Option<Match> {
        self.try_find(SearchInput::new(haystack))
            .expect("default unanchored search must be supported")
    }

    /// Return the first match without using optional candidate acceleration.
    ///
    /// This is a scalar correctness path for differential validation. It uses
    /// the same immutable automaton and match semantics as [`Self::find`].
    pub fn find_reference(&self, haystack: &[u8]) -> Option<Match> {
        self.validate_input(SearchInput::new(haystack))
            .expect("default unanchored reference search must be supported");
        self.find_from_input_with_acceleration(SearchInput::new(haystack), false)
    }

    /// Return the first match for a validated input configuration.
    pub fn try_find(&self, input: SearchInput<'_>) -> Result<Option<Match>, MatchError> {
        self.validate_input(input)?;
        Ok(self.find_from_input_with_acceleration(input, true))
    }

    /// Iterate over successive non-overlapping standard matches.
    pub fn find_iter<'a, 'h>(&'a self, haystack: &'h [u8]) -> FindIter<'a, 'h> {
        self.try_find_iter(SearchInput::new(haystack))
            .expect("default unanchored search must be supported")
    }

    /// Iterate over non-overlapping matches in one validated input range.
    pub fn try_find_iter<'a, 'h>(&'a self, input: SearchInput<'h>) -> Result<FindIter<'a, 'h>, MatchError> {
        self.validate_input(input)?;
        Ok(FindIter {
            matcher: self,
            input,
            last_match_end: None,
            exhausted: false,
        })
    }

    /// Lazily yield at most `limit` successive non-overlapping matches.
    pub fn find_n<'a, 'h>(&'a self, haystack: &'h [u8], limit: usize) -> std::iter::Take<FindIter<'a, 'h>> {
        self.find_iter(haystack).take(limit)
    }

    /// Iterate over every standard match, including overlapping matches.
    pub fn try_find_overlapping_iter<'a, 'h, I>(&'a self, input: I) -> Result<FindOverlappingIter<'a, 'h>, MatchError>
    where
        I: Into<SearchInput<'h>>,
    {
        let input = input.into();
        self.validate_input(input)?;
        if self.match_kind != MatchKind::Standard {
            return Err(MatchError::UnsupportedOverlapping {
                match_kind: self.match_kind,
            });
        }
        if input.anchor_mode().is_anchored() {
            return Err(MatchError::UnsupportedAnchoredOverlapping);
        }
        Ok(FindOverlappingIter {
            matcher: self,
            haystack: input.haystack(),
            state: 0,
            boundary: input.start(),
            end: input.end(),
            outputs: self.backend.output_cursor(0),
            prefilter_state: SearchState::default(),
            exhausted: false,
        })
    }

    fn find_from_input(&self, input: SearchInput<'_>) -> Option<Match> {
        self.find_from_input_with_acceleration(input, true)
    }

    fn find_from_input_with_acceleration(&self, input: SearchInput<'_>, accelerated: bool) -> Option<Match> {
        if input.start() > input.end() {
            return None;
        }
        if input.anchor_mode().is_anchored() {
            return self.find_anchored(input.haystack(), input.start(), input.end(), input.is_earliest());
        }
        if input.is_earliest() {
            return self.find_standard_from(input.haystack(), input.start(), input.end(), accelerated);
        }
        match self.match_kind {
            MatchKind::Standard => self.find_standard_from(input.haystack(), input.start(), input.end(), accelerated),
            MatchKind::LeftmostFirst | MatchKind::LeftmostLongest => {
                self.find_leftmost_from(input.haystack(), input.start(), input.end(), accelerated)
            }
        }
    }

    fn find_standard_from(&self, haystack: &[u8], start: usize, end: usize, accelerated: bool) -> Option<Match> {
        let min_pattern_len = self.min_pattern_len()?;
        let mut state = 0_usize;
        if let Some(pattern_id) = self.backend.outputs(state).next() {
            return Some(Match::from_validated(pattern_id, start, start));
        }
        if end - start < min_pattern_len {
            return None;
        }
        let mut index = start;
        let mut prefilter_state = SearchState::default();
        while index < end {
            if accelerated && state == 0 && !self.acceleration.is_none() {
                let candidate = self.acceleration.find_candidate(
                    &haystack[..end],
                    index,
                    self.ascii_case_insensitive,
                    &mut prefilter_state,
                );
                index = match candidate {
                    Candidate::None => return None,
                    Candidate::Possible { start, .. } => start,
                    Candidate::Exact { matched, .. } => matched.start(),
                };
                if index >= end {
                    return None;
                }
            }
            let byte = fold_ascii(haystack[index], self.ascii_case_insensitive);
            state = self.backend.next_state(state, byte);
            if let Some(pattern_id) = self.backend.outputs(state).next() {
                let end = index + 1;
                let match_start = end - self.backend.pattern_length(pattern_id);
                return Some(Match::from_validated(pattern_id, match_start, end));
            }
            index += 1;
        }
        None
    }

    fn find_leftmost_from(&self, haystack: &[u8], start: usize, end: usize, accelerated: bool) -> Option<Match> {
        self.min_pattern_len()?;
        let mut state = 0_usize;
        let mut best = None;
        for pattern_id in self.backend.outputs(state) {
            best = select_leftmost(self.match_kind, best, Match::from_validated(pattern_id, start, start));
        }
        if leftmost_lookahead_complete(best, start, self.max_pattern_len()) {
            return best;
        }
        let mut index = start;
        let mut prefilter_state = SearchState::default();
        while index < end {
            if accelerated && state == 0 && !self.acceleration.is_none() {
                let candidate = self.acceleration.find_candidate(
                    &haystack[..end],
                    index,
                    self.ascii_case_insensitive,
                    &mut prefilter_state,
                );
                index = match candidate {
                    Candidate::None => return best,
                    Candidate::Possible { start, .. } => start,
                    Candidate::Exact { matched, .. } => {
                        return best.or(Some(matched));
                    }
                };
                if index >= end {
                    return best;
                }
            }
            let byte = fold_ascii(haystack[index], self.ascii_case_insensitive);
            state = self.backend.next_state(state, byte);
            let end = index + 1;
            for pattern_id in self.backend.outputs(state) {
                let matched_start = end - self.backend.pattern_length(pattern_id);
                if best.is_some_and(|current| matched_start > current.start()) {
                    break;
                }
                let matched = Match::from_validated(pattern_id, matched_start, end);
                best = select_leftmost(self.match_kind, best, matched);
            }
            if leftmost_lookahead_complete(best, end, self.max_pattern_len()) {
                return best;
            }
            index += 1;
        }
        best
    }

    fn find_anchored(&self, haystack: &[u8], start: usize, end: usize, earliest: bool) -> Option<Match> {
        self.min_pattern_len()?;
        let mut state = 0;
        let mut best = None;
        for pattern_id in self.backend.outputs(state) {
            let matched = Match::from_validated(pattern_id, start, start);
            if self.match_kind == MatchKind::Standard || earliest {
                return Some(matched);
            }
            best = select_leftmost(self.match_kind, best, matched);
        }
        let scan_end = end.min(start.saturating_add(self.max_pattern_len()));
        for (index, source_byte) in haystack.iter().copied().enumerate().take(scan_end).skip(start) {
            let byte = fold_ascii(source_byte, self.ascii_case_insensitive);
            state = self.backend.next_state(state, byte);
            let boundary = index + 1;
            for pattern_id in self.backend.outputs(state) {
                let matched_start = boundary - self.backend.pattern_length(pattern_id);
                if matched_start != start {
                    continue;
                }
                let matched = Match::from_validated(pattern_id, start, boundary);
                if self.match_kind == MatchKind::Standard || earliest {
                    return Some(matched);
                }
                best = select_leftmost(self.match_kind, best, matched);
            }
        }
        best
    }

    fn validate_input(&self, input: SearchInput<'_>) -> Result<(), MatchError> {
        if self.start_kind.supports(input.anchor_mode()) {
            Ok(())
        } else {
            Err(MatchError::UnsupportedAnchored {
                start_kind: self.start_kind,
                anchored: input.anchor_mode(),
            })
        }
    }

    /// Fill a reusable bitset with every pattern occurring in the haystack.
    pub fn scan_pattern_set(&self, haystack: &[u8], matches: &mut PatternSet) -> Result<(), MatchError> {
        self.scan_pattern_set_input(SearchInput::new(haystack), matches)
    }

    /// Fill a reusable bitset without using optional candidate acceleration.
    ///
    /// This path exists for differential validation of the accelerated scan.
    pub fn scan_pattern_set_reference(&self, haystack: &[u8], matches: &mut PatternSet) -> Result<(), MatchError> {
        self.scan_pattern_set_input_inner(SearchInput::new(haystack), matches, false)
            .map(|_| ())
    }

    /// Fill a reusable bitset and return search-local acceleration diagnostics.
    pub fn scan_pattern_set_with_diagnostics(
        &self,
        haystack: &[u8],
        matches: &mut PatternSet,
    ) -> Result<SearchDiagnostics, MatchError> {
        self.scan_pattern_set_input_with_diagnostics(SearchInput::new(haystack), matches)
    }

    /// Fill a reusable bitset with patterns occurring in one validated input.
    pub fn scan_pattern_set_input(&self, input: SearchInput<'_>, matches: &mut PatternSet) -> Result<(), MatchError> {
        self.scan_pattern_set_input_with_diagnostics(input, matches).map(|_| ())
    }

    /// Fill a reusable bitset for one input and return acceleration diagnostics.
    pub fn scan_pattern_set_input_with_diagnostics(
        &self,
        input: SearchInput<'_>,
        matches: &mut PatternSet,
    ) -> Result<SearchDiagnostics, MatchError> {
        self.scan_pattern_set_input_inner(input, matches, true)
    }

    fn scan_pattern_set_input_inner(
        &self,
        input: SearchInput<'_>,
        matches: &mut PatternSet,
        accelerated: bool,
    ) -> Result<SearchDiagnostics, MatchError> {
        self.validate_input(input)?;
        matches.validate_shape(self.pattern_count())?;
        matches.clear();
        let mut prefilter_state = SearchState::default();
        let Some(min_pattern_len) = self.min_pattern_len() else {
            return Ok(prefilter_state.diagnostics());
        };
        if input.end() - input.start() < min_pattern_len {
            return Ok(prefilter_state.diagnostics());
        }
        let mut state = 0_usize;
        for pattern_id in self.backend.outputs(state) {
            matches.insert(pattern_id)?;
        }
        let mut index = input.start();
        let end = if input.anchor_mode().is_anchored() {
            input.end().min(input.start().saturating_add(self.max_pattern_len()))
        } else {
            input.end()
        };
        while index < end {
            if accelerated && state == 0 && !input.anchor_mode().is_anchored() && !self.acceleration.is_none() {
                let candidate = self.acceleration.find_candidate(
                    &input.haystack()[..end],
                    index,
                    self.ascii_case_insensitive,
                    &mut prefilter_state,
                );
                index = match candidate {
                    Candidate::None => return Ok(prefilter_state.diagnostics()),
                    Candidate::Possible { start, .. } => start,
                    Candidate::Exact { matched, .. } => matched.start(),
                };
                if index >= end {
                    return Ok(prefilter_state.diagnostics());
                }
            }
            let byte = fold_ascii(input.haystack()[index], self.ascii_case_insensitive);
            state = self.backend.next_state(state, byte);
            for pattern_id in self.backend.outputs(state) {
                let matched_start = index + 1 - self.backend.pattern_length(pattern_id);
                if !input.anchor_mode().is_anchored() || matched_start == input.start() {
                    matches.insert(pattern_id)?;
                }
            }
            index += 1;
        }
        Ok(prefilter_state.diagnostics())
    }
}

/// Iterator over successive non-overlapping matches.
#[derive(Debug)]
pub struct FindIter<'a, 'h> {
    matcher: &'a AhoCorasick,
    input: SearchInput<'h>,
    last_match_end: Option<usize>,
    exhausted: bool,
}

impl Iterator for FindIter<'_, '_> {
    type Item = Match;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        let Some(mut matched) = self.matcher.find_from_input(self.input) else {
            self.exhausted = true;
            return None;
        };
        if matched.span().is_empty() && Some(matched.end()) == self.last_match_end {
            let Some(search_start) = self.input.start().checked_add(1) else {
                self.exhausted = true;
                return None;
            };
            self.input = self.input.with_start(search_start);
            let Some(next_match) = self.matcher.find_from_input(self.input) else {
                self.exhausted = true;
                return None;
            };
            matched = next_match;
        }
        self.input = self.input.with_start(matched.end());
        self.last_match_end = Some(matched.end());
        Some(matched)
    }
}

impl FusedIterator for FindIter<'_, '_> {}

/// Iterator over every standard match, including overlapping matches.
#[derive(Debug)]
pub struct FindOverlappingIter<'a, 'h> {
    matcher: &'a AhoCorasick,
    haystack: &'h [u8],
    state: usize,
    boundary: usize,
    end: usize,
    outputs: OutputCursor,
    prefilter_state: SearchState,
    exhausted: bool,
}

impl Iterator for FindOverlappingIter<'_, '_> {
    type Item = Match;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            return None;
        }
        loop {
            if let Some(pattern_id) = self.matcher.backend.next_output(&mut self.outputs) {
                let start = self.boundary - self.matcher.backend.pattern_length(pattern_id);
                return Some(Match::from_validated(pattern_id, start, self.boundary));
            }
            if self.boundary == self.end {
                self.exhausted = true;
                return None;
            }
            if self.state == 0 && !self.matcher.acceleration.is_none() {
                let candidate = self.matcher.acceleration.find_candidate(
                    &self.haystack[..self.end],
                    self.boundary,
                    self.matcher.ascii_case_insensitive,
                    &mut self.prefilter_state,
                );
                self.boundary = match candidate {
                    Candidate::None => {
                        self.exhausted = true;
                        return None;
                    }
                    Candidate::Possible { start, .. } => start,
                    Candidate::Exact { matched, .. } => matched.start(),
                };
                if self.boundary == self.end {
                    self.exhausted = true;
                    return None;
                }
            }
            let byte = fold_ascii(self.haystack[self.boundary], self.matcher.ascii_case_insensitive);
            self.state = self.matcher.backend.next_state(self.state, byte);
            self.boundary += 1;
            self.outputs = self.matcher.backend.output_cursor(self.state);
        }
    }
}

impl FusedIterator for FindOverlappingIter<'_, '_> {}

fn select_leftmost(match_kind: MatchKind, current: Option<Match>, candidate: Match) -> Option<Match> {
    let Some(current) = current else {
        return Some(candidate);
    };
    let replace = match match_kind {
        MatchKind::Standard => false,
        MatchKind::LeftmostFirst => {
            candidate.start() < current.start()
                || (candidate.start() == current.start() && candidate.pattern_id() < current.pattern_id())
        }
        MatchKind::LeftmostLongest => {
            candidate.start() < current.start()
                || (candidate.start() == current.start()
                    && (candidate.end() > current.end()
                        || (candidate.end() == current.end() && candidate.pattern_id() < current.pattern_id())))
        }
    };
    Some(if replace { candidate } else { current })
}

fn leftmost_lookahead_complete(best: Option<Match>, boundary: usize, max_pattern_len: usize) -> bool {
    best.is_some_and(|matched| boundary >= matched.start().saturating_add(max_pattern_len))
}

fn fold_ascii(byte: u8, ascii_case_insensitive: bool) -> u8 {
    if ascii_case_insensitive {
        byte.to_ascii_lowercase()
    } else {
        byte
    }
}

fn next_generation() -> Result<u64, BuildError> {
    NEXT_GENERATION
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1))
        .map_err(|_| BuildError::SizeOverflow {
            component: "matcher generation",
        })
}

#[cfg(test)]
#[path = "../../tests/unit/aho_corasick/mod_test.rs"]
mod tests;
