use std::mem::size_of;

use super::super::{BuildError, PatternId};
use super::build::BuildGraph;
use super::contiguous::MIN_DENSE_FANOUT;
use super::OutputCursor;

const ALPHABET_LEN: usize = u8::MAX as usize + 1;

#[derive(Debug, Clone, Copy)]
struct Transition {
    byte: u8,
    state: usize,
}

#[derive(Debug)]
struct SparseState {
    transitions: Vec<Transition>,
    failure: usize,
    output_link: Option<usize>,
    outputs: Vec<PatternId>,
}

impl SparseState {
    fn transition(&self, byte: u8) -> Option<usize> {
        self.transitions
            .binary_search_by_key(&byte, |transition| transition.byte)
            .ok()
            .and_then(|index| self.transitions.get(index).map(|transition| transition.state))
    }
}

/// Immutable noncontiguous sparse Aho-Corasick NFA.
#[derive(Debug)]
pub(crate) struct SparseNfa {
    states: Vec<SparseState>,
    dense_root: Vec<usize>,
    pattern_lengths: Vec<usize>,
    min_pattern_len: Option<usize>,
    max_pattern_len: usize,
    memory_usage: usize,
    estimated_peak_build_memory_usage: usize,
}

impl SparseNfa {
    pub(super) fn from_graph(graph: BuildGraph, max_compiled_bytes: usize) -> Result<Self, BuildError> {
        let build_graph_bytes = graph.estimated_memory_usage();
        let root_fanout = graph.states.first().map_or(0, |state| state.transitions.len());
        let use_dense_root = root_fanout >= MIN_DENSE_FANOUT;
        let sparse_transition_count = graph
            .transition_count
            .checked_sub(if use_dense_root { root_fanout } else { 0 })
            .ok_or(BuildError::SizeOverflow {
                component: "compiled sparse transition count",
            })?;
        let dense_root_len = if use_dense_root { ALPHABET_LEN } else { 0 };
        let estimated_bytes = estimate_compiled_bytes(
            graph.states.len(),
            sparse_transition_count,
            dense_root_len,
            graph.output_count,
            graph.pattern_lengths.len(),
        )?;
        check_compiled_byte_limit(estimated_bytes, max_compiled_bytes)?;

        let mut dense_root = Vec::new();
        if use_dense_root {
            dense_root
                .try_reserve_exact(ALPHABET_LEN)
                .map_err(|_| BuildError::Allocation {
                    component: "compiled sparse dense root",
                })?;
            dense_root.resize(ALPHABET_LEN, 0);
            let root = graph
                .states
                .first()
                .expect("a validated build graph always contains the root state");
            for (byte, state) in &root.transitions {
                dense_root[usize::from(*byte)] = *state;
            }
        }

        let mut compiled_states = Vec::new();
        compiled_states
            .try_reserve_exact(graph.states.len())
            .map_err(|_| BuildError::Allocation {
                component: "compiled sparse states",
            })?;
        for (state_id, state) in graph.states.into_iter().enumerate() {
            let mut transitions = Vec::new();
            let transition_count = if use_dense_root && state_id == 0 {
                0
            } else {
                state.transitions.len()
            };
            transitions
                .try_reserve_exact(transition_count)
                .map_err(|_| BuildError::Allocation {
                    component: "compiled sparse transitions",
                })?;
            if transition_count != 0 {
                transitions.extend(
                    state
                        .transitions
                        .into_iter()
                        .map(|(byte, state)| Transition { byte, state }),
                );
            }
            let mut outputs = Vec::new();
            outputs
                .try_reserve_exact(state.outputs.len())
                .map_err(|_| BuildError::Allocation {
                    component: "compiled direct outputs",
                })?;
            outputs.extend(state.outputs);
            compiled_states.push(SparseState {
                transitions,
                failure: state.failure,
                output_link: state.output_link,
                outputs,
            });
        }

        let mut compiled_pattern_lengths = Vec::new();
        compiled_pattern_lengths
            .try_reserve_exact(graph.pattern_lengths.len())
            .map_err(|_| BuildError::Allocation {
                component: "compiled pattern lengths",
            })?;
        compiled_pattern_lengths.extend(graph.pattern_lengths);
        let memory_usage = allocated_bytes(
            &compiled_states,
            compiled_states.capacity(),
            dense_root.capacity(),
            compiled_pattern_lengths.capacity(),
        );
        check_compiled_byte_limit(memory_usage, max_compiled_bytes)?;
        let estimated_peak_build_memory_usage = build_graph_bytes.saturating_add(memory_usage);
        Ok(Self {
            states: compiled_states,
            dense_root,
            pattern_lengths: compiled_pattern_lengths,
            min_pattern_len: graph.min_pattern_len,
            max_pattern_len: graph.max_pattern_len,
            memory_usage,
            estimated_peak_build_memory_usage,
        })
    }

