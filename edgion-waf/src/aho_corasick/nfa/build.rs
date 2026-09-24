use std::collections::{BTreeMap, VecDeque};
use std::mem::size_of;

use super::super::{BuildError, BuildLimits, PatternId};
use crate::aho_corasick::prefilter::AccelerationBuilder;

#[derive(Debug)]
pub(crate) struct BuildState {
    pub(crate) transitions: BTreeMap<u8, usize>,
    pub(crate) failure: usize,
    pub(crate) output_link: Option<usize>,
    pub(crate) outputs: Vec<PatternId>,
    pub(crate) depth: usize,
}

impl BuildState {
    fn new(depth: usize) -> Self {
        Self {
            transitions: BTreeMap::new(),
            failure: 0,
            output_link: None,
            outputs: Vec::new(),
            depth,
        }
    }
}

/// Backend-neutral mutable graph used only while compiling one automaton.
#[derive(Debug)]
pub(crate) struct BuildGraph {
    pub(crate) states: Vec<BuildState>,
    pub(crate) pattern_lengths: Vec<usize>,
    pub(crate) min_pattern_len: Option<usize>,
    pub(crate) max_pattern_len: usize,
    pub(crate) transition_count: usize,
    pub(crate) output_count: usize,
}

impl BuildGraph {
    pub(crate) fn build_profiled<I, P>(
        patterns: I,
        limits: BuildLimits,
        ascii_case_insensitive: bool,
        mut acceleration: Option<&mut AccelerationBuilder>,
    ) -> Result<Self, BuildError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        if limits.max_states < 1 {
            return Err(BuildError::StateLimit {
                actual: 1,
                limit: limits.max_states,
            });
        }
        check_minimum_compiled_bytes(1, 0, 0, 0, limits.max_compiled_bytes)?;

        let mut states = Vec::new();
        states.try_reserve_exact(1).map_err(|_| BuildError::Allocation {
            component: "root build state",
        })?;
        states.push(BuildState::new(0));

        let mut pattern_lengths = Vec::new();
        let mut pattern_bytes = 0_usize;
        let mut transition_count = 0_usize;
        let mut output_count = 0_usize;
        let mut min_pattern_len: Option<usize> = None;
        let mut max_pattern_len = 0_usize;

        for (index, pattern) in patterns.into_iter().enumerate() {
            let actual_patterns = index.checked_add(1).ok_or(BuildError::SizeOverflow {
                component: "pattern count",
            })?;
            if actual_patterns > limits.max_patterns {
                return Err(BuildError::PatternLimit {
                    actual: actual_patterns,
                    limit: limits.max_patterns,
                });
            }
            let pattern_id = PatternId::try_from_usize(index)?;
            let pattern = pattern.as_ref();

            pattern_bytes = pattern_bytes
                .checked_add(pattern.len())
                .ok_or(BuildError::SizeOverflow {
                    component: "combined pattern bytes",
                })?;
            if pattern_bytes > limits.max_pattern_bytes {
                return Err(BuildError::PatternByteLimit {
                    actual: pattern_bytes,
                    limit: limits.max_pattern_bytes,
                });
            }

            output_count = output_count.checked_add(1).ok_or(BuildError::SizeOverflow {
                component: "direct output count",
            })?;
            if output_count > limits.max_outputs {
                return Err(BuildError::OutputLimit {
                    actual: output_count,
                    limit: limits.max_outputs,
                });
            }
            check_minimum_compiled_bytes(
                states.len(),
                transition_count,
                output_count,
                actual_patterns,
                limits.max_compiled_bytes,
            )?;

            pattern_lengths.try_reserve(1).map_err(|_| BuildError::Allocation {
                component: "build pattern lengths",
            })?;
            pattern_lengths.push(pattern.len());
            min_pattern_len = Some(min_pattern_len.map_or(pattern.len(), |length| length.min(pattern.len())));
            max_pattern_len = max_pattern_len.max(pattern.len());
            if let Some(builder) = acceleration.as_deref_mut() {
                builder.add(pattern_id, pattern)?;
            }

            let mut state = 0_usize;
            for source_byte in pattern.iter().copied() {
                let byte = if ascii_case_insensitive {
                    source_byte.to_ascii_lowercase()
                } else {
                    source_byte
                };
                let next = match states[state].transitions.get(&byte).copied() {
                    Some(next) => next,
                    None => {
                        let actual_states = states.len().checked_add(1).ok_or(BuildError::SizeOverflow {
                            component: "state count",
                        })?;
                        if actual_states > limits.max_states {
                            return Err(BuildError::StateLimit {
                                actual: actual_states,
                                limit: limits.max_states,
                            });
                        }
                        let actual_transitions = transition_count.checked_add(1).ok_or(BuildError::SizeOverflow {
                            component: "transition count",
                        })?;
                        if actual_transitions > limits.max_transitions {
                            return Err(BuildError::TransitionLimit {
                                actual: actual_transitions,
                                limit: limits.max_transitions,
                            });
                        }
                        check_minimum_compiled_bytes(
                            actual_states,
                            actual_transitions,
                            output_count,
                            pattern_lengths.len(),
                            limits.max_compiled_bytes,
                        )?;
                        let depth = states[state].depth.checked_add(1).ok_or(BuildError::SizeOverflow {
                            component: "state depth",
                        })?;
                        states.try_reserve(1).map_err(|_| BuildError::Allocation {
                            component: "trie build states",
                        })?;
                        let next = states.len();
                        states.push(BuildState::new(depth));
                        states[state].transitions.insert(byte, next);
                        transition_count = actual_transitions;
                        next
                    }
                };
                state = next;
            }
            states[state]
                .outputs
                .try_reserve(1)
                .map_err(|_| BuildError::Allocation {
                    component: "direct build outputs",
                })?;
            states[state].outputs.push(pattern_id);
        }

