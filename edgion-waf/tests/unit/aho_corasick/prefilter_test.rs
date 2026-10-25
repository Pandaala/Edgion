use super::*;

fn build(patterns: &[&[u8]], match_kind: MatchKind, ascii_case_insensitive: bool) -> Acceleration {
    let mut builder = AccelerationBuilder::new(true, ascii_case_insensitive, match_kind);
    for (index, pattern) in patterns.iter().copied().enumerate() {
        builder.add(PatternId::try_from_usize(index).unwrap(), pattern).unwrap();
    }
    builder.build().unwrap()
}

#[test]
fn empty_pattern_disables_all_acceleration() {
    let acceleration = build(&[b"abc", b""], MatchKind::Standard, false);
    assert_eq!(acceleration.kind(), AccelerationKind::None);
}

#[test]
fn start_bytes_find_the_earliest_possible_start() {
    let acceleration = build(&[b"alpha", b"beta"], MatchKind::Standard, false);
    assert_eq!(acceleration.kind(), AccelerationKind::StartBytes);
    let mut state = SearchState::default();
    assert_eq!(
        acceleration.find_candidate(b"zzzbeta", 0, false, &mut state),
        Candidate::Possible { start: 3, scanned: 4 }
    );
}

#[test]
fn safe_word_scan_preserves_first_byte_and_tail_boundaries() {
    for length in 0..=32 {
        for match_at in 0..length {
            let mut haystack = vec![b'z'; length];
            haystack[match_at] = b'b';
            assert_eq!(find_any_byte(&haystack, b"abc", false), Some(match_at));
        }
        assert_eq!(find_any_byte(&vec![b'z'; length], b"abc", false), None);
    }
    assert_eq!(find_any_byte(b"zzAzz", b"a", true), Some(2));
}

#[test]
fn rare_shift_never_starts_after_a_real_match() {
    let acceleration = build(&[b"aa!", b"bb!", b"cc!", b"dd!"], MatchKind::Standard, false);
    assert_eq!(acceleration.kind(), AccelerationKind::RareBytes);
    let mut state = SearchState::default();
    let candidate = acceleration.find_candidate(b"zzzzcc!", 0, false, &mut state);
    assert!(matches!(candidate, Candidate::Possible { start, .. } if start <= 4));
}

#[test]
fn packed_verification_rejects_shared_prefix_collisions() {
    let acceleration = build(&[b"abx", b"aby", b"abz"], MatchKind::LeftmostFirst, false);
    assert_eq!(acceleration.kind(), AccelerationKind::Packed);
    let mut state = SearchState::default();
    assert_eq!(
        acceleration.find_candidate(b"qqaby", 0, false, &mut state),
        Candidate::Exact {
            matched: Match::from_validated(PatternId::try_from_usize(1).unwrap(), 2, 5),
            scanned: 4,
        }
    );
}

#[test]
fn rare_candidates_preserve_all_bytes_folding_and_rollback() {
    fn check(patterns: &[&[u8]], needles: &[u8], rollback: usize, folded: bool) {
        let mut builder = AccelerationBuilder::new(true, folded, MatchKind::Standard);
        for (index, pattern) in patterns.iter().enumerate() {
            builder.add(PatternId::try_from_usize(index).unwrap(), pattern).unwrap();
        }
        let acceleration = builder.build_rare().unwrap();
        assert_eq!(acceleration.kind(), AccelerationKind::RareBytes);
        let haystack: Vec<u8> = (u8::MIN..=u8::MAX).collect();
        for end in [0, 1, 8, 32, 128, 256] {
            for at in 0..=end {
                let expected = if at == end {
                    Candidate::Possible { start: at, scanned: 0 }
                } else {
                    haystack[at..end]
                        .iter()
                        .position(|byte| needles.contains(&if folded { byte.to_ascii_lowercase() } else { *byte }))
                        .map_or(Candidate::None, |relative| Candidate::Possible {
                            start: (at + relative).saturating_sub(rollback).max(at),
                            scanned: relative + 1,
                        })
                };
                assert_eq!(
                    acceleration.find_candidate(&haystack[..end], at, folded, &mut SearchState::default()),
                    expected,
                    "at={at}, end={end}, folded={folded}, needles={needles:?}"
                );
            }
        }
    }

    for folded in [false, true] {
        for byte in u8::MIN..=u8::MAX {
            let pattern = [byte];
            let needle = if folded { byte.to_ascii_lowercase() } else { byte };
            check(&[&pattern], &[needle], 0, folded);
        }
        check(&[b"a\0", b"ab\xFF", b"c!", b"abcd!"], &[0, b'!', 0xFF], 4, folded);
    }
}

#[test]
fn adaptivity_disables_dense_candidates_without_advancing_input() {
    let acceleration = build(&[b"aa", b"ab"], MatchKind::Standard, false);
    let mut state = SearchState::default();
    let haystack = [b'a'; 64];
    for at in 0..ADAPT_MIN_ATTEMPTS as usize {
        let candidate = acceleration.find_candidate(&haystack, at, false, &mut state);
        assert!(matches!(candidate, Candidate::Possible { start, .. } if start == at));
    }
    assert!(state.disabled);
    assert_eq!(state.skipped_bytes, 0);
}

fn two_gram_patterns() -> Vec<Vec<u8>> {
    let rare = b"!#$%&'()*+,-./:;<>?@[]^_`{|}~";
    (0..17)
        .map(|index| vec![b'a' + index as u8, rare[index], b'X', b'0' + (index % 10) as u8])
        .collect()
}

#[test]
fn two_gram_is_a_bounded_possible_only_fallback() {
    let patterns = two_gram_patterns();
    let refs = patterns.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let acceleration = build(&refs, MatchKind::Standard, false);
    assert_eq!(acceleration.kind(), AccelerationKind::TwoGram);
    assert_eq!(acceleration.memory_usage(), TWO_GRAM_TABLE_LEN);

    for pattern in &patterns {
        let mut haystack = b"zzzz".to_vec();
        haystack.extend_from_slice(pattern);
        let mut state = SearchState::default();
        assert!(matches!(
            acceleration.find_candidate(&haystack, 0, false, &mut state),
            Candidate::Possible { start, .. } if start <= 4
        ));
    }
}

#[test]
fn two_gram_respects_ascii_folding_and_short_pattern_safety() {
    let patterns = two_gram_patterns();
    let refs = patterns.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let acceleration = build(&refs, MatchKind::Standard, true);
    assert_eq!(acceleration.kind(), AccelerationKind::TwoGram);
    let mut state = SearchState::default();
    assert!(matches!(
        acceleration.find_candidate(b"ZZZA!x0", 0, true, &mut state),
        Candidate::Possible { start, .. } if start <= 3
    ));

    let mut short = refs.clone();
    short.push(b"q");
    assert_ne!(
        build(&short, MatchKind::Standard, false).kind(),
        AccelerationKind::TwoGram
    );
}
