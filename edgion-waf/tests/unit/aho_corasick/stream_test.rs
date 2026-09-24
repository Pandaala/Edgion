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
