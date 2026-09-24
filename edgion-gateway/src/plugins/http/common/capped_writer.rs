//! A byte sink that rejects output before it exceeds a configured limit.

use std::io::{self, Write};

/// Incremental output buffer with a hard byte limit.
pub struct CappedWriter {
    bytes: Vec<u8>,
    limit: usize,
    overflowed: bool,
}

impl CappedWriter {
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(8 * 1024)),
            limit,
            overflowed: false,
        }
    }

    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > self.limit)
        {
            self.overflowed = true;
            return Err(io::Error::other("output limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/common/capped_writer_test.rs"]
mod tests;
