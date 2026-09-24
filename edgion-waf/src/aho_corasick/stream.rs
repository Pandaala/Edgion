use std::collections::VecDeque;
use std::io::{self, Read};
use std::mem::size_of;

use super::{fold_ascii, AhoCorasick, Anchored, Match, MatchError, MatchKind, PatternSet};

/// Default reader buffer used by streaming search and replacement.
pub const DEFAULT_STREAM_BUFFER_BYTES: usize = 8 * 1024;
/// Largest accepted reader buffer for one streaming operation.
pub const MAX_STREAM_BUFFER_BYTES: usize = 1024 * 1024;

/// Caller-owned incremental state for one standard unanchored byte stream.
#[derive(Debug)]
pub struct StreamState {
    generation: u64,
    automaton_state: usize,
    absolute_offset: usize,
    finished: bool,
}

impl StreamState {
    /// Return the number of stream bytes consumed so far.
    pub const fn absolute_offset(&self) -> usize {
        self.absolute_offset
    }

    /// Return the fixed inline scratch size; no heap storage is owned.
    pub const fn memory_usage(&self) -> usize {
        size_of::<Self>()
    }

    /// Return whether end-of-stream has been observed.
    pub const fn is_finished(&self) -> bool {
        self.finished
    }
}

/// Iterator over standard non-overlapping matches read from a byte stream.
#[derive(Debug)]
pub struct StreamFindIter<'a, R> {
    matcher: &'a AhoCorasick,
    reader: R,
    state: StreamState,
    buffer: Vec<u8>,
    pending: VecDeque<Match>,
    exhausted: bool,
}

impl<R> StreamFindIter<'_, R> {
    /// Return current owned scratch bytes; this remains constant while iterating.
    pub fn memory_usage(&self) -> usize {
        size_of::<StreamState>()
            .saturating_add(self.buffer.capacity())
            .saturating_add(self.pending.capacity().saturating_mul(size_of::<Match>()))
    }

    /// Return the configured reader buffer size.
    pub fn buffer_size(&self) -> usize {
        self.buffer.len()
    }
}

impl AhoCorasick {
    /// Create validated caller-owned state for one incremental stream.
    pub fn try_stream_state(&self) -> Result<StreamState, MatchError> {
        self.validate_stream_support()?;
        Ok(StreamState {
            generation: self.generation,
            automaton_state: 0,
            absolute_offset: 0,
            finished: false,
        })
    }

    /// Consume one byte chunk and report standard non-overlapping matches.
    pub fn try_stream_chunk<F>(
        &self,
        state: &mut StreamState,
        chunk: &[u8],
        end_of_stream: bool,
        mut on_match: F,
    ) -> Result<(), MatchError>
    where
        F: FnMut(Match),
    {
        self.validate_stream_state(state)?;
        if state.finished {
            return Err(MatchError::StreamFinished);
        }
        for source_byte in chunk.iter().copied() {
            let byte = fold_ascii(source_byte, self.ascii_case_insensitive);
            state.automaton_state = self.backend.next_state(state.automaton_state, byte);
            state.absolute_offset = state
                .absolute_offset
                .checked_add(1)
                .ok_or(MatchError::StreamOffsetOverflow)?;
            if let Some(pattern_id) = self.backend.outputs(state.automaton_state).next() {
                let start = state.absolute_offset - self.backend.pattern_length(pattern_id);
                on_match(Match::from_validated(pattern_id, start, state.absolute_offset));
                state.automaton_state = 0;
            }
        }
        if end_of_stream {
            state.finished = true;
        }
        Ok(())
    }

    /// Consume one byte chunk and accumulate every matching pattern ID.
    ///
    /// Unlike the non-overlapping callback API, this keeps the exact automaton
    /// state after every output and therefore records suffix and overlapping
    /// patterns across arbitrary chunk boundaries.
    pub fn try_stream_pattern_set_chunk(
        &self,
        state: &mut StreamState,
        chunk: &[u8],
        end_of_stream: bool,
        matches: &mut PatternSet,
    ) -> Result<(), MatchError> {
        self.validate_stream_state(state)?;
        matches.validate_shape(self.pattern_count())?;
        if state.finished {
            return Err(MatchError::StreamFinished);
        }
        for source_byte in chunk.iter().copied() {
            let byte = fold_ascii(source_byte, self.ascii_case_insensitive);
            state.automaton_state = self.backend.next_state(state.automaton_state, byte);
            state.absolute_offset = state
                .absolute_offset
                .checked_add(1)
                .ok_or(MatchError::StreamOffsetOverflow)?;
            for pattern_id in self.backend.outputs(state.automaton_state) {
                matches.insert(pattern_id)?;
            }
        }
        if end_of_stream {
            state.finished = true;
        }
        Ok(())
    }

