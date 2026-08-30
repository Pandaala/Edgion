use serde::Serialize;
use smallvec::SmallVec;

/// Tier-1 fixed buffer capacity (bytes). Entries that no longer fit overflow
/// into the tier-2 `ULogBuffer`.
pub const BUFFER_CAPACITY: usize = 128;

/// Inline capacity hint for the `positions` SmallVec — NOT the hard entry-count
/// cap. The index may spill to the heap beyond this capacity.
const POSITIONS_INLINE_CAP: usize = 20;

/// Tier-2 overflow buffer cap (bytes).
pub const ULOG_MAX_BUFFER: usize = 512;

/// Byte-bounded log buffer. The byte store is stack-allocated; the `positions`
/// index is inline for up to `POSITIONS_INLINE_CAP` entries and spills to the
/// heap beyond that. The entry count is also capped at `BUFFER_CAPACITY` so
/// zero-length entries cannot grow the index without bound.
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
        if self.positions.len() >= BUFFER_CAPACITY || log.len() > BUFFER_CAPACITY - self.buffer.len() {
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
        let total = PREFIX.len().saturating_add(kind.len());
        if self.positions.len() >= BUFFER_CAPACITY || total > BUFFER_CAPACITY - self.buffer.len() {
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

/// Heap-backed log buffer with byte and entry-count limits.
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
        if self.positions.len() >= ULOG_MAX_BUFFER || log.len() > ULOG_MAX_BUFFER - self.buffer.len() {
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
        let total = PREFIX.len().saturating_add(kind.len());
        if self.positions.len() >= ULOG_MAX_BUFFER || total > ULOG_MAX_BUFFER - self.buffer.len() {
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
    fn log_buffer_accepts_one_entry_per_byte() {
        let mut b = LogBuffer::new();
        // 1-byte entries: all 128 fit within the 128-byte budget.
        for i in 0..128 {
            assert!(b.push("x"), "entry {i} should fit under the byte budget");
        }
        assert!(!b.push("x"), "129th byte must be rejected");
    }

    #[test]
    fn log_buffer_bounds_zero_length_entries() {
        let mut b = LogBuffer::new();
        for i in 0..BUFFER_CAPACITY {
            assert!(b.push(""), "empty entry {i} should fit the entry budget");
        }
        assert!(!b.push(""), "entry index must remain bounded when entries use no bytes");
        assert_eq!(b.entries().count(), BUFFER_CAPACITY);
    }

    #[test]
    fn log_buffer_rejects_oversize_entry_without_partial_write() {
        let mut b = LogBuffer::new();
        let big = "y".repeat(200);
        assert!(!b.push(&big), "an over-budget entry is rejected");
        // The buffer was not partially mutated: a small entry still fits.
        assert!(b.push("ok"), "buffer untouched after a rejected oversize push");
    }

    #[test]
    fn log_buffer_push_err_preserves_prefix_and_entry_boundaries() {
        let mut b = LogBuffer::new();
        assert!(b.push("OK"));
        assert!(b.push_err("dependency fail"));

        assert_eq!(b.entries().collect::<Vec<_>>(), ["OK", "err: dependency fail"]);
        assert_eq!(serde_json::to_string(&b).unwrap(), r#"["OK","err: dependency fail"]"#);
    }

    #[test]
    fn ulog_buffer_enforces_byte_cap_without_partial_write() {
        let mut b = ULogBuffer::new();
        assert!(b.push(&"x".repeat(ULOG_MAX_BUFFER)));
        assert!(!b.push("y"), "an entry beyond the byte cap must be rejected");
        assert_eq!(b.entries().collect::<Vec<_>>(), ["x".repeat(ULOG_MAX_BUFFER)]);
    }

    #[test]
    fn ulog_buffer_bounds_zero_length_entries() {
        let mut b = ULogBuffer::new();
        for i in 0..ULOG_MAX_BUFFER {
            assert!(b.push(""), "empty entry {i} should fit the entry budget");
        }
        assert!(!b.push(""), "entry index must remain bounded when entries use no bytes");
        assert_eq!(b.entries().count(), ULOG_MAX_BUFFER);
    }

    #[test]
    fn ulog_buffer_push_err_preserves_prefix_and_entry_boundaries() {
        let mut b = ULogBuffer::new();
        assert!(b.push("OK"));
        assert!(b.push_err("dependency fail"));

        assert_eq!(b.entries().collect::<Vec<_>>(), ["OK", "err: dependency fail"]);
        assert_eq!(serde_json::to_string(&b).unwrap(), r#"["OK","err: dependency fail"]"#);
    }
}
