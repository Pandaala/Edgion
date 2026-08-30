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
mod tests {
    use std::io;

    use super::*;
    use crate::aho_corasick::{Builder, MatchKind, PatternId, StartKind};

    fn tuples(matches: impl IntoIterator<Item = Match>) -> Vec<(usize, usize, usize)> {
        matches
            .into_iter()
            .map(|matched| (matched.pattern_id().as_usize(), matched.start(), matched.end()))
            .collect()
    }

    #[test]
    fn every_two_chunk_split_matches_block_iteration() {
        let matcher = AhoCorasick::new([b"abcd".as_slice(), b"bc".as_slice(), b"x".as_slice()]).unwrap();
        let haystack = b"xxabcdxbc";
        let expected = tuples(matcher.find_iter(haystack));

        for split in 0..=haystack.len() {
            let mut state = matcher.try_stream_state().unwrap();
            let mut actual = Vec::new();
            matcher
                .try_stream_chunk(&mut state, &haystack[..split], false, |matched| actual.push(matched))
                .unwrap();
            matcher
                .try_stream_chunk(&mut state, &haystack[split..], true, |matched| actual.push(matched))
                .unwrap();
            assert_eq!(tuples(actual), expected, "split={split}");
            assert_eq!(state.absolute_offset(), haystack.len());
            assert!(state.is_finished());
            assert_eq!(state.memory_usage(), size_of::<StreamState>());
        }
    }

    #[test]
    fn reader_buffer_sizes_preserve_cross_boundary_matches() {
        let matcher = AhoCorasick::new([b"append".as_slice(), b"app".as_slice()]).unwrap();
        let haystack = b"append the app";
        let expected = tuples(matcher.find_iter(haystack));
        for buffer_bytes in [1, 2, 3, 7, 32] {
            let actual = matcher
                .try_stream_find_iter_with_buffer(haystack.as_slice(), buffer_bytes)
                .unwrap()
                .collect::<io::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(tuples(actual), expected, "buffer={buffer_bytes}");
        }
        assert!(matches!(
            matcher.try_stream_find_iter_with_buffer(haystack.as_slice(), 0),
            Err(MatchError::StreamBufferSize { .. })
        ));
    }

    #[test]
    fn reader_scratch_does_not_grow_with_total_stream_length() {
        let matcher = AhoCorasick::new([b"abc".as_slice()]).unwrap();
        let haystack = vec![b'z'; 1024 * 1024];
        let mut iter = matcher
            .try_stream_find_iter_with_buffer(haystack.as_slice(), 17)
            .unwrap();
        let memory_usage = iter.memory_usage();
        assert_eq!(iter.buffer_size(), 17);
        assert!(iter.next().is_none());
        assert_eq!(iter.memory_usage(), memory_usage);
    }

    #[test]
    fn stream_support_errors_happen_before_consumption() {
        let mut leftmost_builder = Builder::new();
        leftmost_builder.match_kind(MatchKind::LeftmostFirst);
        let leftmost = leftmost_builder.build([b"a".as_slice()]).unwrap();
        assert_eq!(
            leftmost.try_stream_state().unwrap_err(),
            MatchError::UnsupportedStream {
                match_kind: MatchKind::LeftmostFirst,
            }
        );

        let empty = AhoCorasick::new([b"".as_slice(), b"a".as_slice()]).unwrap();
        assert_eq!(
            empty.try_stream_state().unwrap_err(),
            MatchError::UnsupportedStreamEmpty
        );

        let mut anchored_builder = Builder::new();
        anchored_builder.start_kind(StartKind::Anchored);
        let anchored = anchored_builder.build([b"a".as_slice()]).unwrap();
        assert!(matches!(
            anchored.try_stream_state(),
            Err(MatchError::UnsupportedAnchored { .. })
        ));
    }

    #[test]
    fn stream_state_is_generation_bound_and_single_use() {
        let first = AhoCorasick::new([b"a".as_slice()]).unwrap();
        let second = AhoCorasick::new([b"a".as_slice()]).unwrap();
        let mut state = first.try_stream_state().unwrap();
        assert!(matches!(
            second.try_stream_chunk(&mut state, b"a", false, |_| {}),
            Err(MatchError::StreamStateShape { .. })
        ));
        first.try_stream_chunk(&mut state, b"a", true, |_| {}).unwrap();
        assert_eq!(
            first.try_stream_chunk(&mut state, b"", true, |_| {}).unwrap_err(),
            MatchError::StreamFinished
        );
    }

    #[derive(Debug)]
    struct FailingReader {
        emitted: bool,
    }

    impl Read for FailingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.emitted {
                self.emitted = true;
                buffer[0] = b'a';
                return Ok(1);
            }
            Err(io::Error::new(io::ErrorKind::ConnectionReset, "reader failed"))
        }
    }

    #[test]
    fn reader_errors_are_preserved_without_payload_context() {
        let matcher = AhoCorasick::new([b"z".as_slice()]).unwrap();
        let mut iter = matcher
            .try_stream_find_iter_with_buffer(FailingReader { emitted: false }, 1)
            .unwrap();
        let error = iter.next().unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(error.to_string(), "reader failed");
        assert!(iter.next().is_none());
    }

    #[test]
    fn duplicate_stream_patterns_keep_source_order() {
        let matcher = AhoCorasick::new([b"a".as_slice(), b"a".as_slice()]).unwrap();
        let matches = matcher
            .try_stream_find_iter_with_buffer(b"a".as_slice(), 1)
            .unwrap()
            .collect::<io::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(matches[0].pattern_id(), PatternId::try_from_usize(0).unwrap());
    }
}