    pub(crate) fn pattern_count(&self) -> usize {
        self.pattern_lengths.len()
    }

    pub(crate) fn state_count(&self) -> usize {
        self.states.len()
    }

    pub(crate) fn min_pattern_len(&self) -> Option<usize> {
        self.min_pattern_len
    }

    pub(crate) fn max_pattern_len(&self) -> usize {
        self.max_pattern_len
    }

    pub(crate) fn memory_usage(&self) -> usize {
        self.memory_usage
    }

    pub(crate) fn estimated_peak_build_memory_usage(&self) -> usize {
        self.estimated_peak_build_memory_usage
    }

    pub(crate) fn next_state(&self, mut state: usize, byte: u8) -> usize {
        while state != 0 {
            if let Some(next) = self.states[state].transition(byte) {
                return next;
            }
            state = self.states[state].failure;
        }
        if self.dense_root.is_empty() {
            self.states[0].transition(byte).unwrap_or(0)
        } else {
            self.dense_root[usize::from(byte)]
        }
    }

    pub(crate) fn next_output(&self, cursor: &mut OutputCursor) -> Option<PatternId> {
        loop {
            let state_id = cursor.state?;
            let state = &self.states[state_id];
            if let Some(pattern_id) = state.outputs.get(cursor.output_index).copied() {
                cursor.output_index += 1;
                return Some(pattern_id);
            }
            cursor.state = state.output_link;
            cursor.output_index = 0;
        }
    }

    pub(crate) fn pattern_length(&self, pattern_id: PatternId) -> usize {
        self.pattern_lengths[pattern_id.as_usize()]
    }

    pub(crate) fn dense_state_count(&self) -> usize {
        usize::from(!self.dense_root.is_empty())
    }
}

fn estimate_compiled_bytes(
    states: usize,
    transitions: usize,
    dense_root: usize,
    outputs: usize,
    pattern_lengths: usize,
) -> Result<usize, BuildError> {
    let state_bytes = states
        .checked_mul(size_of::<SparseState>())
        .ok_or(BuildError::SizeOverflow {
            component: "compiled state bytes",
        })?;
    let transition_bytes = transitions
        .checked_mul(size_of::<Transition>())
        .ok_or(BuildError::SizeOverflow {
            component: "compiled transition bytes",
        })?;
    let dense_root_bytes = dense_root
        .checked_mul(size_of::<usize>())
        .ok_or(BuildError::SizeOverflow {
            component: "compiled dense root bytes",
        })?;
    let output_bytes = outputs
        .checked_mul(size_of::<PatternId>())
        .ok_or(BuildError::SizeOverflow {
            component: "compiled output bytes",
        })?;
    let pattern_length_bytes = pattern_lengths
        .checked_mul(size_of::<usize>())
        .ok_or(BuildError::SizeOverflow {
            component: "pattern length bytes",
        })?;
    state_bytes
        .checked_add(transition_bytes)
        .and_then(|total| total.checked_add(dense_root_bytes))
        .and_then(|total| total.checked_add(output_bytes))
        .and_then(|total| total.checked_add(pattern_length_bytes))
        .ok_or(BuildError::SizeOverflow {
            component: "total compiled bytes",
        })
}

fn check_compiled_byte_limit(actual: usize, limit: usize) -> Result<(), BuildError> {
    if actual > limit {
        return Err(BuildError::CompiledByteLimit { actual, limit });
    }
    Ok(())
}

fn allocated_bytes(
    states: &[SparseState],
    state_capacity: usize,
    dense_root_capacity: usize,
    pattern_length_capacity: usize,
) -> usize {
    let state_bytes = state_capacity.saturating_mul(size_of::<SparseState>());
    let transition_bytes = states.iter().fold(0_usize, |total, state| {
        total.saturating_add(state.transitions.capacity().saturating_mul(size_of::<Transition>()))
    });
    let output_bytes = states.iter().fold(0_usize, |total, state| {
        total.saturating_add(state.outputs.capacity().saturating_mul(size_of::<PatternId>()))
    });
    state_bytes
        .saturating_add(transition_bytes)
        .saturating_add(dense_root_capacity.saturating_mul(size_of::<usize>()))
        .saturating_add(output_bytes)
        .saturating_add(pattern_length_capacity.saturating_mul(size_of::<usize>()))
}

#[cfg(test)]
#[path = "../../../tests/unit/aho_corasick/nfa/sparse_test.rs"]
mod tests;
