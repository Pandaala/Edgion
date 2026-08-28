use std::mem::size_of;

use super::super::{BuildError, PatternId};
use super::build::BuildGraph;
use super::OutputCursor;

const NO_STATE: u32 = u32::MAX;
const ALPHABET_LEN: usize = u8::MAX as usize + 1;
pub(super) const MIN_DENSE_FANOUT: usize = 16;

#[derive(Debug, Clone, Copy)]
struct FlatTransition {
    byte: u8,
    state: u32,
}

#[derive(Debug, Clone, Copy)]
struct FlatState {
    sparse_start: u32,
    sparse_len: u32,
    output_start: u32,
    output_len: u32,
    failure: u32,
    output_link: u32,
    dense_start: u32,
}

/// Immutable NFA with state metadata, transitions, and outputs in flat arrays.
#[derive(Debug)]
pub(crate) struct ContiguousNfa {
    states: Vec<FlatState>,
    sparse_transitions: Vec<FlatTransition>,
    dense_transitions: Vec<u32>,
    outputs: Vec<PatternId>,
    pattern_lengths: Vec<usize>,
    min_pattern_len: Option<usize>,
    max_pattern_len: usize,
    memory_usage: usize,
    estimated_peak_build_memory_usage: usize,
    dense_state_count: usize,
}

impl ContiguousNfa {
    pub(super) fn from_graph(
        graph: BuildGraph,
        max_compiled_bytes: usize,
        dense_depth: usize,
    ) -> Result<Self, BuildError> {
        let build_graph_bytes = graph.estimated_memory_usage();
        let dense_state_count = graph
            .states
            .iter()
            .filter(|state| should_use_dense(state.depth, state.transitions.len(), dense_depth))
            .count();
        let dense_transition_count = dense_state_count
            .checked_mul(ALPHABET_LEN)
            .ok_or(BuildError::SizeOverflow {
                component: "contiguous dense transition count",
            })?;
        let sparse_transition_count = graph.states.iter().try_fold(0_usize, |count, state| {
            if should_use_dense(state.depth, state.transitions.len(), dense_depth) {
                return Some(count);
            }
            count.checked_add(state.transitions.len())
        });
        let Some(sparse_transition_count) = sparse_transition_count else {
            return Err(BuildError::SizeOverflow {
                component: "contiguous sparse transition count",
            });
        };
        let estimated_bytes = estimate_bytes(
            graph.states.len(),
            sparse_transition_count,
            dense_transition_count,
            graph.output_count,
            graph.pattern_lengths.len(),
        )?;
        check_compiled_byte_limit(estimated_bytes, max_compiled_bytes)?;

        let mut states = Vec::new();
        states
            .try_reserve_exact(graph.states.len())
            .map_err(|_| BuildError::Allocation {
                component: "contiguous states",
            })?;
        let mut sparse_transitions = Vec::new();
        sparse_transitions
            .try_reserve_exact(sparse_transition_count)
            .map_err(|_| BuildError::Allocation {
                component: "contiguous sparse transitions",
            })?;
        let mut dense_transitions = Vec::new();
        dense_transitions
            .try_reserve_exact(dense_transition_count)
            .map_err(|_| BuildError::Allocation {
                component: "contiguous dense transitions",
            })?;
        let mut outputs = Vec::new();
        outputs
            .try_reserve_exact(graph.output_count)
            .map_err(|_| BuildError::Allocation {
                component: "contiguous outputs",
            })?;

        for state in graph.states {
            let sparse_start = checked_u32(sparse_transitions.len(), "contiguous sparse transition start")?;
            let use_dense = should_use_dense(state.depth, state.transitions.len(), dense_depth);
            let dense_start = if use_dense {
                let start = checked_u32(dense_transitions.len(), "contiguous dense transition start")?;
                let end = dense_transitions
                    .len()
                    .checked_add(ALPHABET_LEN)
                    .ok_or(BuildError::SizeOverflow {
                        component: "contiguous dense transition end",
                    })?;
                checked_u32(end - 1, "contiguous dense transition final index")?;
                dense_transitions.resize(end, NO_STATE);
                for (byte, next) in state.transitions {
                    dense_transitions[start as usize + byte as usize] =
                        checked_u32(next, "contiguous dense transition state")?;
                }
                start
            } else {
                for (byte, next) in state.transitions {
                    sparse_transitions.push(FlatTransition {
                        byte,
                        state: checked_u32(next, "contiguous transition state")?,
                    });
                }
                NO_STATE
            };
            let sparse_len = checked_count_u32(
                sparse_transitions.len() - sparse_start as usize,
                "contiguous sparse transition length",
            )?;
            let output_start = checked_u32(outputs.len(), "contiguous output start")?;
            outputs.extend(state.outputs);
            let output_len = checked_count_u32(outputs.len() - output_start as usize, "contiguous output length")?;
            states.push(FlatState {
                sparse_start,
                sparse_len,
                output_start,
                output_len,
                failure: checked_u32(state.failure, "contiguous failure state")?,
                output_link: match state.output_link {
                    Some(link) => checked_u32(link, "contiguous output-link state")?,
                    None => NO_STATE,
                },
                dense_start,
            });
        }

        let mut pattern_lengths = Vec::new();
        pattern_lengths
            .try_reserve_exact(graph.pattern_lengths.len())
            .map_err(|_| BuildError::Allocation {
                component: "contiguous pattern lengths",
            })?;
        pattern_lengths.extend(graph.pattern_lengths);
        let memory_usage = allocated_bytes(
            states.capacity(),
            sparse_transitions.capacity(),
            dense_transitions.capacity(),
            outputs.capacity(),
            pattern_lengths.capacity(),
        );
        check_compiled_byte_limit(memory_usage, max_compiled_bytes)?;
        let estimated_peak_build_memory_usage = build_graph_bytes.saturating_add(memory_usage);

        Ok(Self {
            states,
            sparse_transitions,
            dense_transitions,
            outputs,
            pattern_lengths,
            min_pattern_len: graph.min_pattern_len,
            max_pattern_len: graph.max_pattern_len,
            memory_usage,
            estimated_peak_build_memory_usage,
            dense_state_count,
        })
    }

