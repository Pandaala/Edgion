use super::*;

#[test]
fn byte_classes_partition_every_byte_deterministically() {
    let graph = BuildGraph::build([b"az".as_slice(), b"za".as_slice()], BuildLimits::default(), false).unwrap();
    let classes = ByteClasses::from_graph(&graph).unwrap();

    assert_eq!(classes.len(), 3);
    assert_ne!(classes.get(b'a'), classes.get(b'z'));
    assert_eq!(classes.get(0), classes.get(b'b'));
    assert_eq!(classes.get(b'b'), classes.get(u8::MAX));
    for byte in u8::MIN..=u8::MAX {
        assert!(classes.get(byte) < classes.len());
    }
    for (class, representative) in classes.representatives.iter().copied().enumerate() {
        assert_eq!(classes.get(representative), class);
    }
}

#[test]
fn all_observed_bytes_form_singleton_classes() {
    let patterns: Vec<[u8; 1]> = (u8::MIN..=u8::MAX).map(|byte| [byte]).collect();
    let graph = BuildGraph::build(&patterns, BuildLimits::default(), false).unwrap();
    let classes = ByteClasses::from_graph(&graph).unwrap();

    assert_eq!(classes.len(), 256);
    for byte in u8::MIN..=u8::MAX {
        assert_eq!(classes.get(byte), usize::from(byte));
    }
}

#[test]
fn dfa_transitions_equal_graph_failure_resolution_for_every_byte() {
    let limits = BuildLimits::default();
    let graph = BuildGraph::build(
        [
            b"he".as_slice(),
            b"she".as_slice(),
            b"hers".as_slice(),
            b"his".as_slice(),
        ],
        limits,
        false,
    )
    .unwrap();
    let expected: Vec<Vec<usize>> = (0..graph.states.len())
        .map(|state| (u8::MIN..=u8::MAX).map(|byte| graph.next_state(state, byte)).collect())
        .collect();
    let dfa = Dfa::from_graph(graph, limits).unwrap();

    for (state, expected_row) in expected.iter().enumerate() {
        for byte in u8::MIN..=u8::MAX {
            assert_eq!(dfa.next_state(state, byte), expected_row[usize::from(byte)]);
        }
    }
}

#[test]
fn reported_memory_matches_owned_vector_capacities() {
    let limits = BuildLimits::default();
    let graph = BuildGraph::build([b"alpha".as_slice(), b"beta".as_slice()], limits, false).unwrap();
    let dfa = Dfa::from_graph(graph, limits).unwrap();
    assert_eq!(
        dfa.memory_usage(),
        allocated_bytes(
            dfa.states.capacity(),
            dfa.transitions.capacity(),
            dfa.outputs.capacity(),
            dfa.pattern_lengths.capacity(),
            dfa.byte_classes.capacity(),
        )
    );
    assert!(dfa.estimated_peak_build_memory_usage() >= dfa.memory_usage());
}

impl ByteClasses {
    #[cfg(test)]
    pub(super) fn get(&self, byte: u8) -> usize {
        usize::from(self.map[usize::from(byte)])
    }
}

impl Dfa {
    #[cfg(test)]
    pub(in crate::aho_corasick) fn stored_output_count(&self) -> usize {
        self.outputs.len()
    }
}
