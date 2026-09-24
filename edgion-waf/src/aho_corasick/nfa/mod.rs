mod build;
mod contiguous;
mod sparse;

use super::dfa::{Dfa, DfaEstimate};
use super::prefilter::{Acceleration, AccelerationBuilder};
use super::{AutomatonKind, BuildError, BuildLimits, MatchKind, PatternId, SelectionReason, AUTO_MAX_DFA_CELLS};
pub(super) use build::BuildGraph;
use contiguous::ContiguousNfa;
use sparse::SparseNfa;
use std::mem::size_of;

pub(crate) fn phrase_build_projection(patterns: usize, bytes: usize, limits: BuildLimits) -> Option<usize> {
    if patterns == 0 {
        return Some(0);
    }
    let states = limits.max_states.min(bytes.checked_add(1)?);
    let transitions = limits.max_transitions.min(bytes);
    let outputs = limits.max_outputs.min(patterns);
    let root_fanout = transitions.min(256);
    let transition_charge = size_of::<(u8, usize)>().checked_add(4 * size_of::<usize>())?;
    states
        .checked_mul(size_of::<build::BuildState>())?
        .checked_add(transitions.checked_mul(transition_charge)?)?
        .checked_add(outputs.checked_mul(size_of::<PatternId>())?)?
        .checked_add(patterns.checked_mul(size_of::<usize>())?)?
        .checked_add(states.checked_mul(size_of::<usize>())?)?
        .checked_add(root_fanout.checked_mul(size_of::<(u8, usize)>())?)?
        .checked_add(limits.max_compiled_bytes)
}

#[derive(Debug)]
pub(super) enum Backend {
    Sparse(SparseNfa),
    Contiguous(ContiguousNfa),
    Dfa(Dfa),
}

impl Backend {
    pub(super) const fn automaton_kind(&self) -> AutomatonKind {
        match self {
            Self::Sparse(_) => AutomatonKind::SparseNfa,
            Self::Contiguous(_) => AutomatonKind::ContiguousNfa,
            Self::Dfa(_) => AutomatonKind::Dfa,
        }
    }

