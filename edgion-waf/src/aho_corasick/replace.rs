use std::io::{self, Read, Write};

use super::stream::{validate_buffer_size, DEFAULT_STREAM_BUFFER_BYTES};
use super::{AhoCorasick, Match, MatchError};

/// Default maximum bytes produced by one block replacement operation.
pub const DEFAULT_MAX_REPLACEMENT_BYTES: usize = 64 * 1024 * 1024;

impl AhoCorasick {
    /// Return the logical byte-buffer bound for streaming replacement.
    pub fn stream_replacement_buffer_bound(&self, buffer_bytes: usize) -> Result<usize, MatchError> {
        validate_buffer_size(buffer_bytes)?;
        buffer_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(self.max_pattern_len().saturating_sub(1)))
            .ok_or(MatchError::StreamMemoryOverflow)
    }

    /// Replace non-overlapping byte matches using the default output cap.
    pub fn replace_all_bytes<B>(&self, haystack: &[u8], replacements: &[B]) -> Result<Vec<u8>, MatchError>
    where
        B: AsRef<[u8]>,
    {
        self.replace_all_bytes_with_limit(haystack, replacements, DEFAULT_MAX_REPLACEMENT_BYTES)
    }

    /// Replace non-overlapping byte matches under an explicit output cap.
    pub fn replace_all_bytes_with_limit<B>(
        &self,
        haystack: &[u8],
        replacements: &[B],
        max_output_bytes: usize,
    ) -> Result<Vec<u8>, MatchError>
    where
        B: AsRef<[u8]>,
    {
        self.validate_replacements(replacements.len())?;
        let mut output = Vec::new();
        reserve_bounded(&mut output, haystack.len().min(max_output_bytes), max_output_bytes)?;
        let mut previous_end = 0;
        for matched in self.try_find_iter(super::SearchInput::new(haystack))? {
            append_bounded(&mut output, &haystack[previous_end..matched.start()], max_output_bytes)?;
            append_bounded(
                &mut output,
                replacements[matched.pattern_id().as_usize()].as_ref(),
                max_output_bytes,
            )?;
            previous_end = matched.end();
        }
        append_bounded(&mut output, &haystack[previous_end..], max_output_bytes)?;
        Ok(output)
    }

    /// Replace byte matches with values returned by a callback.
    pub fn replace_all_bytes_with<F>(
        &self,
        haystack: &[u8],
        max_output_bytes: usize,
        mut replacement: F,
    ) -> Result<Vec<u8>, MatchError>
    where
        F: FnMut(Match, &[u8]) -> Vec<u8>,
    {
        let mut output = Vec::new();
        reserve_bounded(&mut output, haystack.len().min(max_output_bytes), max_output_bytes)?;
        let mut previous_end = 0;
        for matched in self.try_find_iter(super::SearchInput::new(haystack))? {
            append_bounded(&mut output, &haystack[previous_end..matched.start()], max_output_bytes)?;
            let bytes = replacement(matched, &haystack[matched.start()..matched.end()]);
            append_bounded(&mut output, &bytes, max_output_bytes)?;
            previous_end = matched.end();
        }
        append_bounded(&mut output, &haystack[previous_end..], max_output_bytes)?;
        Ok(output)
    }

    /// Replace non-overlapping UTF-8 matches using the default output cap.
    pub fn replace_all<B>(&self, haystack: &str, replacements: &[B]) -> Result<String, MatchError>
    where
        B: AsRef<str>,
    {
        self.validate_replacements(replacements.len())?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(replacements.len())
            .map_err(|_| MatchError::ReplacementAllocation)?;
        bytes.extend(replacements.iter().map(|value| value.as_ref().as_bytes()));
        self.validate_string_boundaries(haystack)?;
        let output = self.replace_all_bytes(haystack.as_bytes(), &bytes)?;
        String::from_utf8(output).map_err(|error| {
            let valid = error.utf8_error().valid_up_to();
            MatchError::InvalidUtf8Boundary {
                start: valid,
                end: valid.saturating_add(1),
            }
        })
    }

    /// Incrementally replace standard stream matches with a fixed table.
    pub fn try_stream_replace_all<R, W, B>(
        &self,
        reader: R,
        writer: W,
        replacements: &[B],
    ) -> Result<(), StreamReplaceError>
    where
        R: Read,
        W: Write,
        B: AsRef<[u8]>,
    {
        self.try_stream_replace_all_with_buffer(reader, writer, replacements, DEFAULT_STREAM_BUFFER_BYTES)
    }

    /// Incrementally replace standard stream matches with an explicit reader buffer.
    pub fn try_stream_replace_all_with_buffer<R, W, B>(
        &self,
        reader: R,
        writer: W,
        replacements: &[B],
        buffer_bytes: usize,
    ) -> Result<(), StreamReplaceError>
    where
        R: Read,
        W: Write,
        B: AsRef<[u8]>,
    {
        self.validate_replacements(replacements.len())?;
        self.try_stream_replace_all_with(reader, writer, buffer_bytes, |matched, _, writer| {
            writer.write_all(replacements[matched.pattern_id().as_usize()].as_ref())
        })
    }

    /// Incrementally replace standard stream matches through a callback.
    pub fn try_stream_replace_all_with<R, W, F>(
        &self,
        mut reader: R,
        mut writer: W,
        buffer_bytes: usize,
        mut replacement: F,
    ) -> Result<(), StreamReplaceError>
    where
        R: Read,
        W: Write,
        F: FnMut(Match, &[u8], &mut W) -> io::Result<()>,
    {
        validate_buffer_size(buffer_bytes)?;
        let mut state = self.try_stream_state()?;
        let mut read_buffer = Vec::new();
        read_buffer
            .try_reserve_exact(buffer_bytes)
            .map_err(|_| MatchError::StreamBufferAllocation {
                requested: buffer_bytes,
            })?;
        read_buffer.resize(buffer_bytes, 0);
        let carry_bound = self.max_pattern_len().saturating_sub(1);
        let mut pending = Vec::new();
        pending
            .try_reserve(buffer_bytes.saturating_add(carry_bound))
            .map_err(|_| MatchError::StreamBufferAllocation {
                requested: buffer_bytes.saturating_add(carry_bound),
            })?;
        let mut pending_base = 0_usize;

        loop {
            let read = loop {
                match reader.read(&mut read_buffer) {
                    Ok(read) => break read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(StreamReplaceError::Read(error)),
                }
            };
            let end_of_stream = read == 0;
            pending.extend_from_slice(&read_buffer[..read]);
            let mut consumed = 0_usize;
            let mut write_error = None;
            self.try_stream_chunk(&mut state, &read_buffer[..read], end_of_stream, |matched| {
                if write_error.is_some() {
                    return;
                }
                let local_start = matched.start() - pending_base;
                let local_end = matched.end() - pending_base;
                if let Err(error) = writer.write_all(&pending[consumed..local_start]) {
                    write_error = Some(error);
                    return;
                }
                if let Err(error) = replacement(matched, &pending[local_start..local_end], &mut writer) {
                    write_error = Some(error);
                    return;
                }
                consumed = local_end;
            })?;
            if let Some(error) = write_error {
                return Err(StreamReplaceError::Write(error));
            }
            let safe_end = if end_of_stream {
                state.absolute_offset()
            } else {
                state.absolute_offset().saturating_sub(carry_bound)
            }
            .max(pending_base);
            let safe_local = safe_end - pending_base;
            if safe_local > consumed {
                writer
                    .write_all(&pending[consumed..safe_local])
                    .map_err(StreamReplaceError::Write)?;
            }
            let drain_local = safe_local.max(consumed);
            pending.drain(..drain_local);
            pending_base = pending_base
                .checked_add(drain_local)
                .ok_or(MatchError::StreamOffsetOverflow)?;
            if end_of_stream {
                return Ok(());
            }
        }
    }

    fn validate_replacements(&self, actual: usize) -> Result<(), MatchError> {
        let expected = self.pattern_count();
        if actual == expected {
            Ok(())
        } else {
            Err(MatchError::ReplacementCount { expected, actual })
        }
    }

    fn validate_string_boundaries(&self, haystack: &str) -> Result<(), MatchError> {
        for matched in self.try_find_iter(super::SearchInput::new(haystack.as_bytes()))? {
            if !haystack.is_char_boundary(matched.start()) || !haystack.is_char_boundary(matched.end()) {
                return Err(MatchError::InvalidUtf8Boundary {
                    start: matched.start(),
                    end: matched.end(),
                });
            }
        }
        Ok(())
    }
}

