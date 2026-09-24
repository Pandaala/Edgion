use std::mem::size_of;

use super::nfa::{BuildGraph, OutputCursor};
use super::{BuildError, BuildLimits, PatternId};

const NO_STATE: u32 = u32::MAX;

#[derive(Debug, Clone)]
struct ByteClasses {
    map: [u8; 256],
    representatives: Vec<u8>,
}

impl ByteClasses {
    fn from_graph(graph: &BuildGraph) -> Result<Self, BuildError> {
        let mut observed = [false; 256];
        for state in &graph.states {
            for byte in state.transitions.keys().copied() {
                observed[usize::from(byte)] = true;
            }
        }

        let mut map = [0_u8; 256];
        let mut representatives = Vec::new();
        representatives
            .try_reserve_exact(256)
            .map_err(|_| BuildError::Allocation {
                component: "DFA byte-class representatives",
            })?;
        for byte in u8::MIN..=u8::MAX {
            if observed[usize::from(byte)] {
                let class = u8::try_from(representatives.len()).map_err(|_| BuildError::IndexOverflow {
                    component: "DFA byte class",
                    value: representatives.len(),
                })?;
                map[usize::from(byte)] = class;
                representatives.push(byte);
            }
        }

        if let Some(representative) = (u8::MIN..=u8::MAX).find(|byte| !observed[usize::from(*byte)]) {
            let class = u8::try_from(representatives.len()).map_err(|_| BuildError::IndexOverflow {
                component: "DFA fallback byte class",
                value: representatives.len(),
            })?;
            for byte in u8::MIN..=u8::MAX {
                if !observed[usize::from(byte)] {
                    map[usize::from(byte)] = class;
                }
            }
            representatives.push(representative);
        }

        Ok(Self { map, representatives })
    }

    fn len(&self) -> usize {
        self.representatives.len()
    }
}

#[derive(Debug, Clone, Copy)]
struct DfaState {
    output_start: u32,
    output_len: u32,
    output_link: u32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DfaEstimate {
    pub(super) cells: usize,
    pub(super) memory_usage: usize,
    pub(super) class_count: usize,
}

impl DfaEstimate {
    pub(super) fn from_graph(graph: &BuildGraph) -> Result<Self, BuildError> {
        let classes = ByteClasses::from_graph(graph)?;
        Self::with_class_count(graph, classes.len())
    }

    fn with_class_count(graph: &BuildGraph, class_count: usize) -> Result<Self, BuildError> {
        let cells = graph
            .states
            .len()
            .checked_mul(class_count)
            .ok_or(BuildError::SizeOverflow {
                component: "DFA transition cell count",
            })?;
        let memory_usage = estimate_compiled_bytes(
            graph.states.len(),
            cells,
            graph.output_count,
            graph.pattern_lengths.len(),
        )?;
        Ok(Self {
            cells,
            memory_usage,
            class_count,
        })
    }
}

/// Complete Aho-Corasick transition table indexed by byte equivalence class.
#[derive(Debug)]
pub(super) struct Dfa {
    states: Vec<DfaState>,
    transitions: Vec<u32>,
    outputs: Vec<PatternId>,
    pattern_lengths: Vec<usize>,
    byte_classes: Vec<u8>,
    class_count: usize,
    min_pattern_len: Option<usize>,
    max_pattern_len: usize,
    memory_usage: usize,
    estimated_peak_build_memory_usage: usize,
}

impl Dfa {
    pub(super) fn from_graph(graph: BuildGraph, limits: BuildLimits) -> Result<Self, BuildError> {
        let build_graph_bytes = graph.estimated_memory_usage();
        let classes = ByteClasses::from_graph(&graph)?;
        let estimate = DfaEstimate::with_class_count(&graph, classes.len())?;
        if estimate.cells > limits.max_dfa_cells {
            return Err(BuildError::DfaCellLimit {
                actual: estimate.cells,
                limit: limits.max_dfa_cells,
            });
        }
        check_compiled_byte_limit(estimate.memory_usage, limits.max_compiled_bytes)?;
        checked_state_count(graph.states.len())?;

        let mut byte_classes = Vec::new();
        byte_classes
            .try_reserve_exact(classes.map.len())
            .map_err(|_| BuildError::Allocation {
                component: "DFA byte-class map",
            })?;
        byte_classes.extend_from_slice(&classes.map);

        let mut transitions = Vec::new();
        transitions
            .try_reserve_exact(estimate.cells)
            .map_err(|_| BuildError::Allocation {
                component: "DFA transitions",
            })?;
        for state in 0..graph.states.len() {
            for byte in classes.representatives.iter().copied() {
                transitions.push(checked_state_id(graph.next_state(state, byte))?);
            }
        }

        let mut states = Vec::new();
        states
            .try_reserve_exact(graph.states.len())
            .map_err(|_| BuildError::Allocation {
                component: "DFA states",
            })?;
        let mut outputs = Vec::new();
        outputs
            .try_reserve_exact(graph.output_count)
            .map_err(|_| BuildError::Allocation {
                component: "DFA outputs",
            })?;
        for state in &graph.states {
            let output_start = checked_count(outputs.len(), "DFA output start")?;
            outputs.extend(state.outputs.iter().copied());
            let output_len = checked_count(state.outputs.len(), "DFA output length")?;
            let output_link = match state.output_link {
                Some(link) => checked_state_id(link)?,
                None => NO_STATE,
            };
            states.push(DfaState {
                output_start,
                output_len,
                output_link,
            });
        }

        let mut pattern_lengths = Vec::new();
        pattern_lengths
            .try_reserve_exact(graph.pattern_lengths.len())
            .map_err(|_| BuildError::Allocation {
                component: "DFA pattern lengths",
            })?;
        pattern_lengths.extend(graph.pattern_lengths.iter().copied());

        let memory_usage = allocated_bytes(
            states.capacity(),
            transitions.capacity(),
            outputs.capacity(),
            pattern_lengths.capacity(),
            byte_classes.capacity(),
        );
        check_compiled_byte_limit(memory_usage, limits.max_compiled_bytes)?;
        Ok(Self {
            states,
            transitions,
            outputs,
            pattern_lengths,
            byte_classes,
            class_count: estimate.class_count,
            min_pattern_len: graph.min_pattern_len,
            max_pattern_len: graph.max_pattern_len,
            memory_usage,
            estimated_peak_build_memory_usage: build_graph_bytes.saturating_add(memory_usage),
        })
    }

