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

impl ContiguousNfa {
    #[cfg(test)]
    pub(in crate::aho_corasick::nfa) fn stored_output_count(&self) -> usize {
        self.outputs.len()
    }
}
