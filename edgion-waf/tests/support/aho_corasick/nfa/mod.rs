use super::*;

impl Backend {
    #[cfg(test)]
    pub(in crate::aho_corasick) fn stored_output_count(&self) -> usize {
        match self {
            Self::Sparse(nfa) => nfa.stored_output_count(),
            Self::Contiguous(nfa) => nfa.stored_output_count(),
            Self::Dfa(dfa) => dfa.stored_output_count(),
        }
    }
}
