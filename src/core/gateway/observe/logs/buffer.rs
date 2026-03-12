use serde::Serialize;
use smallvec::SmallVec;

/// Fixed buffer capacity (bytes)
const BUFFER_CAPACITY: usize = 100;

/// Max log entries in fixed buffer
const MAX_LOG_ENTRIES: usize = 20;

/// Max buffer size (bytes) for unbounded ULogBuffer
const ULOG_MAX_BUFFER: usize = 512;

/// Fixed-size log buffer (stack-allocated, zero heap allocation)
#[derive(Debug, Clone)]
pub struct LogBuffer {
    buffer: SmallVec<[u8; BUFFER_CAPACITY]>,
    positions: SmallVec<[usize; MAX_LOG_ENTRIES]>,
}

impl LogBuffer {
    #[inline]
    pub fn new() -> Self {
        Self {
            buffer: SmallVec::new(),
            positions: SmallVec::new(),
        }
    }

    #[inline]
    pub fn push(&mut self, log: &str) -> bool {
        if self.positions.len() >= MAX_LOG_ENTRIES {
            return false;
        }
        if self.buffer.len() + log.len() > BUFFER_CAPACITY {
            return false;
        }

        self.buffer.extend_from_slice(log.as_bytes());
        self.positions.push(self.buffer.len());
        true
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
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.positions.len()))?;
        let mut start = 0;
        for &end in &self.positions {
            let slice = &self.buffer[start..end];
            let s = std::str::from_utf8(slice).map_err(serde::ser::Error::custom)?;
            seq.serialize_element(s)?;
            start = end;
        }
        seq.end()
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

    #[inline]
    pub fn push(&mut self, log: &str) -> bool {
        if self.buffer.len() + log.len() > ULOG_MAX_BUFFER {
            return false;
        }
        self.buffer.push_str(log);
        self.positions.push(self.buffer.len());
        true
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
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.positions.len()))?;
        let mut start = 0;
        for &end in &self.positions {
            seq.serialize_element(&self.buffer[start..end])?;
            start = end;
        }
        seq.end()
    }
}
