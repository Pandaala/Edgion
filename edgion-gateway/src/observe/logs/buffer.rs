use serde::Serialize;
use smallvec::SmallVec;

/// Tier-1 fixed buffer capacity (bytes). Entries that no longer fit overflow
/// into the tier-2 `ULogBuffer`.
const BUFFER_CAPACITY: usize = 128;

/// Inline capacity hint for the `positions` SmallVec — NOT a hard entry-count
/// cap. The only limit on tier 1 is `BUFFER_CAPACITY`; in the pathological
/// many-tiny-entries case `positions` simply spills to the heap.
const POSITIONS_INLINE_CAP: usize = 20;

/// Tier-2 overflow buffer cap (bytes).
const ULOG_MAX_BUFFER: usize = 512;

/// Byte-bounded log buffer. The byte store is stack-allocated; the `positions`
/// index is inline for up to `POSITIONS_INLINE_CAP` entries and spills to the
/// heap beyond that.
#[derive(Debug, Clone)]
pub struct LogBuffer {
    buffer: SmallVec<[u8; BUFFER_CAPACITY]>,
    positions: SmallVec<[usize; POSITIONS_INLINE_CAP]>,
}

impl LogBuffer {
    #[inline]
    pub fn new() -> Self {
        Self {
            buffer: SmallVec::new(),
            positions: SmallVec::new(),
        }
    }

    /// Push a plain log entry. For error entries, use [`LogBuffer::push_err`] —
    /// it enforces a `&'static str` argument and unified `"err: "` prefix to
    /// block accidental token/claim leakage via `format!` or error `Display`.
    #[inline]
    pub fn push(&mut self, log: &str) -> bool {
        if self.buffer.len() + log.len() > BUFFER_CAPACITY {
            return false;
        }

        self.buffer.extend_from_slice(log.as_bytes());
        self.positions.push(self.buffer.len());
        true
    }

    /// Push an error entry with unified `"err: "` prefix.
    /// `kind` is `&'static str` — enforces compile-time literal, blocks accidental
    /// token / claim / user-input leakage via `format!` or error `Display`.
    #[inline]
    pub fn push_err(&mut self, kind: &'static str) -> bool {
        const PREFIX: &[u8] = b"err: ";
        let total = PREFIX.len() + kind.len();
        if self.buffer.len() + total > BUFFER_CAPACITY {
            return false;
        }
        self.buffer.extend_from_slice(PREFIX);
        self.buffer.extend_from_slice(kind.as_bytes());
        self.positions.push(self.buffer.len());
        true
    }

    /// Iterate the buffer's entries, in push order.
    pub fn entries(&self) -> impl Iterator<Item = &str> + '_ {
        let buffer = &self.buffer;
        let mut start = 0;
        self.positions.iter().map(move |&end| {
            let slice = &buffer[start..end];
            start = end;
            std::str::from_utf8(slice).unwrap_or("")
        })
    }

    #[cfg(test)]
    pub fn contains(&self, substr: &str) -> bool {
        std::str::from_utf8(&self.buffer)
            .map(|content| content.contains(substr))
            .unwrap_or(false)
    }
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Serialize for LogBuffer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.entries())
    }
}

/// Unbounded log buffer with an upper size limit.
#[derive(Debug, Clone)]
pub struct ULogBuffer {
    buffer: String,
    positions: Vec<usize>,
}

impl ULogBuffer {
    #[inline]
    pub fn new() -> Self {
        Self {
            buffer: String::with_capacity(256),
            positions: Vec::with_capacity(32),
        }
    }

    /// Push a plain log entry. For error entries, use [`ULogBuffer::push_err`];
    /// see [`LogBuffer::push_err`] for rationale.
    #[inline]
    pub fn push(&mut self, log: &str) -> bool {
        if self.buffer.len() + log.len() > ULOG_MAX_BUFFER {
            return false;
        }
        self.buffer.push_str(log);
        self.positions.push(self.buffer.len());
        true
    }

    /// Push an error entry with unified `"err: "` prefix.
    /// See [`LogBuffer::push_err`] for rationale.
    #[inline]
    pub fn push_err(&mut self, kind: &'static str) -> bool {
        const PREFIX: &str = "err: ";
        let total = PREFIX.len() + kind.len();
        if self.buffer.len() + total > ULOG_MAX_BUFFER {
            return false;
        }
        self.buffer.push_str(PREFIX);
        self.buffer.push_str(kind);
        self.positions.push(self.buffer.len());
        true
    }

    /// Iterate the buffer's entries, in push order.
    pub fn entries(&self) -> impl Iterator<Item = &str> + '_ {
        let buffer = &self.buffer;
        let mut start = 0;
        self.positions.iter().map(move |&end| {
            let slice = &buffer[start..end];
            start = end;
            slice
        })
    }

    #[cfg(test)]
    pub fn contains(&self, substr: &str) -> bool {
        self.buffer.contains(substr)
    }
}

impl Default for ULogBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Serialize for ULogBuffer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.entries())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_buffer_accepts_until_128_bytes() {
        let mut b = LogBuffer::new();
        // 16-byte entries: 8 entries == 128 bytes exactly fit, the 9th is rejected.
        for i in 0..8 {
            assert!(b.push("0123456789abcdef"), "entry {i} should fit the 128-byte budget");
        }
        assert!(
            !b.push("0123456789abcdef"),
            "9th entry must be rejected (byte budget full)"
        );
    }

    #[test]
    fn log_buffer_has_no_entry_count_cap() {
        let mut b = LogBuffer::new();
        // 1-byte entries: all 128 fit within the 128-byte budget.
        for i in 0..128 {
            assert!(b.push("x"), "entry {i} should fit under the byte budget");
        }
        assert!(!b.push("x"), "129th byte must be rejected");
    }

    #[test]
    fn log_buffer_rejects_oversize_entry_without_partial_write() {
        let mut b = LogBuffer::new();
        let big = "y".repeat(200);
        assert!(!b.push(&big), "an over-budget entry is rejected");
        // The buffer was not partially mutated: a small entry still fits.
        assert!(b.push("ok"), "buffer untouched after a rejected oversize push");
    }
}
