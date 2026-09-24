use std::cmp::Reverse;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OracleKind {
    Standard,
    LeftmostFirst,
    LeftmostLongest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OracleMatch {
    pattern_id: usize,
    start: usize,
    end: usize,
}

fn naive_occurrences(patterns: &[&[u8]], haystack: &[u8]) -> Vec<OracleMatch> {
    let mut occurrences = Vec::new();
    for end in 0..=haystack.len() {
        for (pattern_id, pattern) in patterns.iter().enumerate() {
            if pattern.len() <= end && haystack[end - pattern.len()..end] == **pattern {
                occurrences.push(OracleMatch {
                    pattern_id,
                    start: end - pattern.len(),
                    end,
                });
            }
        }
    }
    occurrences
}

fn oracle_find(occurrences: &[OracleMatch], search_start: usize, kind: OracleKind) -> Option<OracleMatch> {
    let candidates = occurrences
        .iter()
        .copied()
        .filter(|matched| matched.start >= search_start);
    match kind {
        OracleKind::Standard => {
            candidates.min_by_key(|matched| (matched.end, Reverse(matched.end - matched.start), matched.pattern_id))
        }
        OracleKind::LeftmostFirst => candidates.min_by_key(|matched| (matched.start, matched.pattern_id)),
        OracleKind::LeftmostLongest => {
            candidates.min_by_key(|matched| (matched.start, Reverse(matched.end - matched.start), matched.pattern_id))
        }
    }
}

fn oracle_non_overlapping(patterns: &[&[u8]], haystack: &[u8], kind: OracleKind) -> Vec<OracleMatch> {
    let occurrences = naive_occurrences(patterns, haystack);
    let mut matches = Vec::new();
    let mut search_start = 0_usize;
    let mut last_match_end = None;
    while let Some(mut matched) = oracle_find(&occurrences, search_start, kind) {
        if matched.start == matched.end && Some(matched.end) == last_match_end {
            let Some(next_start) = search_start.checked_add(1) else {
                break;
            };
            search_start = next_start;
            let Some(next) = oracle_find(&occurrences, search_start, kind) else {
                break;
            };
            matched = next;
        }
        search_start = matched.end;
        last_match_end = Some(matched.end);
        matches.push(matched);
    }
    matches
}

fn oracle_overlapping(patterns: &[&[u8]], haystack: &[u8]) -> Vec<OracleMatch> {
    let mut occurrences = naive_occurrences(patterns, haystack);
    occurrences.sort_by_key(|matched| (matched.end, Reverse(matched.end - matched.start), matched.pattern_id));
    occurrences
}

fn oracle_tuples(matches: &[OracleMatch]) -> Vec<(usize, usize, usize)> {
    matches
        .iter()
        .map(|matched| (matched.pattern_id, matched.start, matched.end))
        .collect()
}

fn match_tuples(matches: impl IntoIterator<Item = Match>) -> Vec<(usize, usize, usize)> {
    matches
        .into_iter()
        .map(|matched| (matched.pattern_id().as_usize(), matched.start(), matched.end()))
        .collect()
}

fn build_with_kind(patterns: &[&[u8]], match_kind: MatchKind) -> AhoCorasick {
    let mut builder = Builder::new();
    builder.match_kind(match_kind);
    builder.build(patterns).unwrap()
}

fn build_contiguous_with_kind(patterns: &[&[u8]], match_kind: MatchKind) -> AhoCorasick {
    build_contiguous_with_options(patterns, match_kind, false, 0, BuildLimits::default()).unwrap()
}

fn build_contiguous_with_options(
    patterns: &[&[u8]],
    match_kind: MatchKind,
    ascii_case_insensitive: bool,
    dense_depth: usize,
    limits: BuildLimits,
) -> Result<AhoCorasick, BuildError> {
    let (backend, acceleration) =
        Backend::build_contiguous(patterns, limits, ascii_case_insensitive, dense_depth, match_kind, false)?;
    Ok(AhoCorasick {
        backend,
        acceleration,
        ascii_case_insensitive,
        match_kind,
        selection_reason: SelectionReason::Explicit,
        start_kind: StartKind::Unanchored,
        generation: next_generation()?,
    })
}

#[test]
fn naive_oracle_freezes_order_and_empty_progress() {
    let patterns = [b"".as_slice(), b"a".as_slice(), b"".as_slice()];

    assert_eq!(
        oracle_tuples(&oracle_overlapping(&patterns, b"a")),
        vec![(0, 0, 0), (2, 0, 0), (1, 0, 1), (0, 1, 1), (2, 1, 1)]
    );
    assert_eq!(
        oracle_tuples(&oracle_non_overlapping(&patterns, b"a", OracleKind::Standard)),
        vec![(0, 0, 0), (0, 1, 1)]
    );
    assert_eq!(
        oracle_tuples(&oracle_non_overlapping(&patterns, b"a", OracleKind::LeftmostFirst,)),
        vec![(0, 0, 0), (0, 1, 1)]
    );
    assert_eq!(
        oracle_tuples(&oracle_non_overlapping(&patterns, b"a", OracleKind::LeftmostLongest,)),
        vec![(1, 0, 1)]
    );
}

#[test]
fn naive_oracle_freezes_leftmost_ties() {
    let patterns = [b"ab".as_slice(), b"abcd".as_slice(), b"ab".as_slice()];

    assert_eq!(
        oracle_tuples(&oracle_non_overlapping(&patterns, b"abcd", OracleKind::LeftmostFirst,)),
        vec![(0, 0, 2)]
    );
    assert_eq!(
        oracle_tuples(&oracle_non_overlapping(&patterns, b"abcd", OracleKind::LeftmostLongest,)),
        vec![(1, 0, 4)]
    );
}

#[test]
fn value_types_validate_boundaries() {
    let pattern_id = PatternId::try_from_usize(7).unwrap();
    let span = Span::try_new(3, 9).unwrap();
    let matched = Match::try_new(pattern_id, 3, 9).unwrap();

    assert_eq!(pattern_id.as_usize(), 7);
    assert_eq!(span.len(), 6);
    assert_eq!(matched.span(), span);
    if let Some(too_large) = (u32::MAX as usize).checked_add(1) {
        assert_eq!(PatternId::try_from_usize(too_large).unwrap_err().value(), too_large);
    }
    assert_eq!(
        Span::try_new(9, 3).unwrap_err().to_string(),
        "match span start 9 exceeds end 3; provide a half-open span with start <= end"
    );
}

#[test]
fn empty_pattern_list_never_matches() {
    let matcher = AhoCorasick::new(Vec::<&[u8]>::new()).unwrap();
    let mut matches = matcher.create_pattern_set();

    assert!(!matcher.is_match(b"anything"));
    assert_eq!(matcher.find(b"anything"), None);
    matcher.scan_pattern_set(b"anything", &mut matches).unwrap();
    assert!(matches.is_empty());
    assert_eq!(matcher.min_pattern_len(), None);
    assert_eq!(matcher.max_pattern_len(), 0);
}

#[test]
fn find_reports_earliest_end_with_direct_output_priority() {
    let matcher = AhoCorasick::new([b"he".as_slice(), b"she".as_slice()]).unwrap();

    let matched = matcher.find(b"ushers").unwrap();
    assert_eq!(matched.pattern_id().as_usize(), 1);
    assert_eq!(matched.span(), Span::try_new(1, 4).unwrap());
}

#[test]
fn standard_iterators_match_ordered_oracle_without_empty_patterns() {
    let patterns = [b"abcd".as_slice(), b"bcd".as_slice(), b"cd".as_slice(), b"b".as_slice()];
    let matcher = AhoCorasick::new(patterns).unwrap();

    assert_eq!(
        match_tuples(matcher.find_iter(b"abcd")),
        oracle_tuples(&oracle_non_overlapping(&patterns, b"abcd", OracleKind::Standard,))
    );
    assert_eq!(
        match_tuples(matcher.try_find_overlapping_iter(b"abcd").unwrap()),
        oracle_tuples(&oracle_overlapping(&patterns, b"abcd"))
    );
}

#[test]
fn contiguous_flat_sparse_layout_matches_sparse_backend() {
    let patterns = [
        b"".as_slice(),
        b"abcd".as_slice(),
        b"bcd".as_slice(),
        b"cd".as_slice(),
        b"b".as_slice(),
        b"abcd".as_slice(),
    ];
    let haystack = b"zabcdabcd";
    for match_kind in [
        MatchKind::Standard,
        MatchKind::LeftmostFirst,
        MatchKind::LeftmostLongest,
    ] {
        let sparse = build_with_kind(&patterns, match_kind);
        let contiguous = build_contiguous_with_kind(&patterns, match_kind);
        assert_eq!(
            match_tuples(contiguous.find_iter(haystack)),
            match_tuples(sparse.find_iter(haystack))
        );
        assert_eq!(contiguous.backend.dense_state_count(), 0);
        if match_kind == MatchKind::Standard {
            assert_eq!(
                match_tuples(contiguous.try_find_overlapping_iter(haystack).unwrap()),
                match_tuples(sparse.try_find_overlapping_iter(haystack).unwrap())
            );
        }
    }
}

#[test]
fn contiguous_dense_root_matches_sparse_for_every_byte() {
    let patterns: Vec<[u8; 1]> = (u8::MIN..=u8::MAX).map(|byte| [byte]).collect();
    let pattern_refs: Vec<&[u8]> = patterns.iter().map(<[u8; 1]>::as_slice).collect();
    let haystack: Vec<u8> = (u8::MIN..=u8::MAX).rev().collect();
    let sparse = build_with_kind(&pattern_refs, MatchKind::Standard);
    let contiguous =
        build_contiguous_with_options(&pattern_refs, MatchKind::Standard, false, 2, BuildLimits::default()).unwrap();

    assert_eq!(sparse.backend.dense_state_count(), 1);
    assert_eq!(contiguous.backend.dense_state_count(), 1);
    assert_eq!(
        match_tuples(contiguous.try_find_overlapping_iter(&haystack).unwrap()),
        match_tuples(sparse.try_find_overlapping_iter(&haystack).unwrap())
    );
}

#[test]
fn contiguous_dense_policy_and_memory_limit_are_deterministic() {
    let low_fanout = [b"alpha".as_slice(), b"beta".as_slice()];
    let contiguous =
        build_contiguous_with_options(&low_fanout, MatchKind::Standard, false, 16, BuildLimits::default()).unwrap();
    assert_eq!(contiguous.backend.dense_state_count(), 0);

    let required = contiguous.memory_usage();
    let error = build_contiguous_with_options(
        &low_fanout,
        MatchKind::Standard,
        false,
        16,
        BuildLimits {
            max_compiled_bytes: required - 1,
            ..BuildLimits::default()
        },
    )
    .unwrap_err();
    assert!(matches!(error, BuildError::CompiledByteLimit { .. }));
}

#[test]
fn public_backend_selection_is_explicit_and_sparse_by_default() {
    let sparse = AhoCorasick::new([b"a".as_slice()]).unwrap();
    assert_eq!(sparse.automaton_kind(), AutomatonKind::SparseNfa);
    assert_eq!(sparse.selection_reason(), SelectionReason::Explicit);
    assert_eq!(sparse.byte_class_count(), 256);
    assert_eq!(sparse.dense_state_count(), 0);
    assert!(sparse.estimated_peak_build_memory_usage() >= sparse.memory_usage());

    let patterns: Vec<[u8; 1]> = (u8::MIN..=u8::MAX).map(|byte| [byte]).collect();
    let mut builder = Builder::new();
    builder.automaton_kind(AutomatonKind::ContiguousNfa);
    let contiguous = builder.build(&patterns).unwrap();
    assert_eq!(contiguous.automaton_kind(), AutomatonKind::ContiguousNfa);
    assert_eq!(contiguous.selection_reason(), SelectionReason::Explicit);
    assert_eq!(contiguous.dense_state_count(), 1);
    assert!(contiguous.estimated_peak_build_memory_usage() >= contiguous.memory_usage());

    let mut dfa_builder = Builder::new();
    dfa_builder.automaton_kind(AutomatonKind::Dfa);
    let dfa = dfa_builder.build([b"a".as_slice(), b"z".as_slice()]).unwrap();
    assert_eq!(dfa.automaton_kind(), AutomatonKind::Dfa);
    assert_eq!(dfa.selection_reason(), SelectionReason::Explicit);
    assert_eq!(dfa.byte_class_count(), 3);
    assert_eq!(dfa.state_count(), 3);
    assert!(dfa.estimated_peak_build_memory_usage() >= dfa.memory_usage());
}

#[test]
fn explicit_dfa_cell_limit_fails_while_auto_uses_deterministic_fallbacks() {
    let mut explicit = Builder::new();
    explicit.automaton_kind(AutomatonKind::Dfa).limits(BuildLimits {
        max_dfa_cells: 3,
        ..BuildLimits::default()
    });
    assert_eq!(
        explicit.build([b"a".as_slice()]).unwrap_err(),
        BuildError::DfaCellLimit { actual: 4, limit: 3 }
    );

    let mut auto = Builder::new();
    auto.automaton_kind(AutomatonKind::Auto);
    let dfa = auto.build([b"a".as_slice(), b"b".as_slice()]).unwrap();
    assert_eq!(dfa.automaton_kind(), AutomatonKind::Dfa);
    assert_eq!(dfa.selection_reason(), SelectionReason::AutoDfaWithinPolicy);

    auto.limits(BuildLimits {
        max_dfa_cells: 0,
        ..BuildLimits::default()
    });
    let sparse = auto.build([b"a".as_slice(), b"b".as_slice()]).unwrap();
    assert_eq!(sparse.automaton_kind(), AutomatonKind::SparseNfa);
    assert_eq!(sparse.selection_reason(), SelectionReason::AutoSparseDfaTooLarge);

    let root_patterns: Vec<[u8; 1]> = (0..16).map(|byte| [byte]).collect();
    let contiguous = auto.build(&root_patterns).unwrap();
    assert_eq!(contiguous.automaton_kind(), AutomatonKind::ContiguousNfa);
    assert_eq!(
        contiguous.selection_reason(),
        SelectionReason::AutoContiguousDfaTooLarge
    );
    assert_eq!(contiguous.dense_state_count(), 1);
}

#[test]
fn dfa_exact_cell_and_compiled_byte_limits_are_deterministic() {
    let patterns = [b"he".as_slice(), b"she".as_slice(), b"hers".as_slice()];
    let mut builder = Builder::new();
    builder.automaton_kind(AutomatonKind::Dfa);
    let baseline = builder.build(patterns).unwrap();
    let cells = baseline.state_count() * baseline.byte_class_count();
    let bytes = baseline.memory_usage();

    builder.limits(BuildLimits {
        max_dfa_cells: cells,
        max_compiled_bytes: bytes,
        ..BuildLimits::default()
    });
    let exact = builder.build(patterns).unwrap();
    assert_eq!(exact.memory_usage(), bytes);

    builder.limits(BuildLimits {
        max_dfa_cells: cells - 1,
        ..BuildLimits::default()
    });
    assert_eq!(
        builder.build(patterns).unwrap_err(),
        BuildError::DfaCellLimit {
            actual: cells,
            limit: cells - 1,
        }
    );

    builder.limits(BuildLimits {
        max_compiled_bytes: bytes - 1,
        ..BuildLimits::default()
    });
    assert!(matches!(
        builder.build(patterns),
        Err(BuildError::CompiledByteLimit {
            actual,
            limit
        }) if actual == bytes && limit == bytes - 1
    ));
}

#[test]
fn auto_selection_is_repeatable_and_ascii_classes_preserve_matching() {
    let patterns = [b"Alpha".as_slice(), b"BETA".as_slice()];
    let mut builder = Builder::new();
    builder.automaton_kind(AutomatonKind::Auto).ascii_case_insensitive(true);
    let first = builder.build(patterns).unwrap();
    let second = builder.build(patterns).unwrap();

    assert_eq!(first.automaton_kind(), second.automaton_kind());
    assert_eq!(first.selection_reason(), second.selection_reason());
    assert_eq!(first.state_count(), second.state_count());
    assert_eq!(first.byte_class_count(), second.byte_class_count());
    assert_eq!(first.memory_usage(), second.memory_usage());
    assert_eq!(
        match_tuples(first.find_iter(b"xxALPHA-beta")),
        vec![(0, 2, 7), (1, 8, 12)]
    );
}

#[test]
fn auto_policy_cap_declines_dfa_without_speculative_table_allocation() {
    let patterns: Vec<[u8; 2]> = (0_u8..16)
        .flat_map(|first| (u8::MIN..=u8::MAX).map(move |second| [first, second]))
        .collect();
    let mut builder = Builder::new();
    builder.automaton_kind(AutomatonKind::Auto);
    let matcher = builder.build(&patterns).unwrap();

    assert_eq!(matcher.automaton_kind(), AutomatonKind::ContiguousNfa);
    assert_eq!(matcher.selection_reason(), SelectionReason::AutoContiguousDfaTooLarge);
    assert_eq!(matcher.state_count(), 4_113);
    assert!(matcher.state_count() * 256 > AUTO_MAX_DFA_CELLS);
    assert!(matcher.memory_usage() < AUTO_MAX_DFA_CELLS * std::mem::size_of::<u32>());
}

#[test]
fn excessive_dense_depth_is_rejected_without_backend_fallback() {
    let mut builder = Builder::new();
    builder
        .automaton_kind(AutomatonKind::ContiguousNfa)
        .dense_depth(MAX_DENSE_DEPTH + 1);

    assert_eq!(
        builder.build([b"a".as_slice()]).unwrap_err(),
        BuildError::DenseDepthLimit {
            actual: MAX_DENSE_DEPTH + 1,
            limit: MAX_DENSE_DEPTH,
        }
    );
}

#[test]
fn standard_iterators_are_fused() {
    let matcher = AhoCorasick::new([b"a".as_slice()]).unwrap();
    let mut non_overlapping = matcher.find_iter(b"a");
    let mut overlapping = matcher.try_find_overlapping_iter(b"a").unwrap();

    assert!(non_overlapping.next().is_some());
    assert_eq!(non_overlapping.next(), None);
    assert_eq!(non_overlapping.next(), None);
    assert!(overlapping.next().is_some());
    assert_eq!(overlapping.next(), None);
    assert_eq!(overlapping.next(), None);
}

#[test]
fn leftmost_modes_resolve_start_length_and_source_order_ties() {
    let patterns = [b"ab".as_slice(), b"abcd".as_slice(), b"ab".as_slice()];
    let first = build_with_kind(&patterns, MatchKind::LeftmostFirst);
    let longest = build_with_kind(&patterns, MatchKind::LeftmostLongest);

    assert_eq!(first.match_kind(), MatchKind::LeftmostFirst);
    assert_eq!(match_tuples(first.find_iter(b"abcd")), vec![(0, 0, 2)]);
    assert_eq!(match_tuples(longest.find_iter(b"abcd")), vec![(1, 0, 4)]);

    let shifted = [b"abcdefg".as_slice(), b"bcde".as_slice(), b"bcdef".as_slice()];
    let first = build_with_kind(&shifted, MatchKind::LeftmostFirst);
    let longest = build_with_kind(&shifted, MatchKind::LeftmostLongest);
    assert_eq!(match_tuples(first.find_iter(b"abcdef")), vec![(1, 1, 5)]);
    assert_eq!(match_tuples(longest.find_iter(b"abcdef")), vec![(2, 1, 6)]);
}

#[test]
fn leftmost_empty_pattern_selection_matches_oracle() {
    let patterns = [b"".as_slice(), b"a".as_slice(), b"".as_slice()];
    for (match_kind, oracle_kind) in [
        (MatchKind::LeftmostFirst, OracleKind::LeftmostFirst),
        (MatchKind::LeftmostLongest, OracleKind::LeftmostLongest),
    ] {
        let matcher = build_with_kind(&patterns, match_kind);
        assert_eq!(
            match_tuples(matcher.find_iter(b"a")),
            oracle_tuples(&oracle_non_overlapping(&patterns, b"a", oracle_kind))
        );
    }
}

#[test]
fn overlapping_search_rejects_leftmost_modes() {
    for match_kind in [MatchKind::LeftmostFirst, MatchKind::LeftmostLongest] {
        let matcher = build_with_kind(&[b"a".as_slice()], match_kind);
        assert_eq!(
            matcher.try_find_overlapping_iter(b"a").unwrap_err(),
            MatchError::UnsupportedOverlapping { match_kind }
        );
    }
}

#[test]
fn find_n_enforces_the_caller_capture_bound_lazily() {
    let matcher = AhoCorasick::new([b"a".as_slice()]).unwrap();

    assert!(match_tuples(matcher.find_n(b"aaaa", 0)).is_empty());
    assert_eq!(match_tuples(matcher.find_n(b"aaaa", 2)), vec![(0, 0, 1), (0, 1, 2)]);
}

#[test]
fn all_match_kinds_match_the_naive_ordered_oracle() {
    let mut random_state = 0xC0DE_5EED_u64;
    for _ in 0..1_000 {
        let pattern_count = (next_random(&mut random_state) % 6 + 1) as usize;
        let patterns: Vec<Vec<u8>> = (0..pattern_count)
            .map(|_| {
                let length = (next_random(&mut random_state) % 6) as usize;
                (0..length)
                    .map(|_| b'a' + (next_random(&mut random_state) % 3) as u8)
                    .collect()
            })
            .collect();
        let pattern_refs: Vec<&[u8]> = patterns.iter().map(Vec::as_slice).collect();
        let haystack_len = (next_random(&mut random_state) % 24) as usize;
        let haystack: Vec<u8> = (0..haystack_len)
            .map(|_| b'a' + (next_random(&mut random_state) % 3) as u8)
            .collect();

        for (match_kind, oracle_kind) in [
            (MatchKind::Standard, OracleKind::Standard),
            (MatchKind::LeftmostFirst, OracleKind::LeftmostFirst),
            (MatchKind::LeftmostLongest, OracleKind::LeftmostLongest),
        ] {
            let matcher = build_with_kind(&pattern_refs, match_kind);
            assert_eq!(
                match_tuples(matcher.find_iter(&haystack)),
                oracle_tuples(&oracle_non_overlapping(&pattern_refs, &haystack, oracle_kind)),
                "patterns={patterns:?}, haystack={haystack:?}, match_kind={match_kind:?}"
            );
            if match_kind == MatchKind::Standard {
                assert_eq!(
                    match_tuples(matcher.try_find_overlapping_iter(&haystack).unwrap()),
                    oracle_tuples(&oracle_overlapping(&pattern_refs, &haystack)),
                    "patterns={patterns:?}, haystack={haystack:?}"
                );
            }
        }
    }
}

#[test]
fn all_backends_match_on_random_byte_cases() {
    let mut random_state = 0xC011_71C0_05E5_u64;
    for _ in 0..2_000 {
        let pattern_count = (next_random(&mut random_state) % 9) as usize;
        let patterns: Vec<Vec<u8>> = (0..pattern_count)
            .map(|_| {
                let length = (next_random(&mut random_state) % 9) as usize;
                (0..length).map(|_| random_match_byte(&mut random_state)).collect()
            })
            .collect();
        let pattern_refs: Vec<&[u8]> = patterns.iter().map(Vec::as_slice).collect();
        let haystack_len = (next_random(&mut random_state) % 50) as usize;
        let haystack: Vec<u8> = (0..haystack_len)
            .map(|_| random_match_byte(&mut random_state))
            .collect();
        let ascii_case_insensitive = next_random(&mut random_state) & 1 == 1;
        let dense_depth = (next_random(&mut random_state) % 5) as usize;

        for match_kind in [
            MatchKind::Standard,
            MatchKind::LeftmostFirst,
            MatchKind::LeftmostLongest,
        ] {
            let mut sparse_builder = Builder::new();
            sparse_builder
                .match_kind(match_kind)
                .ascii_case_insensitive(ascii_case_insensitive)
                .automaton_kind(AutomatonKind::SparseNfa)
                .prefilter(false);
            let sparse = sparse_builder.build(&pattern_refs).unwrap();

            sparse_builder.prefilter(true);
            let accelerated_sparse = sparse_builder.build(&pattern_refs).unwrap();

            let mut contiguous_builder = Builder::new();
            contiguous_builder
                .match_kind(match_kind)
                .ascii_case_insensitive(ascii_case_insensitive)
                .automaton_kind(AutomatonKind::ContiguousNfa)
                .dense_depth(dense_depth);
            let contiguous = contiguous_builder.build(&pattern_refs).unwrap();

            let mut dfa_builder = Builder::new();
            dfa_builder
                .match_kind(match_kind)
                .ascii_case_insensitive(ascii_case_insensitive)
                .automaton_kind(AutomatonKind::Dfa);
            let dfa = dfa_builder.build(&pattern_refs).unwrap();

            let mut auto_builder = Builder::new();
            auto_builder
                .match_kind(match_kind)
                .ascii_case_insensitive(ascii_case_insensitive)
                .automaton_kind(AutomatonKind::Auto)
                .dense_depth(dense_depth);
            let auto = auto_builder.build(&pattern_refs).unwrap();

            let expected = match_tuples(sparse.find_iter(&haystack));
            for matcher in [&accelerated_sparse, &contiguous, &dfa, &auto] {
                assert_eq!(
                        match_tuples(matcher.find_iter(&haystack)),
                        expected,
                        "patterns={patterns:?}, haystack={haystack:?}, match_kind={match_kind:?}, backend={:?}, case_insensitive={ascii_case_insensitive}, dense_depth={dense_depth}",
                        matcher.automaton_kind()
                    );
            }
            if match_kind == MatchKind::Standard {
                let expected = match_tuples(sparse.try_find_overlapping_iter(&haystack).unwrap());
                for matcher in [&accelerated_sparse, &contiguous, &dfa, &auto] {
                    assert_eq!(
                            match_tuples(matcher.try_find_overlapping_iter(&haystack).unwrap()),
                            expected,
                            "patterns={patterns:?}, haystack={haystack:?}, backend={:?}, case_insensitive={ascii_case_insensitive}, dense_depth={dense_depth}",
                            matcher.automaton_kind()
                        );
                }
            }

            let mut sparse_set = sparse.create_pattern_set();
            let mut accelerated_sparse_set = accelerated_sparse.create_pattern_set();
            let mut contiguous_set = contiguous.create_pattern_set();
            let mut dfa_set = dfa.create_pattern_set();
            let mut auto_set = auto.create_pattern_set();
            sparse.scan_pattern_set(&haystack, &mut sparse_set).unwrap();
            accelerated_sparse
                .scan_pattern_set(&haystack, &mut accelerated_sparse_set)
                .unwrap();
            contiguous.scan_pattern_set(&haystack, &mut contiguous_set).unwrap();
            dfa.scan_pattern_set(&haystack, &mut dfa_set).unwrap();
            auto.scan_pattern_set(&haystack, &mut auto_set).unwrap();
            assert_eq!(sparse_set, contiguous_set);
            assert_eq!(sparse_set, accelerated_sparse_set);
            assert_eq!(sparse_set, dfa_set);
            assert_eq!(sparse_set, auto_set);
            assert!(contiguous.memory_usage() <= BuildLimits::default().max_compiled_bytes);
            assert_eq!(contiguous.backend.stored_output_count(), contiguous.pattern_count());
            assert_eq!(dfa.backend.stored_output_count(), dfa.pattern_count());
            assert_eq!(auto.backend.stored_output_count(), auto.pattern_count());
        }
    }
}

fn random_match_byte(state: &mut u64) -> u8 {
    match next_random(state) % 7 {
        0 => b'a',
        1 => b'b',
        2 => b'A',
        3 => b'B',
        4 => 0,
        5 => 0xFF,
        _ => b'z',
    }
}

#[test]
fn ascii_case_policy_is_byte_oriented() {
    let mut builder = Builder::new();
    builder.ascii_case_insensitive(true);
    let matcher = builder.build([b"Admin".as_slice(), b"\xFFx".as_slice()]).unwrap();

    assert!(matcher.is_match(b"prefix ADMIN suffix"));
    assert!(matcher.is_match(b"\xFFX"));
    assert!(!matcher.is_match(b"\xDFx"));
}

#[test]
fn search_input_rejects_every_invalid_range_shape() {
    let haystack = b"abc";
    assert_eq!(
        SearchInput::new(haystack).try_range(2, 1),
        Err(MatchError::InvalidRange {
            start: 2,
            end: 1,
            haystack_len: 3,
        })
    );
    assert!(matches!(
        SearchInput::new(haystack).try_range(0, 4),
        Err(MatchError::InvalidRange { .. })
    ));
    let empty_end = SearchInput::new(haystack).try_range(3, 3).unwrap();
    assert_eq!((empty_end.start(), empty_end.end()), (3, 3));
}

#[test]
fn ranges_keep_absolute_spans_and_bound_lookahead() {
    let patterns = [b"b".as_slice(), b"abcd".as_slice(), b"abc".as_slice()];
    let matcher = AhoCorasick::new(patterns).unwrap();
    let input = SearchInput::new(b"xxabcdyy").try_range(2, 5).unwrap();

    assert_eq!(
        matcher.try_find(input).unwrap(),
        Some(Match::from_validated(PatternId::try_from_usize(0).unwrap(), 3, 4))
    );
    assert_eq!(match_tuples(matcher.try_find_iter(input).unwrap()), vec![(0, 3, 4)]);
}

#[test]
fn earliest_input_overrides_leftmost_lookahead() {
    let patterns = [b"abc".as_slice(), b"b".as_slice()];
    let mut builder = Builder::new();
    builder.match_kind(MatchKind::LeftmostFirst);
    let matcher = builder.build(patterns).unwrap();
    assert_eq!(matcher.find(b"abc").unwrap().pattern_id().as_usize(), 0);
    assert_eq!(
        matcher
            .try_find(SearchInput::new(b"abc").earliest(true))
            .unwrap()
            .unwrap()
            .pattern_id()
            .as_usize(),
        1
    );
}

#[test]
fn anchor_support_and_semantics_match_across_backends() {
    let patterns = [
        b"b".as_slice(),
        b"abcd".as_slice(),
        b"abc".as_slice(),
        b"foo".as_slice(),
        b"bar".as_slice(),
    ];
    for automaton_kind in [
        AutomatonKind::SparseNfa,
        AutomatonKind::ContiguousNfa,
        AutomatonKind::Dfa,
        AutomatonKind::Auto,
    ] {
        let mut builder = Builder::new();
        builder.automaton_kind(automaton_kind).start_kind(StartKind::Both);
        let matcher = builder.build(patterns).unwrap();
        let anchored = SearchInput::new(b"xxabcd")
            .try_range(2, 6)
            .unwrap()
            .anchored(Anchored::Yes);
        assert_eq!(
            matcher.try_find(anchored).unwrap(),
            Some(Match::from_validated(PatternId::try_from_usize(2).unwrap(), 2, 5)),
            "backend={automaton_kind:?}"
        );

        let adjacent = SearchInput::new(b"foobarXfoo").anchored(Anchored::Yes);
        assert_eq!(
            match_tuples(matcher.try_find_iter(adjacent).unwrap()),
            vec![(3, 0, 3), (0, 3, 4)]
        );
        assert_eq!(
            matcher.try_find_overlapping_iter(anchored).unwrap_err(),
            MatchError::UnsupportedAnchoredOverlapping
        );
    }

    let unanchored_only = AhoCorasick::new([b"a".as_slice()]).unwrap();
    assert!(matches!(
        unanchored_only.try_find(SearchInput::new(b"a").anchored(Anchored::Yes)),
        Err(MatchError::UnsupportedAnchored { .. })
    ));
    let mut builder = Builder::new();
    builder.start_kind(StartKind::Anchored);
    let anchored_only = builder.build([b"a".as_slice()]).unwrap();
    assert!(matches!(
        anchored_only.try_find(SearchInput::new(b"a")),
        Err(MatchError::UnsupportedAnchored { .. })
    ));
}

#[test]
fn anchored_pattern_set_contains_only_range_start_matches() {
    let patterns = [b"b".as_slice(), b"abc".as_slice(), b"bc".as_slice()];
    let mut builder = Builder::new();
    builder.start_kind(StartKind::Both);
    let matcher = builder.build(patterns).unwrap();
    let mut matches = matcher.create_pattern_set();
    let input = SearchInput::new(b"xabc")
        .try_range(1, 4)
        .unwrap()
        .anchored(Anchored::Yes);
    matcher.scan_pattern_set_input(input, &mut matches).unwrap();
    assert_eq!(matches.iter().map(PatternId::as_usize).collect::<Vec<_>>(), vec![1]);
}

#[test]
fn acceleration_selection_is_explicit_bounded_and_empty_safe() {
    let patterns = [b"alpha".as_slice(), b"beta".as_slice()];
    let mut baseline_builder = Builder::new();
    baseline_builder.prefilter(false);
    let baseline = baseline_builder.build(patterns).unwrap();
    assert_eq!(baseline.acceleration_kind(), AccelerationKind::None);
    assert_eq!(baseline.acceleration_memory_usage(), 0);

    let mut builder = Builder::new();
    builder.prefilter(true);
    let start = builder.build(patterns).unwrap();
    assert_eq!(start.acceleration_kind(), AccelerationKind::StartBytes);
    assert!(start.acceleration_memory_usage() > 0);

    let rare = builder
        .build([
            b"aa!".as_slice(),
            b"bb!".as_slice(),
            b"cc!".as_slice(),
            b"dd!".as_slice(),
        ])
        .unwrap();
    assert_eq!(rare.acceleration_kind(), AccelerationKind::RareBytes);

    builder.match_kind(MatchKind::LeftmostFirst);
    let packed = builder
        .build([b"abx".as_slice(), b"aby".as_slice(), b"abz".as_slice()])
        .unwrap();
    assert_eq!(packed.acceleration_kind(), AccelerationKind::Packed);

    let empty = builder.build([b"abc".as_slice(), b"".as_slice()]).unwrap();
    assert_eq!(empty.acceleration_kind(), AccelerationKind::None);
}

#[test]
fn pattern_set_diagnostics_report_skips_and_adaptive_disablement() {
    let patterns = [b"alpha".as_slice(), b"beta".as_slice()];
    let matcher = AhoCorasick::new(patterns).unwrap();
    let mut matches = matcher.create_pattern_set();

    let sparse = matcher
        .scan_pattern_set_with_diagnostics(&vec![b'z'; 256], &mut matches)
        .unwrap();
    assert_eq!(sparse.candidate_attempts(), 1);
    assert_eq!(sparse.candidate_scanned_bytes(), 256);
    assert_eq!(sparse.skipped_bytes(), 0);
    assert!(!sparse.prefilter_disabled());

    let dense = matcher
        .scan_pattern_set_with_diagnostics(&b"az".repeat(128), &mut matches)
        .unwrap();
    assert!(dense.candidate_attempts() >= 32);
    assert!(dense.candidate_scanned_bytes() >= usize::from(dense.candidate_attempts()));
    assert!(dense.prefilter_disabled());
}

#[test]
fn acceleration_is_included_in_the_compiled_byte_limit() {
    let patterns = [b"alpha".as_slice(), b"beta".as_slice()];
    let mut builder = Builder::new();
    builder.prefilter(true);
    let baseline = builder.build(patterns).unwrap();
    let required = baseline.memory_usage();

    builder.limits(BuildLimits {
        max_compiled_bytes: required,
        ..BuildLimits::default()
    });
    assert_eq!(builder.build(patterns).unwrap().memory_usage(), required);

    builder.limits(BuildLimits {
        max_compiled_bytes: required - 1,
        ..BuildLimits::default()
    });
    assert!(matches!(
        builder.build(patterns),
        Err(BuildError::CompiledByteLimit { actual, limit })
            if actual == required && limit == required - 1
    ));
}

fn two_gram_test_patterns() -> Vec<Vec<u8>> {
    let rare = b"!#$%&'()*+,-./:;<>?@[]^_`{|}~";
    (0..17)
        .map(|index| vec![b'a' + index as u8, rare[index], b'X', b'0' + (index % 10) as u8])
        .collect()
}

#[test]
fn two_gram_prefilter_preserves_all_public_search_semantics() {
    let patterns = two_gram_test_patterns();
    let pattern_refs = patterns.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let mut haystack = b"prefix-".to_vec();
    haystack.extend_from_slice(&patterns[9]);
    haystack.extend_from_slice(b"-middle-");
    haystack.extend_from_slice(&patterns[2]);
    haystack.extend_from_slice(b"-suffix");

    for match_kind in [
        MatchKind::Standard,
        MatchKind::LeftmostFirst,
        MatchKind::LeftmostLongest,
    ] {
        for automaton_kind in [
            AutomatonKind::SparseNfa,
            AutomatonKind::ContiguousNfa,
            AutomatonKind::Dfa,
            AutomatonKind::Auto,
        ] {
            let mut builder = Builder::new();
            builder
                .match_kind(match_kind)
                .automaton_kind(automaton_kind)
                .prefilter(false);
            let baseline = builder.build(&pattern_refs).unwrap();
            builder.prefilter(true);
            let accelerated = builder.build(&pattern_refs).unwrap();
            assert_eq!(accelerated.acceleration_kind(), AccelerationKind::TwoGram);
            assert_eq!(accelerated.acceleration_memory_usage(), prefilter::TWO_GRAM_TABLE_LEN);
            assert_eq!(
                match_tuples(accelerated.find_iter(&haystack)),
                match_tuples(baseline.find_iter(&haystack)),
                "match_kind={match_kind:?}, backend={automaton_kind:?}"
            );

            let input = SearchInput::new(&haystack).try_range(3, haystack.len() - 2).unwrap();
            assert_eq!(
                match_tuples(accelerated.try_find_iter(input).unwrap()),
                match_tuples(baseline.try_find_iter(input).unwrap())
            );
            if match_kind == MatchKind::Standard {
                assert_eq!(
                    match_tuples(accelerated.try_find_overlapping_iter(input).unwrap()),
                    match_tuples(baseline.try_find_overlapping_iter(input).unwrap())
                );
            }
            let mut expected = baseline.create_pattern_set();
            let mut actual = accelerated.create_pattern_set();
            baseline.scan_pattern_set_input(input, &mut expected).unwrap();
            accelerated.scan_pattern_set_input(input, &mut actual).unwrap();
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn two_gram_memory_is_charged_to_the_compiled_limit() {
    let patterns = two_gram_test_patterns();
    let pattern_refs = patterns.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let mut builder = Builder::new();
    builder.prefilter(true);
    let required = builder.build(&pattern_refs).unwrap().memory_usage();

    builder.limits(BuildLimits {
        max_compiled_bytes: required,
        ..BuildLimits::default()
    });
    assert_eq!(builder.build(&pattern_refs).unwrap().memory_usage(), required);
    builder.limits(BuildLimits {
        max_compiled_bytes: required - 1,
        ..BuildLimits::default()
    });
    assert!(matches!(
        builder.build(&pattern_refs),
        Err(BuildError::CompiledByteLimit { actual, limit })
            if actual == required && limit == required - 1
    ));
}

#[test]
fn rare_prefilter_global_rollback_preserves_earlier_cross_pattern_match() {
    let patterns = [
        b"aaaaaaaaaa!".as_slice(),
        b"b#".as_slice(),
        b"c#".as_slice(),
        b"d#".as_slice(),
    ];
    let haystack = b"aaaaaaaaaa!#";
    let mut builder = Builder::new();
    builder.prefilter(true);
    let matcher = builder.build(patterns).unwrap();

    assert_eq!(matcher.acceleration_kind(), AccelerationKind::RareBytes);
    assert_eq!(
        matcher.find(haystack),
        Some(Match::from_validated(PatternId::try_from_usize(0).unwrap(), 0, 11))
    );
}

#[test]
fn every_byte_value_can_be_a_transition() {
    let patterns: Vec<[u8; 1]> = (u8::MIN..=u8::MAX).map(|byte| [byte]).collect();
    let haystack: Vec<u8> = (u8::MIN..=u8::MAX).collect();
    let matcher = AhoCorasick::new(&patterns).unwrap();
    let mut matches = matcher.create_pattern_set();

    matcher.scan_pattern_set(&haystack, &mut matches).unwrap();
    assert_eq!(matches.iter().count(), 256);
}

#[test]
fn pattern_set_reports_duplicates_and_suffixes() {
    let matcher = AhoCorasick::new([b"a".as_slice(), b"aa".as_slice(), b"aaa".as_slice(), b"aaa".as_slice()]).unwrap();
    let mut matches = matcher.create_pattern_set();

    matcher.scan_pattern_set(b"aaa", &mut matches).unwrap();
    assert_eq!(
        matches.iter().map(PatternId::as_usize).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(matcher.backend.stored_output_count(), matcher.pattern_count());
}

#[test]
fn pattern_set_reuse_preserves_capacity_and_clears_results() {
    let matcher = AhoCorasick::new([b"attack".as_slice()]).unwrap();
    let mut matches = matcher.create_pattern_set();
    let capacity = matches.word_capacity();
    let allocation = matches.word_allocation_ptr();

    matcher.scan_pattern_set(b"attack", &mut matches).unwrap();
    assert!(!matches.is_empty());
    matcher.scan_pattern_set(b"normal", &mut matches).unwrap();
    assert!(matches.is_empty());
    assert_eq!(matches.word_capacity(), capacity);
    assert_eq!(matches.word_allocation_ptr(), allocation);
}

#[test]
fn mismatched_pattern_set_shape_is_rejected() {
    let first = AhoCorasick::new([b"a".as_slice()]).unwrap();
    let second = AhoCorasick::new([b"a".as_slice(), b"b".as_slice()]).unwrap();
    let mut matches = first.create_pattern_set();

    assert_eq!(
        second.scan_pattern_set(b"ab", &mut matches).unwrap_err(),
        MatchError::PatternSetShape { expected: 2, actual: 1 }
    );
}

#[test]
fn coraza_pm_capture_shape_uses_bounded_leftmost_longest_iteration() {
    let patterns = [b"a".as_slice(), b"ab".as_slice(), b"b".as_slice()];
    let mut builder = Builder::new();
    builder
        .match_kind(MatchKind::LeftmostLongest)
        .ascii_case_insensitive(true);
    let matcher = builder.build(patterns).unwrap();

    assert_eq!(
        match_tuples(matcher.find_n(b"ABaba", 10)),
        vec![(1, 0, 2), (1, 2, 4), (0, 4, 5)]
    );
}

#[test]
fn build_limits_are_enforced_before_growth() {
    let mut builder = Builder::new();
    builder.limits(BuildLimits {
        max_patterns: 1,
        max_pattern_bytes: 3,
        max_states: 3,
        max_transitions: 2,
        max_outputs: 1,
        max_compiled_bytes: usize::MAX,
        max_dfa_cells: usize::MAX,
    });

    assert_eq!(
        builder.build([b"a".as_slice(), b"b".as_slice()]).unwrap_err(),
        BuildError::PatternLimit { actual: 2, limit: 1 }
    );
    assert_eq!(
        builder.build([b"abcd".as_slice()]).unwrap_err(),
        BuildError::PatternByteLimit { actual: 4, limit: 3 }
    );
    assert_eq!(
        builder.build([b"abc".as_slice()]).unwrap_err(),
        BuildError::StateLimit { actual: 4, limit: 3 }
    );

    let mut transition_builder = Builder::new();
    transition_builder.limits(BuildLimits {
        max_transitions: 1,
        ..BuildLimits::default()
    });
    assert_eq!(
        transition_builder.build([b"ab".as_slice()]).unwrap_err(),
        BuildError::TransitionLimit { actual: 2, limit: 1 }
    );

    let mut output_builder = Builder::new();
    output_builder.limits(BuildLimits {
        max_outputs: 1,
        ..BuildLimits::default()
    });
    assert_eq!(
        output_builder
            .build([b"same".as_slice(), b"same".as_slice()])
            .unwrap_err(),
        BuildError::OutputLimit { actual: 2, limit: 1 }
    );

    let mut compiled_byte_builder = Builder::new();
    compiled_byte_builder.limits(BuildLimits {
        max_compiled_bytes: 0,
        ..BuildLimits::default()
    });
    assert!(matches!(
        compiled_byte_builder.build([b"a".as_slice()]),
        Err(BuildError::CompiledByteLimit { limit: 0, .. })
    ));
}

#[test]
fn compiled_memory_usage_stays_within_the_accepted_limit() {
    let matcher = AhoCorasick::new([b"alpha".as_slice(), b"alphabet".as_slice(), b"beta".as_slice()]).unwrap();

    assert!(matcher.memory_usage() > 0);
    assert!(matcher.memory_usage() <= BuildLimits::default().max_compiled_bytes);
}

#[test]
fn pattern_set_can_be_reused_for_an_equal_pattern_count() {
    let first = AhoCorasick::new([b"a".as_slice(), b"b".as_slice()]).unwrap();
    let second = AhoCorasick::new([b"x".as_slice(), b"y".as_slice()]).unwrap();
    let mut matches = first.create_pattern_set();

    second.scan_pattern_set(b"y", &mut matches).unwrap();
    assert_eq!(matches.iter().map(PatternId::as_usize).collect::<Vec<_>>(), vec![1]);
}

#[test]
fn matcher_and_pattern_set_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<AhoCorasick>();
    assert_send_sync::<PatternSet>();
}

#[test]
fn empty_patterns_report_at_every_boundary_without_losing_duplicates() {
    let patterns = [b"".as_slice(), b"a".as_slice(), b"".as_slice()];
    let matcher = AhoCorasick::new(patterns).unwrap();
    let mut pattern_set = matcher.create_pattern_set();

    assert_eq!(matcher.min_pattern_len(), Some(0));
    assert_eq!(matcher.find(b"a").unwrap().span(), Span::try_new(0, 0).unwrap());
    assert_eq!(
        match_tuples(matcher.find_iter(b"a")),
        oracle_tuples(&oracle_non_overlapping(&patterns, b"a", OracleKind::Standard))
    );
    assert_eq!(
        match_tuples(matcher.try_find_overlapping_iter(b"a").unwrap()),
        oracle_tuples(&oracle_overlapping(&patterns, b"a"))
    );
    matcher.scan_pattern_set(b"a", &mut pattern_set).unwrap();
    assert_eq!(
        pattern_set.iter().map(PatternId::as_usize).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(matcher.backend.stored_output_count(), matcher.pattern_count());
}

#[test]
fn scalar_search_matches_naive_reference() {
    let patterns = [
        b"a".as_slice(),
        b"ab".as_slice(),
        b"bab".as_slice(),
        b"bc".as_slice(),
        b"bca".as_slice(),
        b"c".as_slice(),
        b"caa".as_slice(),
    ];
    let matcher = AhoCorasick::new(patterns).unwrap();
    let mut matches = matcher.create_pattern_set();
    let mut random_state = 0xA5A5_1234_u64;

    for _ in 0..2_000 {
        let length = (next_random(&mut random_state) % 64) as usize;
        let haystack: Vec<u8> = (0..length)
            .map(|_| b'a' + (next_random(&mut random_state) % 4) as u8)
            .collect();
        matcher.scan_pattern_set(&haystack, &mut matches).unwrap();
        let actual: Vec<usize> = matches.iter().map(PatternId::as_usize).collect();
        let expected: Vec<usize> = patterns
            .iter()
            .enumerate()
            .filter_map(|(index, pattern)| {
                haystack
                    .windows(pattern.len())
                    .any(|window| window == *pattern)
                    .then_some(index)
            })
            .collect();
        assert_eq!(actual, expected, "haystack={haystack:?}");

        let expected_first = (1..=haystack.len()).find_map(|end| {
            patterns
                .iter()
                .enumerate()
                .filter(|(_, pattern)| haystack[..end].ends_with(pattern))
                .max_by_key(|(index, pattern)| (pattern.len(), usize::MAX - index))
                .map(|(index, pattern)| (index, end - pattern.len(), end))
        });
        let actual_first = matcher
            .find(&haystack)
            .map(|matched| (matched.pattern_id().as_usize(), matched.start(), matched.end()));
        assert_eq!(actual_first, expected_first, "haystack={haystack:?}");
    }
}

fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    *state
}
