use super::*;

impl PatternSet {
    #[cfg(test)]
    pub(crate) fn word_capacity(&self) -> usize {
        self.words.capacity()
    }
    #[cfg(test)]
    pub(crate) fn word_allocation_ptr(&self) -> *const u64 {
        self.words.as_ptr()
    }
}
