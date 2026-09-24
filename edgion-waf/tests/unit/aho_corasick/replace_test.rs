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
