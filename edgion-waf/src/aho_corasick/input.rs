use super::{Anchored, MatchError};

/// Validated borrowed input configuration for one block search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchInput<'h> {
    haystack: &'h [u8],
    start: usize,
    end: usize,
    anchored: Anchored,
    earliest: bool,
}

impl<'h> SearchInput<'h> {
    /// Search an entire byte slice without anchoring.
    pub fn new<H>(haystack: &'h H) -> Self
    where
        H: ?Sized + AsRef<[u8]>,
    {
        let haystack = haystack.as_ref();
        Self {
            haystack,
            start: 0,
            end: haystack.len(),
            anchored: Anchored::No,
            earliest: false,
        }
    }

    /// Replace the absolute half-open search range after validating it.
    pub fn try_range(mut self, start: usize, end: usize) -> Result<Self, MatchError> {
        if start > end || end > self.haystack.len() {
            return Err(MatchError::InvalidRange {
                start,
                end,
                haystack_len: self.haystack.len(),
            });
        }
        self.start = start;
        self.end = end;
        Ok(self)
    }

    /// Select anchored or unanchored search behavior.
    pub const fn anchored(mut self, anchored: Anchored) -> Self {
        self.anchored = anchored;
        self
    }

    /// Return a match as soon as one is observed, overriding leftmost lookahead.
    pub const fn earliest(mut self, yes: bool) -> Self {
        self.earliest = yes;
        self
    }

    /// Return the original complete haystack.
    pub const fn haystack(self) -> &'h [u8] {
        self.haystack
    }

    /// Return the absolute inclusive range start.
    pub const fn start(self) -> usize {
        self.start
    }

    /// Return the absolute exclusive range end.
    pub const fn end(self) -> usize {
        self.end
    }

    /// Return the requested anchor mode.
    pub const fn anchor_mode(self) -> Anchored {
        self.anchored
    }

    /// Return whether earliest-end behavior was requested.
    pub const fn is_earliest(self) -> bool {
        self.earliest
    }

    pub(super) const fn with_start(mut self, start: usize) -> Self {
        self.start = start;
        self
    }
}

impl<'h, H> From<&'h H> for SearchInput<'h>
where
    H: ?Sized + AsRef<[u8]>,
{
    fn from(haystack: &'h H) -> Self {
        Self::new(haystack)
    }
}
