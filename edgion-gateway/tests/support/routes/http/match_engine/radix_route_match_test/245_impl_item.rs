#[cfg(test)]
impl RadixRouteMatchEngine {
    /// Test-only view of the compiled paths, so tests can assert against the
    /// ranks that `assign_precedence_ranks` actually wrote.
    pub(crate) fn paths_for_test(&self) -> &[RadixPath] {
        &self.radix_paths
    }

    /// Test-only view of the dense bucket table (`tree_value - 1` -> path indices).
    pub(crate) fn buckets_for_test(&self) -> &[Vec<u32>] {
        &self.tree_value_paths
    }

    /// Test-only entry into the request path's filter-and-order step, which
    /// `match_route` otherwise reaches only with a live `Session`.
    pub(crate) fn ordered_candidates_for_test(&self, path: &str) -> SmallVec<[(u32, u32); 8]> {
        self.ordered_candidates(path)
    }
}