    pub(super) fn pattern_count(&self) -> usize {
        self.pattern_lengths.len()
    }

    pub(super) fn state_count(&self) -> usize {
        self.states.len()
    }

    pub(super) fn byte_class_count(&self) -> usize {
        self.class_count
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

    pub(super) fn next_state(&self, state: usize, byte: u8) -> usize {
        let class = usize::from(self.byte_classes[usize::from(byte)]);
        let index = state * self.class_count + class;
        self.transitions[index] as usize
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
}

fn checked_state_count(state_count: usize) -> Result<(), BuildError> {
    if state_count >= u32::MAX as usize {
        return Err(BuildError::IndexOverflow {
            component: "DFA state count",
            value: state_count,
        });
    }
    Ok(())
}

fn checked_state_id(state: usize) -> Result<u32, BuildError> {
    if state >= u32::MAX as usize {
        return Err(BuildError::IndexOverflow {
            component: "DFA state ID",
            value: state,
        });
    }
    Ok(state as u32)
}

fn checked_count(value: usize, component: &'static str) -> Result<u32, BuildError> {
    u32::try_from(value).map_err(|_| BuildError::IndexOverflow { component, value })
}

fn estimate_compiled_bytes(
    states: usize,
    cells: usize,
    outputs: usize,
    pattern_lengths: usize,
) -> Result<usize, BuildError> {
    states
        .checked_mul(size_of::<DfaState>())
        .and_then(|total| total.checked_add(256 * size_of::<u8>()))
        .and_then(|total| {
            cells
                .checked_mul(size_of::<u32>())
                .and_then(|bytes| total.checked_add(bytes))
        })
        .and_then(|total| {
            outputs
                .checked_mul(size_of::<PatternId>())
                .and_then(|bytes| total.checked_add(bytes))
        })
        .and_then(|total| {
            pattern_lengths
                .checked_mul(size_of::<usize>())
                .and_then(|bytes| total.checked_add(bytes))
        })
        .ok_or(BuildError::SizeOverflow {
            component: "total DFA compiled bytes",
        })
}

fn allocated_bytes(
    state_capacity: usize,
    transition_capacity: usize,
    output_capacity: usize,
    pattern_length_capacity: usize,
    byte_class_capacity: usize,
) -> usize {
    state_capacity
        .saturating_mul(size_of::<DfaState>())
        .saturating_add(transition_capacity.saturating_mul(size_of::<u32>()))
        .saturating_add(output_capacity.saturating_mul(size_of::<PatternId>()))
        .saturating_add(pattern_length_capacity.saturating_mul(size_of::<usize>()))
        .saturating_add(byte_class_capacity.saturating_mul(size_of::<u8>()))
}

fn check_compiled_byte_limit(actual: usize, limit: usize) -> Result<(), BuildError> {
    if actual > limit {
        return Err(BuildError::CompiledByteLimit { actual, limit });
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/aho_corasick/dfa_test.rs"]
mod tests;
