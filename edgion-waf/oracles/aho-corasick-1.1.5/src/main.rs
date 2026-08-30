use std::error::Error;

use aho_corasick::{
    AhoCorasick as UpstreamAhoCorasick, Anchored as UpstreamAnchored, Input as UpstreamInput,
    MatchKind as UpstreamMatchKind, StartKind as UpstreamStartKind,
};
use edgion_waf::aho_corasick::{
    AhoCorasick, Anchored, AutomatonKind, Builder, MatchKind, SearchInput, StartKind,
};

type NormalizedMatch = (usize, usize, usize);

fn main() -> Result<(), Box<dyn Error>> {
    let mut random_state = 0xAC15_1150_CAFE_BABE_u64;
    for case_index in 0..5_000 {
        let pattern_count = (next_random(&mut random_state) % 7) as usize;
        let patterns: Vec<Vec<u8>> = (0..pattern_count).map(|_| random_bytes(&mut random_state, 6)).collect();
        let haystack = random_bytes(&mut random_state, 32);
        let ascii_case_insensitive = next_random(&mut random_state) & 1 == 1;

        compare_case(
            case_index,
            &patterns,
            &haystack,
            ascii_case_insensitive,
            MatchKind::Standard,
            UpstreamMatchKind::Standard,
        )?;
        compare_case(
            case_index,
            &patterns,
            &haystack,
            ascii_case_insensitive,
            MatchKind::LeftmostFirst,
            UpstreamMatchKind::LeftmostFirst,
        )?;
        compare_case(
            case_index,
            &patterns,
            &haystack,
            ascii_case_insensitive,
            MatchKind::LeftmostLongest,
            UpstreamMatchKind::LeftmostLongest,
        )?;
    }
    println!(
        "matched upstream 1.1.5 across 5000 deterministic block, range, anchor, earliest, stream, and replacement cases"
    );
    Ok(())
}

