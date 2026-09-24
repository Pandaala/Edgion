use super::*;

impl BuildGraph {
    #[cfg(test)]
    pub(crate) fn build<I, P>(
        patterns: I,
        limits: BuildLimits,
        ascii_case_insensitive: bool,
    ) -> Result<Self, BuildError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        Self::build_profiled(patterns, limits, ascii_case_insensitive, None)
    }
}