    pub(super) fn build_sparse<I, P>(
        patterns: I,
        limits: BuildLimits,
        ascii_case_insensitive: bool,
        match_kind: MatchKind,
        prefilter: bool,
    ) -> Result<(Self, Acceleration), BuildError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        let (graph, acceleration, backend_limits) =
            prepare(patterns, limits, ascii_case_insensitive, match_kind, prefilter)?;
        let acceleration_bytes = acceleration.memory_usage();
        let backend = SparseNfa::from_graph(graph, backend_limits.max_compiled_bytes)
            .map(Self::Sparse)
            .map_err(|error| restore_compiled_limit(error, acceleration_bytes, limits.max_compiled_bytes))?;
        Ok((backend, acceleration))
    }

    pub(super) fn build_contiguous<I, P>(
        patterns: I,
        limits: BuildLimits,
        ascii_case_insensitive: bool,
        dense_depth: usize,
        match_kind: MatchKind,
        prefilter: bool,
    ) -> Result<(Self, Acceleration), BuildError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        let (graph, acceleration, backend_limits) =
            prepare(patterns, limits, ascii_case_insensitive, match_kind, prefilter)?;
        let acceleration_bytes = acceleration.memory_usage();
        let backend = ContiguousNfa::from_graph(graph, backend_limits.max_compiled_bytes, dense_depth)
            .map(Self::Contiguous)
            .map_err(|error| restore_compiled_limit(error, acceleration_bytes, limits.max_compiled_bytes))?;
        Ok((backend, acceleration))
    }

    pub(super) fn build_dfa<I, P>(
        patterns: I,
        limits: BuildLimits,
        ascii_case_insensitive: bool,
        match_kind: MatchKind,
        prefilter: bool,
    ) -> Result<(Self, Acceleration), BuildError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        let (graph, acceleration, backend_limits) =
            prepare(patterns, limits, ascii_case_insensitive, match_kind, prefilter)?;
        let acceleration_bytes = acceleration.memory_usage();
        let backend = Dfa::from_graph(graph, backend_limits)
            .map(Self::Dfa)
            .map_err(|error| restore_compiled_limit(error, acceleration_bytes, limits.max_compiled_bytes))?;
        Ok((backend, acceleration))
    }

    pub(super) fn build_auto<I, P>(
        patterns: I,
        limits: BuildLimits,
        ascii_case_insensitive: bool,
        dense_depth: usize,
        match_kind: MatchKind,
        prefilter: bool,
    ) -> Result<(Self, Acceleration, SelectionReason), BuildError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        let (graph, acceleration, backend_limits) =
            prepare(patterns, limits, ascii_case_insensitive, match_kind, prefilter)?;
        let acceleration_bytes = acceleration.memory_usage();
        let estimate = DfaEstimate::from_graph(&graph)?;
        let dfa_fits = estimate.cells <= AUTO_MAX_DFA_CELLS
            && estimate.cells <= backend_limits.max_dfa_cells
            && estimate.memory_usage <= backend_limits.max_compiled_bytes;
        if dfa_fits {
            let backend = Dfa::from_graph(graph, backend_limits)
                .map(Self::Dfa)
                .map_err(|error| restore_compiled_limit(error, acceleration_bytes, limits.max_compiled_bytes))?;
            return Ok((backend, acceleration, SelectionReason::AutoDfaWithinPolicy));
        }
        if graph.root_fanout() >= contiguous::MIN_DENSE_FANOUT {
            let backend = ContiguousNfa::from_graph(graph, backend_limits.max_compiled_bytes, dense_depth)
                .map(Self::Contiguous)
                .map_err(|error| restore_compiled_limit(error, acceleration_bytes, limits.max_compiled_bytes))?;
            return Ok((backend, acceleration, SelectionReason::AutoContiguousDfaTooLarge));
        }
        let backend = SparseNfa::from_graph(graph, backend_limits.max_compiled_bytes)
            .map(Self::Sparse)
            .map_err(|error| restore_compiled_limit(error, acceleration_bytes, limits.max_compiled_bytes))?;
        Ok((backend, acceleration, SelectionReason::AutoSparseDfaTooLarge))
    }

    pub(super) fn pattern_count(&self) -> usize {
        match self {
            Self::Sparse(nfa) => nfa.pattern_count(),
            Self::Contiguous(nfa) => nfa.pattern_count(),
            Self::Dfa(dfa) => dfa.pattern_count(),
        }
    }

    pub(super) fn min_pattern_len(&self) -> Option<usize> {
        match self {
            Self::Sparse(nfa) => nfa.min_pattern_len(),
            Self::Contiguous(nfa) => nfa.min_pattern_len(),
            Self::Dfa(dfa) => dfa.min_pattern_len(),
        }
    }

    pub(super) fn max_pattern_len(&self) -> usize {
        match self {
            Self::Sparse(nfa) => nfa.max_pattern_len(),
            Self::Contiguous(nfa) => nfa.max_pattern_len(),
            Self::Dfa(dfa) => dfa.max_pattern_len(),
        }
    }

    pub(super) fn memory_usage(&self) -> usize {
        match self {
            Self::Sparse(nfa) => nfa.memory_usage(),
            Self::Contiguous(nfa) => nfa.memory_usage(),
            Self::Dfa(dfa) => dfa.memory_usage(),
        }
    }

    pub(super) fn estimated_peak_build_memory_usage(&self) -> usize {
        match self {
            Self::Sparse(nfa) => nfa.estimated_peak_build_memory_usage(),
            Self::Contiguous(nfa) => nfa.estimated_peak_build_memory_usage(),
            Self::Dfa(dfa) => dfa.estimated_peak_build_memory_usage(),
        }
    }

    pub(super) fn next_state(&self, state: usize, byte: u8) -> usize {
        match self {
            Self::Sparse(nfa) => nfa.next_state(state, byte),
            Self::Contiguous(nfa) => nfa.next_state(state, byte),
            Self::Dfa(dfa) => dfa.next_state(state, byte),
        }
    }

    pub(super) fn outputs(&self, state: usize) -> OutputIter<'_> {
        OutputIter {
            backend: self,
            cursor: self.output_cursor(state),
        }
    }

    pub(super) const fn output_cursor(&self, state: usize) -> OutputCursor {
        OutputCursor {
            state: Some(state),
            output_index: 0,
        }
    }

    pub(super) fn next_output(&self, cursor: &mut OutputCursor) -> Option<PatternId> {
        match self {
            Self::Sparse(nfa) => nfa.next_output(cursor),
            Self::Contiguous(nfa) => nfa.next_output(cursor),
            Self::Dfa(dfa) => dfa.next_output(cursor),
        }
    }

    pub(super) fn pattern_length(&self, pattern_id: PatternId) -> usize {
        match self {
            Self::Sparse(nfa) => nfa.pattern_length(pattern_id),
            Self::Contiguous(nfa) => nfa.pattern_length(pattern_id),
            Self::Dfa(dfa) => dfa.pattern_length(pattern_id),
        }
    }

    pub(super) fn dense_state_count(&self) -> usize {
        match self {
            Self::Sparse(nfa) => nfa.dense_state_count(),
            Self::Contiguous(nfa) => nfa.dense_state_count(),
            Self::Dfa(_) => 0,
        }
    }

    pub(super) fn state_count(&self) -> usize {
        match self {
            Self::Sparse(nfa) => nfa.state_count(),
            Self::Contiguous(nfa) => nfa.state_count(),
            Self::Dfa(dfa) => dfa.state_count(),
        }
    }

    pub(super) fn byte_class_count(&self) -> usize {
        match self {
            Self::Sparse(_) | Self::Contiguous(_) => 256,
            Self::Dfa(dfa) => dfa.byte_class_count(),
        }
    }
}