fn compare_case(
    case_index: usize,
    patterns: &[Vec<u8>],
    haystack: &[u8],
    ascii_case_insensitive: bool,
    edgion_kind: MatchKind,
    upstream_kind: UpstreamMatchKind,
) -> Result<(), Box<dyn Error>> {
    let mut upstream_builder = UpstreamAhoCorasick::builder();
    upstream_builder
        .match_kind(upstream_kind)
        .ascii_case_insensitive(ascii_case_insensitive)
        .start_kind(UpstreamStartKind::Both);
    let upstream = upstream_builder.build(patterns)?;

    let upstream_non_overlapping = upstream_matches(&upstream, haystack);
    let upstream_overlapping =
        (edgion_kind == MatchKind::Standard).then(|| upstream_overlapping_matches(&upstream, haystack));
    let range_start = case_index % (haystack.len() + 1);
    let range_end = range_start + (case_index / 7) % (haystack.len() - range_start + 1);

    for prefilter in [false, true] {
        for automaton_kind in [
            AutomatonKind::SparseNfa,
            AutomatonKind::ContiguousNfa,
            AutomatonKind::Dfa,
            AutomatonKind::Auto,
        ] {
            let mut edgion_builder = Builder::new();
            edgion_builder
                .match_kind(edgion_kind)
                .ascii_case_insensitive(ascii_case_insensitive)
                .automaton_kind(automaton_kind)
                .start_kind(StartKind::Both)
                .prefilter(prefilter);
            let edgion = edgion_builder.build(patterns)?;

            assert_eq!(
                edgion_matches(&edgion, haystack),
                upstream_non_overlapping,
                "non-overlapping mismatch: case={case_index}, kind={edgion_kind:?}, backend={automaton_kind:?}, prefilter={prefilter}, case_insensitive={ascii_case_insensitive}, patterns={patterns:?}, haystack={haystack:?}"
            );
            if let Some(upstream_overlapping) = &upstream_overlapping {
                assert_eq!(
                    edgion_overlapping_matches(&edgion, haystack)?,
                    *upstream_overlapping,
                    "overlapping mismatch: case={case_index}, backend={automaton_kind:?}, prefilter={prefilter}, case_insensitive={ascii_case_insensitive}, patterns={patterns:?}, haystack={haystack:?}"
                );
            }

            for earliest in [false, true] {
                for (upstream_anchor, edgion_anchor) in [
                    (UpstreamAnchored::No, Anchored::No),
                    (UpstreamAnchored::Yes, Anchored::Yes),
                ] {
                    let upstream_input = UpstreamInput::new(haystack)
                        .range(range_start..range_end)
                        .anchored(upstream_anchor)
                        .earliest(earliest);
                    let upstream_range = upstream
                        .try_find_iter(upstream_input)?
                        .map(|matched| (matched.pattern().as_usize(), matched.start(), matched.end()))
                        .collect::<Vec<_>>();
                    let edgion_input = SearchInput::new(haystack)
                        .try_range(range_start, range_end)?
                        .anchored(edgion_anchor)
                        .earliest(earliest);
                    let edgion_range = edgion
                        .try_find_iter(edgion_input)?
                        .map(|matched| (matched.pattern_id().as_usize(), matched.start(), matched.end()))
                        .collect::<Vec<_>>();
                    assert_eq!(
                        edgion_range, upstream_range,
                        "range mismatch: case={case_index}, range={range_start}..{range_end}, anchor={edgion_anchor:?}, earliest={earliest}, kind={edgion_kind:?}, backend={automaton_kind:?}, prefilter={prefilter}, patterns={patterns:?}, haystack={haystack:?}"
                    );
                }
            }

            if edgion_kind == MatchKind::Standard && patterns.iter().all(|pattern| !pattern.is_empty()) {
                let upstream_stream = upstream
                    .try_stream_find_iter(haystack)?
                    .map(|result| {
                        result.map(|matched| {
                            (matched.pattern().as_usize(), matched.start(), matched.end())
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let edgion_stream = edgion
                    .try_stream_find_iter_with_buffer(haystack, 1)?
                    .map(|result| {
                        result.map(|matched| {
                            (matched.pattern_id().as_usize(), matched.start(), matched.end())
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                assert_eq!(
                    edgion_stream, upstream_stream,
                    "stream mismatch: case={case_index}, backend={automaton_kind:?}, prefilter={prefilter}, patterns={patterns:?}, haystack={haystack:?}"
                );
            }
        }
    }

    if !patterns.is_empty() {
        let replacements: Vec<Vec<u8>> = (0..patterns.len())
            .map(|index| vec![b'0' + (index % 10) as u8])
            .collect();
        assert_eq!(
            edgion_replacement(patterns, haystack, ascii_case_insensitive, edgion_kind, &replacements)?,
            upstream.replace_all_bytes(haystack, &replacements),
            "replacement mismatch: case={case_index}, kind={edgion_kind:?}, patterns={patterns:?}, haystack={haystack:?}"
        );
    }
    Ok(())
}

fn edgion_replacement(
    patterns: &[Vec<u8>],
    haystack: &[u8],
    ascii_case_insensitive: bool,
    match_kind: MatchKind,
    replacements: &[Vec<u8>],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut builder = Builder::new();
    builder
        .match_kind(match_kind)
        .ascii_case_insensitive(ascii_case_insensitive);
    Ok(builder.build(patterns)?.replace_all_bytes(haystack, replacements)?)
}

fn edgion_matches(matcher: &AhoCorasick, haystack: &[u8]) -> Vec<NormalizedMatch> {
    matcher
        .find_iter(haystack)
        .map(|matched| (matched.pattern_id().as_usize(), matched.start(), matched.end()))
        .collect()
}

fn edgion_overlapping_matches(matcher: &AhoCorasick, haystack: &[u8]) -> Result<Vec<NormalizedMatch>, Box<dyn Error>> {
    Ok(matcher
        .try_find_overlapping_iter(haystack)?
        .map(|matched| (matched.pattern_id().as_usize(), matched.start(), matched.end()))
        .collect())
}

fn upstream_matches(matcher: &UpstreamAhoCorasick, haystack: &[u8]) -> Vec<NormalizedMatch> {
    matcher
        .find_iter(haystack)
        .map(|matched| (matched.pattern().as_usize(), matched.start(), matched.end()))
        .collect()
}

fn upstream_overlapping_matches(matcher: &UpstreamAhoCorasick, haystack: &[u8]) -> Vec<NormalizedMatch> {
    matcher
        .find_overlapping_iter(haystack)
        .map(|matched| (matched.pattern().as_usize(), matched.start(), matched.end()))
        .collect()
}

fn random_bytes(state: &mut u64, exclusive_max_len: u64) -> Vec<u8> {
    let length = (next_random(state) % exclusive_max_len) as usize;
    (0..length)
        .map(|_| match next_random(state) % 5 {
            0 => b'a',
            1 => b'b',
            2 => b'A',
            3 => 0,
            _ => 0xFF,
        })
        .collect()
}

fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    *state
}