    /// Construct a reader iterator with the default fixed buffer.
    pub fn try_stream_find_iter<R: Read>(&self, reader: R) -> Result<StreamFindIter<'_, R>, MatchError> {
        self.try_stream_find_iter_with_buffer(reader, DEFAULT_STREAM_BUFFER_BYTES)
    }

    /// Construct a reader iterator with an explicit fixed buffer size.
    pub fn try_stream_find_iter_with_buffer<R: Read>(
        &self,
        reader: R,
        buffer_bytes: usize,
    ) -> Result<StreamFindIter<'_, R>, MatchError> {
        validate_buffer_size(buffer_bytes)?;
        let state = self.try_stream_state()?;
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(buffer_bytes)
            .map_err(|_| MatchError::StreamBufferAllocation {
                requested: buffer_bytes,
            })?;
        buffer.resize(buffer_bytes, 0);
        let max_pending = buffer_bytes
            .checked_div(self.min_pattern_len().unwrap_or(1).max(1))
            .unwrap_or(0)
            .saturating_add(1);
        let mut pending = VecDeque::new();
        pending
            .try_reserve(max_pending)
            .map_err(|_| MatchError::StreamBufferAllocation {
                requested: buffer_bytes,
            })?;
        Ok(StreamFindIter {
            matcher: self,
            reader,
            state,
            buffer,
            pending,
            exhausted: false,
        })
    }

    fn validate_stream_support(&self) -> Result<(), MatchError> {
        if self.match_kind != MatchKind::Standard {
            return Err(MatchError::UnsupportedStream {
                match_kind: self.match_kind,
            });
        }
        if self.min_pattern_len() == Some(0) {
            return Err(MatchError::UnsupportedStreamEmpty);
        }
        if !self.start_kind.supports(Anchored::No) {
            return Err(MatchError::UnsupportedAnchored {
                start_kind: self.start_kind,
                anchored: Anchored::No,
            });
        }
        Ok(())
    }

    fn validate_stream_state(&self, state: &StreamState) -> Result<(), MatchError> {
        self.validate_stream_support()?;
        if state.generation != self.generation {
            return Err(MatchError::StreamStateShape {
                expected: self.generation,
                actual: state.generation,
            });
        }
        Ok(())
    }
}

impl<R: Read> Iterator for StreamFindIter<'_, R> {
    type Item = io::Result<Match>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(matched) = self.pending.pop_front() {
                return Some(Ok(matched));
            }
            if self.exhausted {
                return None;
            }
            let read = match self.reader.read(&mut self.buffer) {
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.exhausted = true;
                    return Some(Err(error));
                }
            };
            let end_of_stream = read == 0;
            let result =
                self.matcher
                    .try_stream_chunk(&mut self.state, &self.buffer[..read], end_of_stream, |matched| {
                        self.pending.push_back(matched)
                    });
            if let Err(error) = result {
                self.exhausted = true;
                return Some(Err(io::Error::other(error)));
            }
            if end_of_stream {
                self.exhausted = true;
            }
        }
    }
}

impl<R: Read> std::iter::FusedIterator for StreamFindIter<'_, R> {}

pub(super) fn validate_buffer_size(buffer_bytes: usize) -> Result<(), MatchError> {
    if (1..=MAX_STREAM_BUFFER_BYTES).contains(&buffer_bytes) {
        Ok(())
    } else {
        Err(MatchError::StreamBufferSize {
            actual: buffer_bytes,
            min: 1,
            max: MAX_STREAM_BUFFER_BYTES,
        })
    }
}

#[cfg(test)]
#[path = "../../tests/unit/aho_corasick/stream_test.rs"]
mod tests;
