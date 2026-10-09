#![no_main]

use edgion_waf::aho_corasick::{
    Anchored, AutomatonKind, Builder, Match, MatchKind, SearchInput, StartKind,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(case) = Case::decode(data) else {
        return;
    };
    run_case(&case);
});

struct Case {
    patterns: Vec<Vec<u8>>,
    haystack: Vec<u8>,
    match_kind: MatchKind,
    ascii_case_insensitive: bool,
    anchored: Anchored,
    earliest: bool,
    range_start: usize,
    range_end: usize,
    chunk_size: usize,
}

impl Case {
    fn decode(data: &[u8]) -> Option<Self> {
        let (&options, mut rest) = data.split_first()?;
        let pattern_count = usize::from(options & 0x07);
        let mut patterns = Vec::with_capacity(pattern_count);
        for _ in 0..pattern_count {
            let (&length_byte, tail) = rest.split_first()?;
            let length = usize::from(length_byte & 0x0f);
            if tail.len() < length {
                return None;
            }
            patterns.push(tail[..length].to_vec());
            rest = &tail[length..];
        }
        let haystack = rest[..rest.len().min(512)].to_vec();
        let match_kind = match (options >> 3) % 3 {
            0 => MatchKind::Standard,
            1 => MatchKind::LeftmostFirst,
            _ => MatchKind::LeftmostLongest,
        };
        let first = data.get(1).copied().unwrap_or(0) as usize;
        let second = data.get(2).copied().unwrap_or(0) as usize;
        let left = first.min(second) % haystack.len().saturating_add(1);
        let right = first.max(second) % haystack.len().saturating_add(1);
        Some(Self {
            patterns,
            haystack,
            match_kind,
            ascii_case_insensitive: options & 0x20 != 0,
            anchored: if options & 0x40 == 0 {
                Anchored::No
            } else {
                Anchored::Yes
            },
            earliest: options & 0x80 != 0,
            range_start: left.min(right),
            range_end: left.max(right),
            chunk_size: data.get(3).copied().map_or(1, |value| usize::from(value % 31) + 1),
        })
    }
}

fn run_case(case: &Case) {
    let reference = build(case, AutomatonKind::SparseNfa, false);
    let input = search_input(case);
    let expected = tuples(reference.try_find_iter(input).expect("both start modes must be compiled"));
    let expected_overlapping = (case.match_kind == MatchKind::Standard && case.anchored == Anchored::No)
        .then(|| {
            tuples(
                reference
                    .try_find_overlapping_iter(input)
                    .expect("standard unanchored overlap must be supported"),
            )
        });
    let replacements: Vec<Vec<u8>> = (0..case.patterns.len())
        .map(|pattern_id| vec![b'R', pattern_id as u8])
        .collect();
    let expected_replacement = reference
        .replace_all_bytes_with_limit(&case.haystack, &replacements, 4096)
        .expect("bounded fuzz replacement must fit");

    for backend in [
        AutomatonKind::SparseNfa,
        AutomatonKind::ContiguousNfa,
        AutomatonKind::Dfa,
        AutomatonKind::Auto,
    ] {
        for prefilter in [false, true] {
            let matcher = build(case, backend, prefilter);
            assert_eq!(
                expected,
                tuples(matcher.try_find_iter(input).expect("both start modes must be compiled"))
            );
            if let Some(expected_overlapping) = &expected_overlapping {
                assert_eq!(
                    *expected_overlapping,
                    tuples(
                        matcher
                            .try_find_overlapping_iter(input)
                            .expect("standard unanchored overlap must be supported"),
                    )
                );
            }
            assert_eq!(
                expected_replacement,
                matcher
                    .replace_all_bytes_with_limit(&case.haystack, &replacements, 4096)
                    .expect("bounded fuzz replacement must fit")
            );
            let mut pattern_set = matcher.create_pattern_set();
            matcher
                .scan_pattern_set(&case.haystack, &mut pattern_set)
                .expect("matching pattern-set shape");
        }
    }

    if case.match_kind == MatchKind::Standard && case.patterns.iter().all(|pattern| !pattern.is_empty()) {
        let expected_stream = tuples(reference.find_iter(&case.haystack));
        let mut state = reference.try_stream_state().expect("supported standard stream");
        let mut actual_stream = Vec::new();
        for chunk in case.haystack.chunks(case.chunk_size) {
            reference
                .try_stream_chunk(&mut state, chunk, false, |matched| actual_stream.push(matched))
                .expect("valid stream state");
        }
        reference
            .try_stream_chunk(&mut state, &[], true, |matched| actual_stream.push(matched))
            .expect("end-of-stream flush");
        assert_eq!(expected_stream, tuples(actual_stream));
    }
}

fn build(case: &Case, backend: AutomatonKind, prefilter: bool) -> edgion_waf::aho_corasick::AhoCorasick {
    let mut builder = Builder::new();
    builder
        .automaton_kind(backend)
        .prefilter(prefilter)
        .match_kind(case.match_kind)
        .ascii_case_insensitive(case.ascii_case_insensitive)
        .start_kind(StartKind::Both);
    builder.build(&case.patterns).expect("bounded fuzz patterns must compile")
}

fn search_input(case: &Case) -> SearchInput<'_> {
    SearchInput::new(&case.haystack)
        .try_range(case.range_start, case.range_end)
        .expect("decoded range is valid")
        .anchored(case.anchored)
        .earliest(case.earliest)
}

fn tuples(matches: impl IntoIterator<Item = Match>) -> Vec<(usize, usize, usize)> {
    matches
        .into_iter()
        .map(|matched| (matched.pattern_id().as_usize(), matched.start(), matched.end()))
        .collect()
}
