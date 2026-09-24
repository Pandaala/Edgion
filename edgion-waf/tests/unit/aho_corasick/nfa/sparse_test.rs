use super::*;
use crate::aho_corasick::BuildLimits;

#[test]
fn dense_root_is_conditional_and_memory_accounting_is_exact() {
    let limits = BuildLimits::default();
    let low_fanout = BuildGraph::build([b"alpha".as_slice(), b"beta".as_slice()], limits, false).unwrap();
    let sparse = SparseNfa::from_graph(low_fanout, limits.max_compiled_bytes).unwrap();
    assert_eq!(sparse.dense_state_count(), 0);
    assert_eq!(
        sparse.memory_usage,
        allocated_bytes(
            &sparse.states,
            sparse.states.capacity(),
            sparse.dense_root.capacity(),
            sparse.pattern_lengths.capacity(),
        )
    );

    let patterns = (u8::MIN..=u8::MAX).map(|byte| [byte]);
    let dense_graph = BuildGraph::build(patterns, limits, false).unwrap();
    let sparse = SparseNfa::from_graph(dense_graph, limits.max_compiled_bytes).unwrap();
    assert_eq!(sparse.dense_state_count(), 1);
    assert_eq!(sparse.dense_root.len(), ALPHABET_LEN);
    assert_eq!(
        sparse.memory_usage,
        allocated_bytes(
            &sparse.states,
            sparse.states.capacity(),
            sparse.dense_root.capacity(),
            sparse.pattern_lengths.capacity(),
        )
    );
    for byte in u8::MIN..=u8::MAX {
        assert_ne!(sparse.next_state(0, byte), 0);
    }

    let required = sparse.memory_usage();
    let dense_graph = BuildGraph::build((u8::MIN..=u8::MAX).map(|byte| [byte]), limits, false).unwrap();
    assert_eq!(
        SparseNfa::from_graph(dense_graph, required - 1).unwrap_err(),
        BuildError::CompiledByteLimit {
            actual: required,
            limit: required - 1,
        }
    );
}

impl SparseNfa {
    #[cfg(test)]
    pub(crate) fn stored_output_count(&self) -> usize {
        self.states.iter().map(|state| state.outputs.len()).sum()
    }
}