fn prepare<I, P>(
    patterns: I,
    limits: BuildLimits,
    ascii_case_insensitive: bool,
    match_kind: MatchKind,
    prefilter: bool,
) -> Result<(BuildGraph, Acceleration, BuildLimits), BuildError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<[u8]>,
{
    let mut acceleration_builder = AccelerationBuilder::new(prefilter, ascii_case_insensitive, match_kind);
    let graph = BuildGraph::build_profiled(
        patterns,
        limits,
        ascii_case_insensitive,
        Some(&mut acceleration_builder),
    )?;
    let acceleration = acceleration_builder.build()?;
    let Some(remaining) = limits.max_compiled_bytes.checked_sub(acceleration.memory_usage()) else {
        return Err(BuildError::CompiledByteLimit {
            actual: acceleration.memory_usage(),
            limit: limits.max_compiled_bytes,
        });
    };
    let backend_limits = BuildLimits {
        max_compiled_bytes: remaining,
        ..limits
    };
    Ok((graph, acceleration, backend_limits))
}

fn restore_compiled_limit(error: BuildError, acceleration_bytes: usize, original_limit: usize) -> BuildError {
    match error {
        BuildError::CompiledByteLimit { actual, .. } => BuildError::CompiledByteLimit {
            actual: actual.saturating_add(acceleration_bytes),
            limit: original_limit,
        },
        error => error,
    }
}

pub(super) struct OutputIter<'a> {
    backend: &'a Backend,
    cursor: OutputCursor,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OutputCursor {
    pub(super) state: Option<usize>,
    pub(super) output_index: usize,
}

impl Iterator for OutputIter<'_> {
    type Item = PatternId;

    fn next(&mut self) -> Option<Self::Item> {
        self.backend.next_output(&mut self.cursor)
    }
}

#[cfg(test)]
#[path = "../../../tests/support/aho_corasick/nfa/mod.rs"]
mod test_support;