        build_failure_links(&mut states)?;
        Ok(Self {
            states,
            pattern_lengths,
            min_pattern_len,
            max_pattern_len,
            transition_count,
            output_count,
        })
    }

    pub(crate) fn estimated_memory_usage(&self) -> usize {
        let state_bytes = self.states.capacity().saturating_mul(size_of::<BuildState>());
        let transition_node_bytes = self
            .transition_count
            .saturating_mul(size_of::<(u8, usize)>().saturating_add(4_usize.saturating_mul(size_of::<usize>())));
        let output_bytes = self.states.iter().fold(0_usize, |total, state| {
            total.saturating_add(state.outputs.capacity().saturating_mul(size_of::<PatternId>()))
        });
        let pattern_length_bytes = self.pattern_lengths.capacity().saturating_mul(size_of::<usize>());
        let failure_queue_bytes = self.states.len().saturating_mul(size_of::<usize>());
        let max_fanout = self
            .states
            .iter()
            .map(|state| state.transitions.len())
            .max()
            .unwrap_or(0);
        let edge_snapshot_bytes = max_fanout.saturating_mul(size_of::<(u8, usize)>());
        state_bytes
            .saturating_add(transition_node_bytes)
            .saturating_add(output_bytes)
            .saturating_add(pattern_length_bytes)
            .saturating_add(failure_queue_bytes)
            .saturating_add(edge_snapshot_bytes)
    }

    pub(crate) fn next_state(&self, mut state: usize, byte: u8) -> usize {
        while state != 0 {
            if let Some(next) = self.states[state].transitions.get(&byte).copied() {
                return next;
            }
            state = self.states[state].failure;
        }
        self.states[0].transitions.get(&byte).copied().unwrap_or(0)
    }

    pub(crate) fn root_fanout(&self) -> usize {
        self.states[0].transitions.len()
    }
}

fn build_failure_links(states: &mut [BuildState]) -> Result<(), BuildError> {
    let mut queue = VecDeque::new();
    queue.try_reserve(states.len()).map_err(|_| BuildError::Allocation {
        component: "failure-link queue",
    })?;
    queue.extend(states[0].transitions.values().copied());
    if !states[0].outputs.is_empty() {
        for state in queue.iter().copied() {
            states[state].output_link = Some(0);
        }
    }

    while let Some(state) = queue.pop_front() {
        let mut edges = Vec::new();
        edges
            .try_reserve_exact(states[state].transitions.len())
            .map_err(|_| BuildError::Allocation {
                component: "failure-link transition snapshot",
            })?;
        edges.extend(states[state].transitions.iter().map(|(byte, next)| (*byte, *next)));
        for (byte, next) in edges {
            queue.push_back(next);
            let mut fallback = states[state].failure;
            while fallback != 0 && !states[fallback].transitions.contains_key(&byte) {
                fallback = states[fallback].failure;
            }
            if let Some(target) = states[fallback].transitions.get(&byte) {
                states[next].failure = *target;
            }
            let failure = states[next].failure;
            states[next].output_link = if states[failure].outputs.is_empty() {
                states[failure].output_link
            } else {
                Some(failure)
            };
        }
    }
    Ok(())
}

fn check_minimum_compiled_bytes(
    states: usize,
    transitions: usize,
    outputs: usize,
    pattern_lengths: usize,
    limit: usize,
) -> Result<(), BuildError> {
    let actual = states
        .checked_mul(size_of::<u32>())
        .and_then(|bytes| bytes.checked_add(transitions.checked_mul(size_of::<u32>() * 2)?))
        .and_then(|bytes| bytes.checked_add(outputs.checked_mul(size_of::<PatternId>())?))
        .and_then(|bytes| bytes.checked_add(pattern_lengths.checked_mul(size_of::<usize>())?))
        .ok_or(BuildError::SizeOverflow {
            component: "minimum backend bytes",
        })?;
    if actual > limit {
        return Err(BuildError::CompiledByteLimit { actual, limit });
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/support/aho_corasick/nfa/build.rs"]
mod test_support;