    pub(super) fn pattern_count(&self) -> usize {
        self.pattern_lengths.len()
    }

    pub(super) fn state_count(&self) -> usize {
        self.states.len()
    }

    pub(super) fn min_pattern_len(&self) -> Option<usize> {
        self.min_pattern_len
    }

    pub(super) fn max_pattern_len(&self) -> usize {
        self.max_pattern_len
    }

    pub(super) fn memory_usage(&self) -> usize {
        self.memory_usage
    }

    pub(super) fn estimated_peak_build_memory_usage(&self) -> usize {
        self.estimated_peak_build_memory_usage
    }

    pub(super) fn dense_state_count(&self) -> usize {
        self.dense_state_count
    }

    pub(super) fn next_state(&self, mut state: usize, byte: u8) -> usize {
        while state != 0 {
            if let Some(next) = self.direct_transition(state, byte) {
                return next;
            }
            state = self.states[state].failure as usize;
        }
        self.direct_transition(0, byte).unwrap_or(0)
    }

    fn direct_transition(&self, state: usize, byte: u8) -> Option<usize> {
        let state = &self.states[state];
        if state.dense_start != NO_STATE {
            let index = state.dense_start as usize + byte as usize;
            let next = *self.dense_transitions.get(index)?;
            return (next != NO_STATE).then_some(next as usize);
        }
        let start = state.sparse_start as usize;
        let end = start + state.sparse_len as usize;
        self.sparse_transitions[start..end]
            .binary_search_by_key(&byte, |transition| transition.byte)
            .ok()
            .map(|index| self.sparse_transitions[start + index].state as usize)
    }

    pub(super) fn next_output(&self, cursor: &mut OutputCursor) -> Option<PatternId> {
        loop {
            let state_id = cursor.state?;
            let state = &self.states[state_id];
            if cursor.output_index < state.output_len as usize {
                let index = state.output_start as usize + cursor.output_index;
                cursor.output_index += 1;
                return self.outputs.get(index).copied();
            }
            cursor.state = (state.output_link != NO_STATE).then_some(state.output_link as usize);
            cursor.output_index = 0;
        }
    }

