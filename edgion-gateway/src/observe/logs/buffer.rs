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
#[path = "../../../tests/unit/observe/logs/buffer_test.rs"]
mod tests;