/// Error from streaming replacement setup, reading, or writing.
#[derive(Debug)]
pub enum StreamReplaceError {
    /// Matcher or buffer configuration is unsupported.
    Match(MatchError),
    /// Reading the source stream failed.
    Read(io::Error),
    /// Writing the transformed stream failed.
    Write(io::Error),
}

impl std::fmt::Display for StreamReplaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Match(error) => write!(formatter, "stream replacement setup failed: {error}"),
            Self::Read(error) => write!(formatter, "stream replacement read failed: {error}"),
            Self::Write(error) => write!(formatter, "stream replacement write failed: {error}"),
        }
    }
}

impl std::error::Error for StreamReplaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Match(error) => Some(error),
            Self::Read(error) | Self::Write(error) => Some(error),
        }
    }
}

impl From<MatchError> for StreamReplaceError {
    fn from(error: MatchError) -> Self {
        Self::Match(error)
    }
}

fn append_bounded(output: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Result<(), MatchError> {
    let actual = output
        .len()
        .checked_add(bytes.len())
        .ok_or(MatchError::ReplacementByteLimit {
            actual: usize::MAX,
            limit,
        })?;
    reserve_bounded(output, bytes.len(), limit)?;
    output.extend_from_slice(bytes);
    debug_assert_eq!(output.len(), actual);
    Ok(())
}

fn reserve_bounded(output: &mut Vec<u8>, additional: usize, limit: usize) -> Result<(), MatchError> {
    let actual = output
        .len()
        .checked_add(additional)
        .ok_or(MatchError::ReplacementByteLimit {
            actual: usize::MAX,
            limit,
        })?;
    if actual > limit {
        return Err(MatchError::ReplacementByteLimit { actual, limit });
    }
    output
        .try_reserve(additional)
        .map_err(|_| MatchError::ReplacementAllocation)
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read, Write};

    use super::*;
    use crate::aho_corasick::{Builder, MatchKind};

    #[test]
    fn byte_table_and_callback_replacement_use_canonical_matches() {
        let mut builder = Builder::new();
        builder.match_kind(MatchKind::LeftmostFirst);
        let matcher = builder
            .build([b"append".as_slice(), b"appendage".as_slice(), b"app".as_slice()])
            .unwrap();
        let haystack = b"append the app to the appendage";
        let replacements = [b"x".as_slice(), b"y".as_slice(), b"z".as_slice()];
        assert_eq!(
            matcher.replace_all_bytes(haystack, &replacements).unwrap(),
            b"x the z to the xage"
        );
        assert_eq!(
            matcher
                .replace_all_bytes_with(haystack, 1024, |matched, _| matched
                    .pattern_id()
                    .as_usize()
                    .to_string()
                    .into_bytes())
                .unwrap(),
            b"0 the 2 to the 0age"
        );
        assert_eq!(
            matcher.replace_all("append the app", &["x", "y", "z"]).unwrap(),
            "x the z"
        );
    }

    #[test]
    fn replacement_shape_cap_and_utf8_boundaries_are_typed_errors() {
        let matcher = AhoCorasick::new([b"a".as_slice()]).unwrap();
        assert_eq!(
            matcher.replace_all_bytes(b"a", &[] as &[&[u8]]).unwrap_err(),
            MatchError::ReplacementCount { expected: 1, actual: 0 }
        );
        assert!(matches!(
            matcher.replace_all_bytes_with_limit(b"a", &[b"long".as_slice()], 3),
            Err(MatchError::ReplacementByteLimit { .. })
        ));

        let split = AhoCorasick::new([b"\xA9".as_slice()]).unwrap();
        assert!(matches!(
            split.replace_all("©", &["x"]),
            Err(MatchError::InvalidUtf8Boundary { .. })
        ));
    }

    #[test]
    fn stream_replacement_matches_block_at_every_small_buffer_size() {
        let matcher = AhoCorasick::new([b"abcd".as_slice(), b"x".as_slice()]).unwrap();
        let haystack = b"xxabcdyyxabcd";
        let replacements = [b"Q".as_slice(), b"Z".as_slice()];
        let expected = matcher.replace_all_bytes(haystack, &replacements).unwrap();
        for buffer_bytes in 1..=8 {
            assert_eq!(
                matcher.stream_replacement_buffer_bound(buffer_bytes).unwrap(),
                buffer_bytes * 2 + matcher.max_pattern_len() - 1
            );
            let mut output = Vec::new();
            matcher
                .try_stream_replace_all_with_buffer(haystack.as_slice(), &mut output, &replacements, buffer_bytes)
                .unwrap();
            assert_eq!(output, expected, "buffer={buffer_bytes}");
        }

        let mut callback_output = Vec::new();
        matcher
            .try_stream_replace_all_with(
                haystack.as_slice(),
                &mut callback_output,
                2,
                |matched, bytes, writer| {
                    assert_eq!(bytes, &haystack[matched.start()..matched.end()]);
                    writer.write_all(replacements[matched.pattern_id().as_usize()])
                },
            )
            .unwrap();
        assert_eq!(callback_output, expected);
    }

    #[derive(Debug)]
    struct ReadFailure;

    impl Read for ReadFailure {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::TimedOut, "read timeout"))
        }
    }

    #[derive(Debug)]
    struct WriteFailure;

    impl Write for WriteFailure {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "write closed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stream_replacement_distinguishes_read_write_and_setup_errors() {
        let matcher = AhoCorasick::new([b"a".as_slice()]).unwrap();
        assert!(matches!(
            matcher.try_stream_replace_all(ReadFailure, Vec::new(), &[b"x".as_slice()]),
            Err(StreamReplaceError::Read(error)) if error.kind() == io::ErrorKind::TimedOut
        ));
        assert!(matches!(
            matcher.try_stream_replace_all(b"a".as_slice(), WriteFailure, &[b"x".as_slice()]),
            Err(StreamReplaceError::Write(error)) if error.kind() == io::ErrorKind::BrokenPipe
        ));

        let mut builder = Builder::new();
        builder.match_kind(MatchKind::LeftmostLongest);
        let leftmost = builder.build([b"a".as_slice()]).unwrap();
        assert!(matches!(
            leftmost.try_stream_replace_all(b"a".as_slice(), Vec::new(), &[b"x".as_slice()]),
            Err(StreamReplaceError::Match(MatchError::UnsupportedStream { .. }))
        ));
    }
}
