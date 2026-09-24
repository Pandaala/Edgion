#[cfg(test)]
impl Vm {
    /// Expose the internal const_str_cache for alignment invariant tests.
    pub(crate) fn const_str_cache_ref(&self) -> &[Option<Arc<str>>] {
        &self.const_str_cache
    }
}