    pub(super) fn pattern_length(&self, pattern_id: PatternId) -> usize {
        self.pattern_lengths[pattern_id.as_usize()]
    }

    #[cfg(test)]
    pub(super) fn stored_output_count(&self) -> usize {
        self.outputs.len()
    }
}

fn checked_u32(value: usize, component: &'static str) -> Result<u32, BuildError> {
    if value >= u32::MAX as usize {
        return Err(BuildError::IndexOverflow { component, value });
    }
    Ok(value as u32)
}

fn checked_count_u32(value: usize, component: &'static str) -> Result<u32, BuildError> {
    u32::try_from(value).map_err(|_| BuildError::IndexOverflow { component, value })
}

fn should_use_dense(depth: usize, fanout: usize, dense_depth: usize) -> bool {
    depth < dense_depth && fanout >= MIN_DENSE_FANOUT
}

fn estimate_bytes(
    states: usize,
    sparse_transitions: usize,
    dense_transitions: usize,
    outputs: usize,
    pattern_lengths: usize,
) -> Result<usize, BuildError> {
    states
        .checked_mul(size_of::<FlatState>())
        .and_then(|bytes| bytes.checked_add(sparse_transitions.checked_mul(size_of::<FlatTransition>())?))
        .and_then(|bytes| bytes.checked_add(dense_transitions.checked_mul(size_of::<u32>())?))
        .and_then(|bytes| bytes.checked_add(outputs.checked_mul(size_of::<PatternId>())?))
        .and_then(|bytes| bytes.checked_add(pattern_lengths.checked_mul(size_of::<usize>())?))
        .ok_or(BuildError::SizeOverflow {
            component: "contiguous compiled bytes",
        })
}

fn allocated_bytes(
    states: usize,
    sparse_transitions: usize,
    dense_transitions: usize,
    outputs: usize,
    pattern_lengths: usize,
) -> usize {
    states
        .saturating_mul(size_of::<FlatState>())
        .saturating_add(sparse_transitions.saturating_mul(size_of::<FlatTransition>()))
        .saturating_add(dense_transitions.saturating_mul(size_of::<u32>()))
        .saturating_add(outputs.saturating_mul(size_of::<PatternId>()))
        .saturating_add(pattern_lengths.saturating_mul(size_of::<usize>()))
}

fn check_compiled_byte_limit(actual: usize, limit: usize) -> Result<(), BuildError> {
    if actual > limit {
        return Err(BuildError::CompiledByteLimit { actual, limit });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aho_corasick::BuildLimits;

    #[test]
    fn checked_index_conversion_rejects_values_above_u32() {
        let sentinel = u32::MAX as usize;
        assert_eq!(
            checked_u32(sentinel, "test index").unwrap_err(),
            BuildError::IndexOverflow {
                component: "test index",
                value: sentinel,
            }
        );
        if let Some(value) = (u32::MAX as usize).checked_add(1) {
            assert_eq!(
                checked_u32(value, "test index").unwrap_err(),
                BuildError::IndexOverflow {
                    component: "test index",
                    value,
                }
            );
        }
    }

    #[test]
    fn reported_memory_matches_owned_vector_capacities() {
        let limits = BuildLimits::default();
        let graph = BuildGraph::build(
            [b"alpha".as_slice(), b"beta".as_slice(), b"alphabet".as_slice()],
            limits,
            false,
        )
        .unwrap();
        let nfa = ContiguousNfa::from_graph(graph, limits.max_compiled_bytes, 2).unwrap();

        assert_eq!(
            nfa.memory_usage,
            allocated_bytes(
                nfa.states.capacity(),
                nfa.sparse_transitions.capacity(),
                nfa.dense_transitions.capacity(),
                nfa.outputs.capacity(),
                nfa.pattern_lengths.capacity(),
            )
        );
    }
}
