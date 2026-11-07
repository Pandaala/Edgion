use super::*;

fn ids(results: MatchResults<'_>) -> Vec<usize> {
    results.iter().map(PatternId::as_usize).collect()
}

impl CompiledMatcher {
    pub(crate) fn test_regex_counts(&self) -> (usize, usize) {
        (self.regex_pattern_ids.len(), self.regex_physical_pattern_count())
    }
    pub(crate) fn test_guard_stats(&self) -> (usize, usize) {
        self.regex
            .as_ref()
            .map_or((0, 0), CompiledRegexProgram::test_guard_stats)
    }
    pub(crate) fn test_simple_count(&self) -> usize {
        self.regex.as_ref().map_or(0, CompiledRegexProgram::test_simple_count)
    }
}

impl RegexCaptureScratch {
    /// (VM cache built, capture slots built).
    pub(crate) fn test_built(&self) -> (bool, bool) {
        (self.cache.is_some(), self.captures.is_some())
    }
}

impl CompiledRegexProgram {
    pub(crate) fn test_simple_count(&self) -> usize {
        usize::from(matches!(self.execution_hint, RegexExecutionHint::Simple(_)))
    }
    pub(crate) fn test_guard_stats(&self) -> (usize, usize) {
        let RegexExecutionHint::Guard(guards) = &self.execution_hint else {
            return (0, 0);
        };
        (
            guards.len(),
            guards.iter().map(AhoCorasick::memory_usage).sum::<usize>() + guards.capacity() * size_of::<AhoCorasick>(),
        )
    }
}

#[test]
fn exact_simple_regex_matches_original_vm_for_anchors_empty_and_raw_bytes() {
    assert_eq!(size_of::<RegexExecutionHint>(), 32);
    for source in [
        "alpha",
        "^alpha",
        "alpha$",
        "^alpha$",
        "",
        "^",
        "$",
        "^$",
        "\\Aalpha\\z",
        "abc\\x00",
        "\\xFFabc",
        "écho",
        "(?s:alpha)",
        "1234567890123456",
    ] {
        let mut program = CompiledRegexProgram::compile_one_pcre_bytes_with_peak_budget(
            source,
            RegexProgramLimits::default(),
            usize::MAX,
        )
        .unwrap();
        let before = program.memory_usage();
        let (proof, installed) = program.lower_simple(guard_budget()).unwrap();
        assert!(installed, "source={source}");
        assert_eq!(proof.owned, 0);
        assert_eq!(program.memory_usage(), before);
        assert!(proof.work <= 64);
        let mut scratch = program.create_scratch();
        let mut oracle = program.core.vm.create_cache();
        let mut seed = 57_u64;
        for length in 0..128 {
            let mut input = (0..length)
                .map(|_| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (seed >> 32) as u8
                })
                .collect::<Vec<_>>();
            for tail in [
                b"".as_slice(),
                b"alpha",
                b"alpha\n",
                b"ALPHA",
                b"abc\0",
                b"\xffabc",
                "écho".as_bytes(),
                b"1234567890123456",
            ] {
                let base = input.len();
                input.extend_from_slice(tail);
                assert_eq!(
                    program.is_match(&input, &mut scratch).unwrap(),
                    program.core.vm.is_match(&mut oracle, &input),
                    "source={source} length={length}"
                );
                input.truncate(base);
            }
        }
    }
}

#[test]
fn exact_simple_regex_declines_unsupported_shapes_and_admits_bounded_proof() {
    for source in [
        "(?m:^alpha)",
        "(?m:alpha$)",
        "\\balpha\\b",
        "alpha|beta",
        "alpha?",
        "alpha+",
        "[a-z]",
        "(?i:alpha)",
        "12345678901234567",
        "a$z",
    ] {
        let mut program = CompiledRegexProgram::compile_one_pcre_bytes_with_peak_budget(
            source,
            RegexProgramLimits::default(),
            usize::MAX,
        )
        .unwrap();
        let (proof, installed) = program.lower_simple(guard_budget()).unwrap();
        assert!(!installed, "source={source}");
        assert!(proof.work <= 64);
        assert!(matches!(program.execution_hint, RegexExecutionHint::None));
    }
    for budget in [
        RegexGuardBudget {
            work: 1,
            ..guard_budget()
        },
        RegexGuardBudget {
            peak: REGEX_SIMPLE_PROOF_BYTES - 1,
            ..guard_budget()
        },
    ] {
        let mut program = CompiledRegexProgram::compile_one("alpha", RegexProgramLimits::default()).unwrap();
        let (proof, installed) = program.lower_simple(budget).unwrap();
        assert!(!installed);
        assert!(proof.work <= budget.work);
        assert!(proof.peak <= budget.peak);
    }
}

#[test]
fn exact_simple_regex_preserves_physical_bit_fanout_and_foreign_owner_checks() {
    let patterns = [
        PatternSpec::Regex("^alpha$"),
        PatternSpec::Phrase {
            value: "alpha",
            ascii_case_insensitive: false,
        },
        PatternSpec::Regex("^alpha$"),
    ];
    let mut matcher = CompiledMatcher::compile_pcre_bytes(&patterns, MatcherLimits::default()).unwrap();
    assert!(
        matcher
            .regex_program_mut()
            .unwrap()
            .lower_simple(guard_budget())
            .unwrap()
            .1
    );
    let mut scratch = matcher.create_scratch();
    for input in [b"alpha".as_slice(), b"alpha\n", b"otheralpha", b"", b"alpha"] {
        let expected = ids(matcher.scan_reference(input, &mut scratch).unwrap());
        assert_eq!(ids(matcher.scan(input, &mut scratch).unwrap()), expected);
    }
    let foreign = CompiledRegexProgram::compile_one("other", RegexProgramLimits::default()).unwrap();
    let program = matcher.regex.as_ref().unwrap();
    assert_eq!(
        program.is_match(b"alpha", &mut foreign.create_scratch()),
        Err(RegexScratchError::ScratchShape)
    );
}

#[test]
fn exact_simple_regex_has_exact_work_and_peak_admission() {
    let compile = || CompiledRegexProgram::compile_one("alpha", RegexProgramLimits::default()).unwrap();
    let exact_work = compile().lower_simple(guard_budget()).unwrap().0.work;
    for (work, peak, installs) in [
        (exact_work, REGEX_SIMPLE_PROOF_BYTES, true),
        (exact_work - 1, REGEX_SIMPLE_PROOF_BYTES, false),
        (exact_work, REGEX_SIMPLE_PROOF_BYTES - 1, false),
    ] {
        let mut program = compile();
        let (proof, installed) = program
            .lower_simple(RegexGuardBudget {
                work,
                peak,
                ..guard_budget()
            })
            .unwrap();
        assert_eq!(installed, installs);
        assert!(proof.work <= work && proof.peak <= peak);
    }
}

#[test]
#[ignore = "explicit release timing witness"]
fn exact_simple_regex_release_witness() {
    for source in ["alpha", "^alpha", "alpha$", "^alpha$"] {
        let mut matcher =
            CompiledMatcher::compile_pcre_bytes(&[PatternSpec::Regex(source)], MatcherLimits::default()).unwrap();
        assert!(
            matcher
                .regex_program_mut()
                .unwrap()
                .lower_simple(guard_budget())
                .unwrap()
                .1
        );
        for bytes in [16, 1024, 8192] {
            for hit in [false, true] {
                let mut input = vec![b'z'; bytes];
                if hit {
                    if source == "^alpha$" {
                        input = b"alpha".to_vec();
                    } else if source.starts_with('^') {
                        input[..5].copy_from_slice(b"alpha");
                    } else {
                        input.extend_from_slice(b"alpha");
                    }
                }
                for reverse in [false, true] {
                    for accelerated in if reverse { [true, false] } else { [false, true] } {
                        let mut scratch = matcher.create_scratch();
                        let mut samples = Vec::new();
                        for sample in 0..24 {
                            let start = std::time::Instant::now();
                            for _ in 0..100 {
                                let results = if accelerated {
                                    matcher.scan(&input, &mut scratch)
                                } else {
                                    matcher.scan_reference(&input, &mut scratch)
                                }
                                .unwrap();
                                assert_eq!(results.contains_any_in_range(0..1), Some(hit));
                                std::hint::black_box(results);
                            }
                            if sample >= 3 {
                                samples.push(start.elapsed().as_nanos() / 100);
                            }
                        }
                        samples.sort_unstable();
                        println!("Simple timing source={source} input_bytes={} hit={hit} reverse={reverse} accelerated={accelerated} p50_ns={}", input.len(), samples[10]);
                    }
                }
            }
        }
    }
}

fn guard_budget() -> RegexGuardBudget {
    RegexGuardBudget {
        owned: 512 * 1024,
        peak: 8 * 1024 * 1024,
        work: 8192,
    }
}

#[test]
fn necessary_prefix_guard_preserves_original_vm_over_bytes_flags_and_branches() {
    for source in [
        "alpha[0-9]",
        "^alpha.*beta$",
        "alpha(?:x|y)",
        "(?:alpha)?beta",
        "alpha+",
        "(?i:alpha)",
        "écho.*",
        "\\xFFabc[0-9]",
        "\\x00abc",
        "",
        "a*",
        "alpha|beta",
        "abc{1000}",
    ] {
        let mut program = CompiledRegexProgram::compile_one_pcre_bytes_with_peak_budget(
            source,
            RegexProgramLimits::default(),
            usize::MAX,
        )
        .unwrap();
        let before = program.memory_usage();
        let outcome = program.add_literal_guard(guard_budget()).unwrap();
        assert_eq!(program.memory_usage(), before + outcome.owned);
        assert!(outcome.work <= 8192);
        assert!(outcome.peak <= guard_budget().peak);
        let mut scratch = program.create_scratch();
        let mut reference = program.core.vm.create_cache();
        let mut seed = 31_u64;
        for length in 0..256 {
            let mut input = (0..length)
                .map(|_| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (seed >> 32) as u8
                })
                .collect::<Vec<_>>();
            for tail in [
                b"".as_slice(),
                b"alpha1",
                b"alpha!",
                b"ALPHA2",
                b"beta",
                b"alpha\n\nbeta",
                "écho!".as_bytes(),
                b"\xffabc3",
                b"\x00abc",
            ] {
                let base = input.len();
                input.extend_from_slice(tail);
                assert_eq!(
                    program.is_match(&input, &mut scratch).unwrap(),
                    program.core.vm.is_match(&mut reference, &input),
                    "source={source} length={length}"
                );
                input.truncate(base);
            }
        }
    }
}

#[test]
fn necessary_prefix_guard_admits_proof_peak_and_preserves_optional_fallback() {
    let mut program = CompiledRegexProgram::compile_one("alpha[0-9]", RegexProgramLimits::default()).unwrap();
    for budget in [
        RegexGuardBudget {
            peak: REGEX_GUARD_PROOF_BYTES - 1,
            ..guard_budget()
        },
        RegexGuardBudget {
            work: 1,
            ..guard_budget()
        },
        RegexGuardBudget {
            owned: 0,
            ..guard_budget()
        },
        RegexGuardBudget {
            peak: REGEX_GUARD_PROOF_BYTES,
            ..guard_budget()
        },
    ] {
        let before = program.memory_usage();
        let outcome = program.add_literal_guard(budget).unwrap();
        assert_eq!(outcome.owned, 0);
        assert_eq!(program.memory_usage(), before);
        assert!(matches!(program.execution_hint, RegexExecutionHint::None));
        assert!(outcome.peak <= budget.peak);
        assert!(outcome.work <= budget.work);
        if budget.work != 0 && budget.peak >= REGEX_GUARD_PROOF_BYTES {
            assert_eq!(outcome.peak, REGEX_GUARD_PROOF_BYTES);
        }
    }
    assert!(program.add_literal_guard(guard_budget()).unwrap().owned > 0);
    let foreign = CompiledRegexProgram::compile_one("other[0-9]", RegexProgramLimits::default()).unwrap();
    assert_eq!(
        program.is_match(&vec![b'z'; 1024], &mut foreign.create_scratch()),
        Err(RegexScratchError::ScratchShape)
    );
}

#[test]
fn necessary_prefix_union_guard_keeps_duplicate_fanout_and_full_input_confirmation() {
    let patterns = [
        PatternSpec::Regex("alpha[0-9]"),
        PatternSpec::Regex("beta[0-9]"),
        PatternSpec::Regex("alpha[0-9]"),
    ];
    let mut matcher = CompiledMatcher::compile_pcre_bytes(&patterns, MatcherLimits::default()).unwrap();
    assert!(
        matcher
            .regex_program_mut()
            .unwrap()
            .add_literal_guard(guard_budget())
            .unwrap()
            .owned
            > 0
    );
    let mut scratch = matcher.create_scratch();
    for suffix in [b"alpha1".as_slice(), b"none", b"alpha!", b"beta2 alpha3"] {
        let mut input = vec![b'z'; 1024];
        input.extend_from_slice(suffix);
        let reference = ids(matcher.scan_reference(&input, &mut scratch).unwrap());
        assert_eq!(ids(matcher.scan(&input, &mut scratch).unwrap()), reference);
        if suffix == b"alpha!" {
            assert!(reference.is_empty());
        }
    }
    let guarded = matcher.regex.as_ref().unwrap();
    assert!(!guarded.may_match(&vec![b'z'; 1024]));
    assert!(guarded.may_match(b"small"));
}

#[test]
fn necessary_prefix_guard_declines_absolute_anchored_single_and_mixed_groups() {
    for sources in [vec!["^alpha[0-9]"], vec!["^alpha[0-9]", "beta[0-9]"]] {
        let patterns = sources
            .iter()
            .map(|source| PatternSpec::Regex(source))
            .collect::<Vec<_>>();
        let mut matcher = CompiledMatcher::compile_pcre_bytes(&patterns, MatcherLimits::default()).unwrap();
        let outcome = matcher
            .regex_program_mut()
            .unwrap()
            .add_literal_guard(guard_budget())
            .unwrap();
        assert_eq!(outcome.owned, 0);
        assert_eq!(matcher.test_guard_stats(), (0, 0));
        assert!(outcome.work > 0);
        assert_eq!(outcome.peak, REGEX_GUARD_PROOF_BYTES);
    }
    let mut multiline = CompiledRegexProgram::compile_one("(?m:^alpha[0-9])", RegexProgramLimits::default()).unwrap();
    assert!(multiline.add_literal_guard(guard_budget()).unwrap().owned > 0);
}

#[test]
fn necessary_prefix_guard_has_exact_optional_build_peak_admission() {
    let compile = || CompiledRegexProgram::compile_one("alpha[0-9]", RegexProgramLimits::default()).unwrap();
    let required = compile().add_literal_guard(guard_budget()).unwrap().peak;
    assert!(required >= REGEX_GUARD_PROOF_BYTES + REGEX_GUARD_ACCELERATION_BUILD_BYTES);
    for (peak, installs) in [(required, true), (required - 1, false)] {
        let mut program = compile();
        let outcome = program
            .add_literal_guard(RegexGuardBudget { peak, ..guard_budget() })
            .unwrap();
        assert_eq!(outcome.owned > 0, installs);
        assert!(outcome.peak <= peak);
        if !installs {
            assert_eq!(outcome.peak, REGEX_GUARD_PROOF_BYTES);
        }
    }
}

#[test]
fn necessary_prefix_guard_has_exact_optional_owned_admission() {
    let compile = || CompiledRegexProgram::compile_one("alpha[0-9]", RegexProgramLimits::default()).unwrap();
    let owned = compile().add_literal_guard(guard_budget()).unwrap().owned;
    assert!(owned > 0);
    for (limit, installs) in [(owned, true), (owned - 1, false)] {
        let mut program = compile();
        let before = program.memory_usage();
        let outcome = program
            .add_literal_guard(RegexGuardBudget {
                owned: limit,
                ..guard_budget()
            })
            .unwrap();
        assert_eq!(outcome.owned > 0, installs);
        assert_eq!(program.memory_usage(), before + outcome.owned);
        assert!(outcome.owned <= limit);
        if installs {
            assert_eq!(outcome.owned, owned);
        }
    }
}

#[test]
#[ignore = "explicit release timing witness"]
fn necessary_prefix_guard_release_witness() {
    let sources = ["alpha[0-9].*omega", "beta[0-9].*theta", "gamma[0-9].*delta"];
    let patterns = sources.map(PatternSpec::Regex);
    let mut matcher = CompiledMatcher::compile_pcre_bytes(&patterns, MatcherLimits::default()).unwrap();
    let cost = matcher
        .regex_program_mut()
        .unwrap()
        .add_literal_guard(guard_budget())
        .unwrap()
        .owned;
    assert!(cost > 0);
    for bytes in [64, 1024, 8192] {
        for case in ["miss", "late_hit", "late_false_positive", "dense_false_positive"] {
            let mut input = if case == "dense_false_positive" {
                b"alpha!beta!gamma!"
                    .iter()
                    .copied()
                    .cycle()
                    .take(bytes)
                    .collect::<Vec<_>>()
            } else {
                vec![b'z'; bytes]
            };
            match case {
                "late_hit" => input.extend_from_slice(b"alpha1 omega"),
                "late_false_positive" => input.extend_from_slice(b"alpha!beta!gamma!"),
                _ => {}
            }
            for reverse in [false, true] {
                for accelerated in if reverse { [true, false] } else { [false, true] } {
                    let mut scratch = matcher.create_scratch();
                    let mut samples = Vec::new();
                    for sample in 0..24 {
                        let start = std::time::Instant::now();
                        for _ in 0..100 {
                            let result = if accelerated {
                                matcher.scan(&input, &mut scratch)
                            } else {
                                matcher.scan_reference(&input, &mut scratch)
                            }
                            .unwrap();
                            std::hint::black_box(result);
                        }
                        if sample >= 3 {
                            samples.push(start.elapsed().as_nanos() / 100);
                        }
                    }
                    samples.sort_unstable();
                    println!("Guard timing bytes={bytes} case={case} reverse={reverse} accelerated={accelerated} p50_ns={} owned={cost}", samples[10]);
                }
            }
        }
    }
}

#[test]
fn deduplicated_regexes_match_independent_programs_in_both_byte_modes() {
    let sources = ["a", "", "(?i:a)", "^a$", "a|ba", "(?-u:\\xFF)", "a", "a|ba", "", "é"];
    let mut patterns = sources
        .iter()
        .map(|source| PatternSpec::Regex(source))
        .collect::<Vec<_>>();
    patterns.insert(
        3,
        PatternSpec::Phrase {
            value: "ph",
            ascii_case_insensitive: false,
        },
    );
    for unicode in [true, false] {
        let matcher =
            CompiledMatcher::compile_with_regex_unicode(&patterns, MatcherLimits::default(), unicode).unwrap();
        assert_eq!(matcher.test_regex_counts(), (10, 7));
        assert_eq!(matcher.regex.as_ref().unwrap().core.vm.pattern_len(), 7);
        let programs = sources
            .iter()
            .map(|source| {
                if unicode {
                    CompiledRegexProgram::compile_one(source, RegexProgramLimits::default())
                } else {
                    CompiledRegexProgram::compile_one_pcre_bytes_with_peak_budget(
                        source,
                        RegexProgramLimits::default(),
                        usize::MAX,
                    )
                }
                .unwrap()
            })
            .collect::<Vec<_>>();
        let mut single_scratch = programs
            .iter()
            .map(CompiledRegexProgram::create_scratch)
            .collect::<Vec<_>>();
        let mut inputs = vec![
            vec![],
            b"a".to_vec(),
            b"A".to_vec(),
            b"ba\nph".to_vec(),
            vec![255, b'a'],
            "é".as_bytes().to_vec(),
        ];
        let mut seed = 42_u64;
        for length in 0..128 {
            inputs.push(
                (0..length % 31)
                    .map(|_| {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        (seed >> 32) as u8
                    })
                    .collect(),
            );
        }
        let mut scratch = matcher.create_scratch();
        for input in inputs {
            let mut expected = Vec::new();
            for (index, (program, cache)) in programs.iter().zip(&mut single_scratch).enumerate() {
                if program.is_match(&input, cache).unwrap() {
                    expected.push(if index < 3 { index } else { index + 1 });
                }
            }
            if input.windows(2).any(|bytes| bytes == b"ph") {
                expected.push(3);
            }
            expected.sort_unstable();
            let results = matcher.scan(&input, &mut scratch).unwrap();
            assert_eq!(ids(results), expected, "unicode={unicode} input={input:?}");
            for index in 0..patterns.len() {
                assert_eq!(
                    results.contains_any_in_range(index..index + 1),
                    Some(expected.contains(&index))
                );
            }
            assert_eq!(ids(matcher.scan_reference(&input, &mut scratch).unwrap()), expected);
        }
    }
}

#[test]
fn regex_dedup_work_exhaustion_rebuilds_original_mapping_and_keeps_source_gates() {
    let patterns = [
        PatternSpec::Regex("z+"),
        PatternSpec::Regex("a"),
        PatternSpec::Regex("z+"),
    ];
    for budget in [0, 1, 5] {
        let matcher = CompiledMatcher::compile_with_regex_unicode_and_work_limit(
            &patterns,
            MatcherLimits::default(),
            false,
            budget,
        )
        .unwrap();
        assert_eq!(matcher.test_regex_counts(), (3, 3));
        assert!(matcher.regex_fanout_ends.is_empty());
        assert_eq!(matcher.regex_fanout_ends.capacity(), 0);
        assert_eq!(
            ids(matcher.scan(b"zzz", &mut matcher.create_scratch()).unwrap()),
            [0, 2]
        );
        assert_eq!(ids(matcher.scan(b"a", &mut matcher.create_scratch()).unwrap()), [1]);
    }
    for limits in [
        MatcherLimits {
            max_patterns: 2,
            ..MatcherLimits::default()
        },
        MatcherLimits {
            max_regex_source_bytes_per_group: 4,
            ..MatcherLimits::default()
        },
        MatcherLimits {
            max_pattern_bytes: 4,
            ..MatcherLimits::default()
        },
    ] {
        assert!(CompiledMatcher::compile(&patterns, limits).is_err());
    }
    let duplicate = [PatternSpec::Regex("a"), PatternSpec::Regex("a")];
    // Syntax-work limits apply to the unique compiled sources; logical count and
    // source-byte gates above still apply to every occurrence.
    let limits = MatcherLimits {
        max_regex_ast_nodes: 1,
        ..MatcherLimits::default()
    };
    assert!(CompiledMatcher::compile(&duplicate, limits).is_ok());
    assert!(CompiledMatcher::compile(&[PatternSpec::Regex("a"), PatternSpec::Regex("b")], limits).is_err());
    assert!(matches!(
        CompiledMatcher::compile(
            &[PatternSpec::Regex("z"), PatternSpec::Regex("(")],
            MatcherLimits::default()
        ),
        Err(MatcherCompileError::Regex { pattern_id: Some(0) })
    ));
}

#[test]
fn regex_fanout_capacity_scratch_shape_and_physical_savings_are_accounted() {
    let patterns = [
        PatternSpec::Regex("(?i)attack[0-9]+"),
        PatternSpec::Regex("(?i)attack[0-9]+"),
        PatternSpec::Regex("safe"),
        PatternSpec::Regex("(?i)attack[0-9]+"),
    ];
    let shared = CompiledMatcher::compile_pcre_bytes(&patterns, MatcherLimits::default()).unwrap();
    let unshared =
        CompiledMatcher::compile_with_regex_unicode_and_work_limit(&patterns, MatcherLimits::default(), false, 0)
            .unwrap();
    assert_eq!(shared.test_regex_counts(), (4, 2));
    assert_eq!(shared.regex_fanout_ends.capacity(), 4);
    assert_eq!(
        shared.memory_usage(),
        shared.regex.as_ref().unwrap().memory_usage()
            + shared.regex_pattern_ids.capacity() * size_of::<PatternId>()
            + shared.regex_fanout_ends.capacity() * size_of::<usize>()
    );
    assert_eq!(shared.create_scratch().regex_matches.capacity(), 2);
    assert!(shared.memory_usage() < unshared.memory_usage());
    assert!(shared.required_scratch_bytes() < unshared.required_scratch_bytes());
    assert!(matches!(
        shared.scan(b"ATTACK1", &mut unshared.create_scratch()),
        Err(MatcherScanError::ScratchShape)
    ));
    let foreign = CompiledMatcher::compile_pcre_bytes(
        &[
            PatternSpec::Regex("x"),
            PatternSpec::Regex("x"),
            PatternSpec::Regex("y"),
            PatternSpec::Regex("y"),
        ],
        MatcherLimits::default(),
    )
    .unwrap();
    assert!(matches!(
        shared.scan(b"ATTACK1", &mut foreign.create_scratch()),
        Err(MatcherScanError::ScratchShape)
    ));
    println!(
        "Regex fanout witness: physical=4->2 retained={}->{} scratch={}->{}",
        unshared.memory_usage(),
        shared.memory_usage(),
        unshared.required_scratch_bytes(),
        shared.required_scratch_bytes()
    );
}

#[test]
fn overlapping_phrases_all_match() {
    let patterns = [
        PatternSpec::Phrase {
            value: "he",
            ascii_case_insensitive: false,
        },
        PatternSpec::Phrase {
            value: "she",
            ascii_case_insensitive: false,
        },
        PatternSpec::Phrase {
            value: "hers",
            ascii_case_insensitive: false,
        },
    ];
    let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
    let mut scratch = matcher.create_scratch();

    assert_eq!(ids(matcher.scan(b"ushers", &mut scratch).unwrap()), vec![0, 1, 2]);
}

#[test]
fn phrase_case_policies_are_independent() {
    let patterns = [
        PatternSpec::Phrase {
            value: "Admin",
            ascii_case_insensitive: false,
        },
        PatternSpec::Phrase {
            value: "Root",
            ascii_case_insensitive: true,
        },
    ];
    let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
    let mut scratch = matcher.create_scratch();

    assert_eq!(ids(matcher.scan(b"admin ROOT", &mut scratch).unwrap()), vec![1]);
}

#[test]
fn phrase_and_regex_results_keep_source_order() {
    let patterns = [
        PatternSpec::Regex(r"union\s+select"),
        PatternSpec::Phrase {
            value: "script",
            ascii_case_insensitive: true,
        },
        PatternSpec::Regex(r"(?i)select"),
    ];
    let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
    let mut scratch = matcher.create_scratch();

    assert_eq!(
        ids(matcher.scan(b"<SCRIPT> union select", &mut scratch).unwrap()),
        vec![0, 1, 2]
    );
}

#[test]
fn byte_oriented_scan_tolerates_invalid_utf8() {
    let patterns = [
        PatternSpec::Phrase {
            value: "admin",
            ascii_case_insensitive: false,
        },
        PatternSpec::Regex("admin"),
    ];
    let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
    let mut scratch = matcher.create_scratch();

    assert_eq!(ids(matcher.scan(b"\xFFadmin", &mut scratch).unwrap()), vec![0, 1]);
}

#[test]
fn duplicate_patterns_keep_distinct_ids() {
    let patterns = [
        PatternSpec::Phrase {
            value: "same",
            ascii_case_insensitive: false,
        },
        PatternSpec::Phrase {
            value: "same",
            ascii_case_insensitive: false,
        },
    ];
    let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
    let mut scratch = matcher.create_scratch();

    assert_eq!(ids(matcher.scan(b"same", &mut scratch).unwrap()), vec![0, 1]);
}

#[test]
fn capture_phrase_program_is_ascii_insensitive_leftmost_longest() {
    let program =
        CompiledCapturePhraseProgram::compile(&["he", "hers", "HERS", "she"], MatcherLimits::default()).unwrap();

    let matched = program.find(b"xxSHERS he").unwrap();
    assert_eq!(matched.pattern_id(), 3);
    assert_eq!((matched.start(), matched.end()), (2, 5));

    let tied = program.find(b"HERS").unwrap();
    assert_eq!(tied.pattern_id(), 1);
    assert_eq!((tied.start(), tied.end()), (0, 4));
    assert_eq!(program.pattern_count(), 4);
    assert!(program.memory_usage() > 0);
    assert!(program.estimated_peak_build_memory_usage() >= program.memory_usage());
}

#[test]
fn capture_phrase_program_enforces_every_shared_ac_limit() {
    let count = MatcherLimits {
        max_patterns: 1,
        ..MatcherLimits::default()
    };
    assert_eq!(
        CompiledCapturePhraseProgram::compile(&["a", "b"], count).unwrap_err(),
        MatcherCompileError::PatternLimit { actual: 2, limit: 1 }
    );

    let bytes = MatcherLimits {
        max_pattern_bytes: 1,
        ..MatcherLimits::default()
    };
    assert_eq!(
        CompiledCapturePhraseProgram::compile(&["ab"], bytes).unwrap_err(),
        MatcherCompileError::PatternByteLimit { actual: 2, limit: 1 }
    );
    assert_eq!(
        CompiledCapturePhraseProgram::compile(&[""], MatcherLimits::default()).unwrap_err(),
        MatcherCompileError::EmptyPhrase { pattern_id: 0 }
    );

    let states = MatcherLimits {
        max_phrase_states: 3,
        ..MatcherLimits::default()
    };
    assert_eq!(
        CompiledCapturePhraseProgram::compile(&["abc"], states).unwrap_err(),
        MatcherCompileError::PhraseStateLimit { limit: 3 }
    );

    let transitions = MatcherLimits {
        max_phrase_transitions: 1,
        ..MatcherLimits::default()
    };
    assert_eq!(
        CompiledCapturePhraseProgram::compile(&["ab"], transitions).unwrap_err(),
        MatcherCompileError::PhraseTransitionLimit { limit: 1 }
    );

    let outputs = MatcherLimits {
        max_phrase_outputs: 1,
        ..MatcherLimits::default()
    };
    assert_eq!(
        CompiledCapturePhraseProgram::compile(&["same", "same"], outputs).unwrap_err(),
        MatcherCompileError::PhraseOutputLimit { limit: 1 }
    );

    let compiled = MatcherLimits {
        max_phrase_compiled_bytes: 0,
        ..MatcherLimits::default()
    };
    assert_eq!(
        CompiledCapturePhraseProgram::compile(&["a"], compiled).unwrap_err(),
        MatcherCompileError::PhraseCompiledByteLimit { limit: 0 }
    );
}

#[test]
fn suffix_patterns_are_reported_through_the_phrase_adapter() {
    let source_patterns = ["a", "aa", "aaa", "aaaa", "aaaaa", "aaaaaa"];
    let patterns: Vec<PatternSpec<'_>> = source_patterns
        .iter()
        .map(|value| PatternSpec::Phrase {
            value,
            ascii_case_insensitive: false,
        })
        .collect();
    let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
    let mut scratch = matcher.create_scratch();

    assert_eq!(
        ids(matcher.scan(b"aaaaaa", &mut scratch).unwrap()),
        vec![0, 1, 2, 3, 4, 5]
    );
}

#[test]
fn scratch_reuse_clears_previous_results() {
    let patterns = [PatternSpec::Phrase {
        value: "attack",
        ascii_case_insensitive: false,
    }];
    let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
    let mut scratch = matcher.create_scratch();

    assert_eq!(ids(matcher.scan(b"attack", &mut scratch).unwrap()), vec![0]);
    assert!(matcher.scan(b"normal", &mut scratch).unwrap().is_empty());
}

#[test]
fn scratch_from_another_matcher_is_rejected() {
    let first = CompiledMatcher::compile(&[PatternSpec::Regex("a")], MatcherLimits::default()).unwrap();
    let second = CompiledMatcher::compile(
        &[
            PatternSpec::Regex("a"),
            PatternSpec::Phrase {
                value: "b",
                ascii_case_insensitive: false,
            },
        ],
        MatcherLimits::default(),
    )
    .unwrap();
    let mut scratch = first.create_scratch();

    assert!(matches!(
        second.scan(b"ab", &mut scratch),
        Err(MatcherScanError::ScratchShape)
    ));
}

#[test]
fn compile_limits_are_enforced() {
    let limits = MatcherLimits {
        max_patterns: 1,
        max_pattern_bytes: 3,
        max_phrase_states: 3,
        ..MatcherLimits::default()
    };
    let too_many = [PatternSpec::Regex("a"), PatternSpec::Regex("b")];
    assert_eq!(
        CompiledMatcher::compile(&too_many, limits).unwrap_err(),
        MatcherCompileError::PatternLimit { actual: 2, limit: 1 }
    );

    let too_large = [PatternSpec::Regex("abcd")];
    assert_eq!(
        CompiledMatcher::compile(&too_large, limits).unwrap_err(),
        MatcherCompileError::PatternByteLimit { actual: 4, limit: 3 }
    );

    let state_limits = MatcherLimits {
        max_patterns: 1,
        max_pattern_bytes: 16,
        max_phrase_states: 3,
        ..MatcherLimits::default()
    };
    let too_many_states = [PatternSpec::Phrase {
        value: "abc",
        ascii_case_insensitive: false,
    }];
    assert_eq!(
        CompiledMatcher::compile(&too_many_states, state_limits).unwrap_err(),
        MatcherCompileError::PhraseStateLimit { limit: 3 }
    );

    let transition_limits = MatcherLimits {
        max_phrase_transitions: 1,
        ..MatcherLimits::default()
    };
    let too_many_transitions = [PatternSpec::Phrase {
        value: "ab",
        ascii_case_insensitive: false,
    }];
    assert_eq!(
        CompiledMatcher::compile(&too_many_transitions, transition_limits).unwrap_err(),
        MatcherCompileError::PhraseTransitionLimit { limit: 1 }
    );

    let output_limits = MatcherLimits {
        max_phrase_outputs: 1,
        ..MatcherLimits::default()
    };
    let too_many_outputs = [
        PatternSpec::Phrase {
            value: "same",
            ascii_case_insensitive: false,
        },
        PatternSpec::Phrase {
            value: "same",
            ascii_case_insensitive: false,
        },
    ];
    assert_eq!(
        CompiledMatcher::compile(&too_many_outputs, output_limits).unwrap_err(),
        MatcherCompileError::PhraseOutputLimit { limit: 1 }
    );

    let compiled_byte_limits = MatcherLimits {
        max_phrase_compiled_bytes: 0,
        ..MatcherLimits::default()
    };
    let compiled_bytes = [PatternSpec::Phrase {
        value: "a",
        ascii_case_insensitive: false,
    }];
    assert_eq!(
        CompiledMatcher::compile(&compiled_bytes, compiled_byte_limits).unwrap_err(),
        MatcherCompileError::PhraseCompiledByteLimit { limit: 0 }
    );
}

#[test]
fn empty_phrase_is_rejected() {
    let error = CompiledMatcher::compile(
        &[PatternSpec::Phrase {
            value: "",
            ascii_case_insensitive: false,
        }],
        MatcherLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error, MatcherCompileError::EmptyPhrase { pattern_id: 0 });
}

#[test]
fn invalid_regex_identifies_its_source_pattern() {
    let error = CompiledMatcher::compile(
        &[
            PatternSpec::Phrase {
                value: "valid",
                ascii_case_insensitive: false,
            },
            PatternSpec::Regex("("),
        ],
        MatcherLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        MatcherCompileError::Regex {
            pattern_id: Some(1),
            ..
        }
    ));
}

#[test]
fn phrase_automaton_matches_naive_reference() {
    let source_patterns = ["a", "ab", "bab", "bc", "bca", "c", "caa"];
    let patterns: Vec<PatternSpec<'_>> = source_patterns
        .iter()
        .map(|value| PatternSpec::Phrase {
            value,
            ascii_case_insensitive: false,
        })
        .collect();
    let matcher = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
    let mut scratch = matcher.create_scratch();
    let mut state = 0xA5A5_1234_u64;

    for _ in 0..2_000 {
        let length = (next_random(&mut state) % 64) as usize;
        let haystack: Vec<u8> = (0..length)
            .map(|_| b'a' + (next_random(&mut state) % 4) as u8)
            .collect();
        let actual = ids(matcher.scan(&haystack, &mut scratch).unwrap());
        let expected: Vec<usize> = source_patterns
            .iter()
            .enumerate()
            .filter_map(|(index, pattern)| {
                haystack
                    .windows(pattern.len())
                    .any(|window| window == pattern.as_bytes())
                    .then_some(index)
            })
            .collect();
        assert_eq!(actual, expected, "haystack={haystack:?}");
    }
}

#[test]
fn prebuild_projection_has_exact_partition_and_map_charges() {
    let counts = MatcherPatternCounts {
        sensitive_phrases: 2,
        insensitive_phrases: 1,
        regexes: 3,
    };
    let bytes = MatcherPatternBytes {
        sensitive_phrases: 5,
        insensitive_phrases: 7,
        regexes: 11,
    };
    let projection = MatcherBuildProjection::for_counts(counts, bytes, MatcherLimits::default()).unwrap();
    assert_eq!(
        projection.working_partition_bytes(),
        3 * size_of::<(PatternId, &str)>() + 3 * (size_of::<RegexPatternEntry<'_>>() + size_of::<&str>())
    );
    assert_eq!(
        projection.retained_map_bytes(),
        6 * size_of::<PatternId>() + 3 * size_of::<usize>()
    );
    assert!(projection.sensitive_phrase_peak() > 0);
    assert!(projection.insensitive_phrase_peak() > 0);
    assert!(projection.regex_peak() > 0);
    assert!(projection.conservative_peak_bytes().unwrap() >= projection.regex_peak());
}

#[test]
fn prebuild_projection_accepts_exact_limits_and_rejects_plus_one() {
    let counts = MatcherPatternCounts {
        sensitive_phrases: 1,
        insensitive_phrases: 1,
        regexes: 1,
    };
    let bytes = MatcherPatternBytes {
        sensitive_phrases: 2,
        insensitive_phrases: 3,
        regexes: 4,
    };
    let limits = MatcherLimits {
        max_patterns: 3,
        max_pattern_bytes: 9,
        ..MatcherLimits::default()
    };
    MatcherBuildProjection::for_counts(counts, bytes, limits).unwrap();
    assert_eq!(
        MatcherBuildProjection::for_counts(MatcherPatternCounts { regexes: 2, ..counts }, bytes, limits,).unwrap_err(),
        MatcherCompileError::PatternLimit { actual: 4, limit: 3 }
    );
    assert_eq!(
        MatcherBuildProjection::for_counts(counts, MatcherPatternBytes { regexes: 5, ..bytes }, limits,).unwrap_err(),
        MatcherCompileError::PatternByteLimit { actual: 10, limit: 9 }
    );
    assert_eq!(
        MatcherBuildProjection::for_counts(
            MatcherPatternCounts {
                sensitive_phrases: usize::MAX,
                ..MatcherPatternCounts::default()
            },
            MatcherPatternBytes::default(),
            MatcherLimits {
                max_patterns: usize::MAX,
                ..MatcherLimits::default()
            },
        )
        .unwrap_err(),
        MatcherCompileError::PeakArithmetic
    );
}

#[test]
fn retained_and_peak_reports_cover_every_backend_and_mixed_groups() {
    let cases: &[&[PatternSpec<'_>]] = &[
        &[PatternSpec::Phrase {
            value: "sensitive",
            ascii_case_insensitive: false,
        }],
        &[PatternSpec::Phrase {
            value: "insensitive",
            ascii_case_insensitive: true,
        }],
        &[PatternSpec::Regex("regex")],
        &[
            PatternSpec::Phrase {
                value: "sensitive",
                ascii_case_insensitive: false,
            },
            PatternSpec::Phrase {
                value: "insensitive",
                ascii_case_insensitive: true,
            },
            PatternSpec::Regex("regex"),
        ],
    ];
    for patterns in cases {
        let matcher = CompiledMatcher::compile(patterns, MatcherLimits::default()).unwrap();
        let exact_retained = matcher.sensitive_phrases.as_ref().map_or(0, PhraseGroup::memory_usage)
            + matcher
                .insensitive_phrases
                .as_ref()
                .map_or(0, PhraseGroup::memory_usage)
            + matcher.regex.as_ref().map_or(0, CompiledRegexProgram::memory_usage)
            + matcher.regex_pattern_ids.len() * size_of::<PatternId>();
        let exact_peak = matcher
            .sensitive_phrases
            .as_ref()
            .map_or(0, PhraseGroup::estimated_peak_build_memory_usage)
            + matcher
                .insensitive_phrases
                .as_ref()
                .map_or(0, PhraseGroup::estimated_peak_build_memory_usage)
            + matcher.regex.as_ref().map_or(0, CompiledRegexProgram::memory_usage)
            + matcher.regex_pattern_ids.len() * size_of::<PatternId>();
        assert_eq!(matcher.memory_usage(), exact_retained);
        assert_eq!(matcher.estimated_peak_build_memory_usage(), exact_peak);
        assert!(matcher.memory_usage() > 0);
        assert!(matcher.estimated_peak_build_memory_usage() >= matcher.memory_usage());
    }
}

#[test]
fn regex_retained_program_limit_is_post_build_and_exact() {
    let patterns = [PatternSpec::Regex("abc")];
    let baseline = CompiledMatcher::compile(&patterns, MatcherLimits::default()).unwrap();
    let exact = baseline.regex.as_ref().unwrap().core.program_peak_bound;
    CompiledMatcher::compile(
        &patterns,
        MatcherLimits {
            max_regex_program_bytes: exact,
            ..MatcherLimits::default()
        },
    )
    .unwrap();
    let error = CompiledMatcher::compile(
        &patterns,
        MatcherLimits {
            max_regex_program_bytes: exact - 1,
            ..MatcherLimits::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        error,
        MatcherCompileError::RegexProgramByteLimit {
            actual: exact,
            limit: exact - 1
        }
    );
}

#[test]
fn single_regex_program_is_byte_oriented_empty_safe_and_redacted() {
    let empty = CompiledRegexProgram::compile_one("", RegexProgramLimits::default()).unwrap();
    let mut empty_scratch = empty.create_scratch();
    assert!(empty.is_match(b"\xff", &mut empty_scratch).unwrap());
    let bytes = CompiledRegexProgram::compile_one("(?-u:\\xFF)", RegexProgramLimits::default()).unwrap();
    let mut byte_scratch = bytes.create_scratch();
    assert!(bytes.is_match(b"\xff", &mut byte_scratch).unwrap());
    let error = CompiledRegexProgram::compile_one("private-(", RegexProgramLimits::default()).unwrap_err();
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("private"));
    assert_eq!(error, RegexProgramError::Syntax);
}

#[test]
fn single_regex_program_rejects_scratch_from_another_program() {
    let left = CompiledRegexProgram::compile_one("a", RegexProgramLimits::default()).unwrap();
    let right = CompiledRegexProgram::compile_one("b", RegexProgramLimits::default()).unwrap();
    let mut boolean_scratch = right.create_scratch();
    assert_eq!(
        left.is_match(b"a", &mut boolean_scratch),
        Err(RegexScratchError::ScratchShape)
    );
    assert!(boolean_scratch.cache.is_none(), "owner check precedes cache creation");

    let left = CompiledCaptureRegexProgram::compile_one("(a)", RegexProgramLimits::default()).unwrap();
    let right = CompiledCaptureRegexProgram::compile_one("(b)", RegexProgramLimits::default()).unwrap();
    let mut capture_scratch = right.create_scratch();
    assert_eq!(
        left.is_match(b"a", &mut capture_scratch),
        Err(RegexScratchError::ScratchShape)
    );
    assert!(matches!(
        left.captures(b"a", &mut capture_scratch),
        Err(RegexScratchError::ScratchShape)
    ));
}

#[test]
fn class_range_projection_bounds_translated_hir_for_nested_sets_and_flags() {
    for source in [
        "",
        "a",
        "^$",
        "a|b",
        "a*",
        "(a)",
        "(?i)",
        r"\d",
        r"\p{Greek}",
        ".",
        "(?s:.)",
        "(?-u:.)",
        "(?i)a(?-i)b",
        "(?i:a)b",
        "(?i)(a)(?-i:b)",
        "[a-z]",
        "[abc]",
        "[[:word:]]",
        "(?i:[a-z])",
        "[^aeiou]",
        "[a-z&&[^aeiou]]",
        "[a-z--[aeiou]]",
        "[a-z~~[n-z]]",
    ] {
        let ast = ast::parse::ParserBuilder::new()
            .nest_limit(RegexProgramLimits::default().max_nest_depth)
            .octal(false)
            .build()
            .parse(source)
            .unwrap();
        let projection = project_ast_ranges(&ast, source.len() + 1, false, true, false).unwrap();
        let hir = hir::translate::TranslatorBuilder::new()
            .utf8(false)
            .unicode(true)
            .build()
            .translate(source, &ast)
            .unwrap();
        let metrics = hir_metrics(&hir, source.len() + 1).unwrap();
        assert!(
            metrics.ranges <= projection.out_ranges,
            "source={source:?}, actual={}, projection={projection:?}",
            metrics.ranges
        );
        assert!(projection.peak_ranges >= projection.out_ranges, "source={source:?}");
    }
}

#[test]
fn single_regex_source_and_projected_peak_limits_accept_exact_boundary() {
    let source = "(?i:[a-z&&[^aeiou]])";
    let baseline = CompiledRegexProgram::compile_one(source, RegexProgramLimits::default()).unwrap();
    let exact_peak = baseline.estimated_peak_build_memory_usage();
    let exact_source_limits = RegexProgramLimits {
        max_source_bytes: source.len(),
        ..RegexProgramLimits::default()
    };
    CompiledRegexProgram::compile_one_with_peak_budget(source, exact_source_limits, exact_peak).unwrap();
    assert_eq!(
        CompiledRegexProgram::compile_one_with_peak_budget(source, exact_source_limits, exact_peak - 1).unwrap_err(),
        RegexProgramError::SyntaxPeakLimit
    );
    assert_eq!(
        CompiledRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_source_bytes: source.len() - 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::SourceByteLimit {
            actual: source.len(),
            limit: source.len() - 1,
        }
    );
}

#[test]
fn regex_structural_limits_accept_exact_and_reject_first_excess() {
    let source = "(?P<name>[a-z]{2,4})xy";
    let ast = ast::parse::ParserBuilder::new()
        .nest_limit(RegexProgramLimits::default().max_nest_depth)
        .octal(false)
        .build()
        .parse(source)
        .unwrap();
    let ast_nodes = count_ast_nodes(&ast).unwrap();
    let hir = hir::translate::TranslatorBuilder::new()
        .utf8(false)
        .unicode(true)
        .build()
        .translate(source, &ast)
        .unwrap();
    let metrics = hir_metrics(&hir, source.len() + 1).unwrap();

    for limits in [
        RegexProgramLimits {
            max_ast_nodes: ast_nodes,
            ..RegexProgramLimits::default()
        },
        RegexProgramLimits {
            max_hir_nodes: metrics.nodes,
            ..RegexProgramLimits::default()
        },
        RegexProgramLimits {
            max_class_ranges: metrics.ranges,
            ..RegexProgramLimits::default()
        },
        RegexProgramLimits {
            max_hir_literal_bytes: metrics.literal_bytes,
            ..RegexProgramLimits::default()
        },
        RegexProgramLimits {
            max_repetition_work: metrics.repetition_work,
            ..RegexProgramLimits::default()
        },
    ] {
        CompiledCaptureRegexProgram::compile_one(source, limits).unwrap();
    }
    assert_eq!(
        CompiledCaptureRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_ast_nodes: ast_nodes - 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::AstNodeLimit {
            actual: ast_nodes,
            limit: ast_nodes - 1,
        }
    );
    assert_eq!(
        CompiledCaptureRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_hir_nodes: metrics.nodes - 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::HirNodeLimit {
            actual: metrics.nodes,
            limit: metrics.nodes - 1,
        }
    );
    assert_eq!(
        CompiledCaptureRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_class_ranges: metrics.ranges - 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::ClassRangeLimit {
            actual: metrics.ranges,
            limit: metrics.ranges - 1,
        }
    );
    assert_eq!(
        CompiledCaptureRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_hir_literal_bytes: metrics.literal_bytes - 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::HirLiteralByteLimit {
            actual: metrics.literal_bytes,
            limit: metrics.literal_bytes - 1,
        }
    );
    assert_eq!(
        CompiledCaptureRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_repetition_work: metrics.repetition_work - 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::RepetitionWorkLimit {
            actual: metrics.repetition_work,
            limit: metrics.repetition_work - 1,
        }
    );
    assert_eq!(
        CompiledRegexProgram::compile_one(
            "(a)",
            RegexProgramLimits {
                max_nest_depth: 0,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::NestLimit { limit: 0 }
    );
    assert_eq!(
        CompiledRegexProgram::compile_one(
            "a",
            RegexProgramLimits {
                max_nest_depth: MAX_SUPPORTED_REGEX_NEST_DEPTH + 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::NestConfigurationLimit {
            actual: MAX_SUPPORTED_REGEX_NEST_DEPTH + 1,
            limit: MAX_SUPPORTED_REGEX_NEST_DEPTH,
        }
    );
    assert_eq!(
        CompiledRegexProgram::compile_one(
            "aa",
            RegexProgramLimits {
                max_source_bytes: 1,
                max_nest_depth: MAX_SUPPORTED_REGEX_NEST_DEPTH + 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::SourceByteLimit { actual: 2, limit: 1 }
    );
}

#[test]
fn project_and_meter_walk_deep_trees_without_recursive_project_stacks() {
    let depth = 1_024_usize;
    let source = format!("{}a{}", "(".repeat(depth), ")".repeat(depth));
    let ast = ast::parse::ParserBuilder::new()
        .nest_limit(u32::try_from(depth).unwrap())
        .octal(false)
        .build()
        .parse(&source)
        .unwrap();
    let projection = project_ast_ranges(&ast, source.len() + 1, false, true, false).unwrap();
    let hir = hir::translate::TranslatorBuilder::new()
        .utf8(false)
        .unicode(true)
        .build()
        .translate(&source, &ast)
        .unwrap();
    let metrics = hir_metrics(&hir, source.len() + 1).unwrap();
    assert!(projection.out_ranges >= metrics.ranges);
    assert!(metrics.nodes > depth);
}

#[test]
fn capture_program_distinguishes_unmatched_empty_named_and_numeric_groups() {
    let program =
        CompiledCaptureRegexProgram::compile_one(r"^(?P<head>a)?(?P<empty>)(b)$", RegexProgramLimits::default())
            .unwrap();
    let mut scratch = program.create_scratch();
    let results = program.captures(b"b", &mut scratch).unwrap();
    assert_eq!(results.overall(), Some(0..1));
    assert_eq!(results.group(1), None);
    assert_eq!(results.group_by_name("head"), None);
    assert_eq!(results.group(2), Some(0..0));
    assert_eq!(results.group_by_name("empty"), Some(0..0));
    assert_eq!(results.group(3), Some(0..1));
    assert_eq!(results.group(4), None);
}

#[test]
fn capture_program_supports_crs_unicode_word_boundaries_on_byte_haystacks() {
    let source = r"(?:\bhttp/\d|<(?:html|meta)\b)";
    let program = CompiledCaptureRegexProgram::compile_one(source, RegexProgramLimits::default()).unwrap();
    let mut scratch = program.create_scratch();

    assert!(program.is_match(b"prefix http/2 suffix", &mut scratch).unwrap());
    assert!(program.is_match(b"<meta charset=x>", &mut scratch).unwrap());
    assert!(!program.is_match(b"xhttp/2", &mut scratch).unwrap());
}

#[test]
fn capture_program_uses_the_first_leftmost_match_in_a_multi_range_header() {
    let program = CompiledCaptureRegexProgram::compile_one(r"(\d+)-(\d+)", RegexProgramLimits::default()).unwrap();
    let mut scratch = program.create_scratch();
    let captures = program.captures(b"bytes=0-,5-0,5-1,5-15", &mut scratch).unwrap();
    assert_eq!(captures.group(0), Some(9..12));
    assert_eq!(captures.group(1), Some(9..10));
    assert_eq!(captures.group(2), Some(11..12));
}

#[test]
fn capture_scratch_is_owner_bound_non_zst_and_size_bounded_at_compile() {
    let left = CompiledCaptureRegexProgram::compile_one("(a)", RegexProgramLimits::default()).unwrap();
    let right = CompiledCaptureRegexProgram::compile_one("(a)", RegexProgramLimits::default()).unwrap();
    let required = left.required_scratch_bytes();
    CompiledCaptureRegexProgram::compile_one(
        "(a)",
        RegexProgramLimits {
            max_scratch_bytes: required,
            ..RegexProgramLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        CompiledCaptureRegexProgram::compile_one(
            "(a)",
            RegexProgramLimits {
                max_scratch_bytes: required - 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::ScratchByteLimit {
            actual: required,
            limit: required - 1
        }
    );
    assert_ne!(
        std::ptr::from_ref(&left.core.token),
        std::ptr::from_ref(&right.core.token)
    );
    assert!(size_of::<CaptureShapeToken>() > 0);
}

#[test]
fn scratch_upper_bounds_first_use_epsilon_growth_for_boolean_and_capture_programs() {
    let boolean_source = "(?:a?|b?|c?|d?){8}z";
    let boolean = CompiledRegexProgram::compile_one(boolean_source, RegexProgramLimits::default()).unwrap();
    let boolean_required = boolean.required_scratch_bytes();
    CompiledRegexProgram::compile_one(
        boolean_source,
        RegexProgramLimits {
            max_scratch_bytes: boolean_required,
            ..RegexProgramLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        CompiledRegexProgram::compile_one(
            boolean_source,
            RegexProgramLimits {
                max_scratch_bytes: boolean_required - 1,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::ScratchByteLimit {
            actual: boolean_required,
            limit: boolean_required - 1,
        }
    );
    let mut boolean_scratch = boolean.create_scratch();
    assert!(!boolean.is_match(b"abcdabcd", &mut boolean_scratch).unwrap());
    let boolean_actual = size_of::<RegexBooleanScratch>()
        + boolean_scratch
            .cache
            .as_ref()
            .expect("VM ran for an 8-byte haystack")
            .memory_usage();
    assert!(boolean_actual <= boolean_required);

    let capture_source = "(?:(a?)|(b?)|(c?)|(d?)){8}z";
    let capture = CompiledCaptureRegexProgram::compile_one(capture_source, RegexProgramLimits::default()).unwrap();
    let capture_required = capture.required_scratch_bytes();
    let mut capture_scratch = capture.create_scratch();
    assert!(capture
        .captures(b"abcdabcd", &mut capture_scratch)
        .unwrap()
        .overall()
        .is_none());
    let capture_actual = size_of::<RegexCaptureScratch>()
        + capture_scratch
            .cache
            .as_ref()
            .expect("capture search ran")
            .memory_usage()
        + capture_scratch
            .captures
            .as_ref()
            .expect("capture search ran")
            .slots()
            .len()
            * size_of::<usize>();
    assert!(capture_actual <= capture_required);
}

#[test]
fn boolean_scratch_cache_is_lazy_for_exact_hint() {
    let mut program = CompiledRegexProgram::compile_one("^abc$", RegexProgramLimits::default()).unwrap();
    assert!(program.lower_simple(guard_budget()).unwrap().1);
    let mut scratch = program.create_scratch();
    assert!(scratch.cache.is_none());
    assert!(program.is_match(b"abc", &mut scratch).unwrap());
    assert!(!program.is_match(b"xabc", &mut scratch).unwrap());
    assert!(!program.is_match(b"", &mut scratch).unwrap());
    assert!(scratch.cache.is_none(), "exact hint must not build the VM cache");
}

#[test]
fn boolean_scratch_cache_is_lazy_for_guard_miss() {
    let mut program = CompiledRegexProgram::compile_one("alpha[0-9]", RegexProgramLimits::default()).unwrap();
    assert!(program.add_literal_guard(guard_budget()).unwrap().owned > 0);
    let mut scratch = program.create_scratch();
    let haystack = vec![b'z'; 1024];
    assert!(!program.may_match(&haystack));
    assert!(!program.is_match(&haystack, &mut scratch).unwrap());
    assert!(scratch.cache.is_none(), "guard miss must not build the VM cache");

    let mut hit = vec![b'z'; 1024];
    hit.extend_from_slice(b"alpha7");
    assert!(program.is_match(&hit, &mut scratch).unwrap());
    assert!(scratch.cache.is_some(), "guard hit confirms through the VM");
}

#[test]
fn boolean_scratch_cache_builds_on_vm_path() {
    let program = CompiledRegexProgram::compile_one("(?:a?|b?|c?|d?){8}z", RegexProgramLimits::default()).unwrap();
    let mut scratch = program.create_scratch();
    assert!(scratch.cache.is_none());
    assert!(!program.is_match(b"abcdabcd", &mut scratch).unwrap());
    assert!(scratch.cache.is_some(), "VM ran");
    assert!(program.is_match(b"abcdz", &mut scratch).unwrap());
}

#[test]
fn lazy_cache_results_equal_eager_reference() {
    enum Hint {
        None,
        Simple,
        Guard,
    }
    let fixtures: [(&str, Hint, &[&[u8]]); 6] = [
        ("^alpha$", Hint::Simple, &[b"alpha", b"alpha\n", b"otheralpha", b""]),
        ("alpha[0-9]", Hint::Guard, &[b"alpha1", b"alpha!", b"zzzalpha9", b""]),
        ("beta[0-9]", Hint::None, &[b"beta2", b"beta", b"xbeta0y"]),
        ("(?:a?|b?|c?|d?){8}z", Hint::None, &[b"abcdabcd", b"abcdz", b"z"]),
        ("(?-u:\\xFF)", Hint::None, &[b"\xff", b"abc"]),
        ("", Hint::None, &[b"", b"\xff"]),
    ];
    for (source, hint, haystacks) in fixtures {
        let mut program = CompiledRegexProgram::compile_one(source, RegexProgramLimits::default()).unwrap();
        match hint {
            Hint::None => {}
            Hint::Simple => assert!(program.lower_simple(guard_budget()).unwrap().1),
            Hint::Guard => assert!(program.add_literal_guard(guard_budget()).unwrap().owned > 0),
        }
        let mut scratch = program.create_scratch();
        for &short in haystacks {
            for padding in [0_usize, 1024] {
                let mut input = vec![b'q'; padding];
                input.extend_from_slice(short);
                let mut eager = program.core.vm.create_cache();
                assert_eq!(
                    program.is_match(&input, &mut scratch).unwrap(),
                    program.core.vm.is_match(&mut eager, &input),
                    "source={source} padding={padding}"
                );
            }
        }
    }
}

#[test]
fn matcher_reference_scan_after_accelerated_builds_cache() {
    let patterns = [PatternSpec::Regex("^alpha$"), PatternSpec::Regex("^alpha$")];
    let mut matcher = CompiledMatcher::compile_pcre_bytes(&patterns, MatcherLimits::default()).unwrap();
    assert!(
        matcher
            .regex_program_mut()
            .unwrap()
            .lower_simple(guard_budget())
            .unwrap()
            .1
    );
    let mut scratch = matcher.create_scratch();
    let accelerated = ids(matcher.scan(b"alpha", &mut scratch).unwrap());
    assert_eq!(accelerated, [0, 1]);
    assert!(scratch.regex_scratch.as_ref().unwrap().cache.is_none());

    let reference = ids(matcher.scan_reference(b"alpha", &mut scratch).unwrap());
    assert_eq!(reference, accelerated);
    assert!(scratch.regex_scratch.as_ref().unwrap().cache.is_some(), "VM ran");
    assert_eq!(
        ids(matcher.scan_reference(b"nope", &mut scratch).unwrap()),
        Vec::<usize>::new()
    );
    assert_eq!(ids(matcher.scan(b"alpha", &mut scratch).unwrap()), accelerated);
}

#[test]
fn capture_metadata_limits_accept_exact_and_reject_first_excess() {
    let source = r"(?P<one>a)(?P<two>b)";
    let baseline = CompiledCaptureRegexProgram::compile_one(source, RegexProgramLimits::default()).unwrap();
    let exact_program = baseline.conservative_program_memory_bound();
    let exact_scratch = baseline.required_scratch_bytes();
    CompiledCaptureRegexProgram::compile_one(
        source,
        RegexProgramLimits {
            max_capture_slots: 3,
            max_named_groups: 2,
            max_capture_name_bytes: 3,
            max_total_capture_name_bytes: 6,
            max_scratch_bytes: exact_scratch,
            max_program_bytes: exact_program,
            ..RegexProgramLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        CompiledCaptureRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_capture_slots: 2,
                max_nfa_bytes: 0,
                ..RegexProgramLimits::default()
            },
        ),
        Err(RegexProgramError::CaptureSlotLimit { actual: 3, limit: 2 })
    ));
    assert!(matches!(
        CompiledCaptureRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_named_groups: 1,
                ..RegexProgramLimits::default()
            },
        ),
        Err(RegexProgramError::NamedGroupLimit { actual: 2, limit: 1 })
    ));
}

#[test]
fn nfa_limit_accepts_its_first_configured_boundary_and_has_a_distinct_error() {
    let source = "a(?:bc|de)+f";
    let mut rejected = 0_usize;
    let mut accepted = RegexProgramLimits::default().max_nfa_bytes;
    assert_eq!(
        CompiledRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_nfa_bytes: rejected,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::NfaByteLimit { limit: rejected }
    );
    while accepted - rejected > 1 {
        let candidate = rejected + (accepted - rejected) / 2;
        match CompiledRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_nfa_bytes: candidate,
                ..RegexProgramLimits::default()
            },
        ) {
            Ok(_) => accepted = candidate,
            Err(RegexProgramError::NfaByteLimit { .. }) => rejected = candidate,
            Err(error) => panic!("unexpected regex error: {error:?}"),
        }
    }
    CompiledRegexProgram::compile_one(
        source,
        RegexProgramLimits {
            max_nfa_bytes: accepted,
            ..RegexProgramLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        CompiledRegexProgram::compile_one(
            source,
            RegexProgramLimits {
                max_nfa_bytes: rejected,
                ..RegexProgramLimits::default()
            },
        )
        .unwrap_err(),
        RegexProgramError::NfaByteLimit { limit: rejected }
    );
}

#[test]
fn regex_peak_formula_and_program_reports_are_stable() {
    let limits = RegexProgramLimits {
        max_source_bytes: 11,
        max_nfa_bytes: 13,
        max_program_bytes: 1_000_000,
        max_scratch_bytes: 1_000_000,
        ..RegexProgramLimits::default()
    };
    assert!(CompiledRegexProgram::prebuild_peak(limits).unwrap() > 2 * 13 + 2_000_000);
    let program = CompiledRegexProgram::compile_one("a", RegexProgramLimits::default()).unwrap();
    assert!(program.memory_usage() > 0);
    let capture = CompiledCaptureRegexProgram::compile_one("(a)", RegexProgramLimits::default()).unwrap();
    assert!(capture.estimated_peak_build_memory_usage() > program.estimated_peak_build_memory_usage());
    assert_eq!(
        capture.memory_usage(),
        capture.engine_memory_usage() + capture.capture_metadata_memory_usage()
    );
    for source in ["a", r"\p{Greek}", "(?i:[a-z&&[^aeiou]])"] {
        let limits = RegexProgramLimits {
            max_source_bytes: source.len(),
            ..RegexProgramLimits::default()
        };
        let prebuild = CompiledRegexProgram::prebuild_peak(limits).unwrap();
        let compiled = CompiledRegexProgram::compile_one(source, limits).unwrap();
        assert!(prebuild >= compiled.estimated_peak_build_memory_usage());
    }
}

fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    *state
}

#[test]
fn member_pattern_ranges_preserve_word_boundaries_and_reject_invalid_ranges() {
    let patterns = (0..130).map(|i| format!("#{i:03}#")).collect::<Vec<_>>();
    let specs = patterns
        .iter()
        .map(|value| PatternSpec::Phrase {
            value,
            ascii_case_insensitive: false,
        })
        .collect::<Vec<_>>();
    let matcher = CompiledMatcher::compile(&specs, MatcherLimits::default()).unwrap();
    let mut scratch = matcher.create_scratch();
    for reference in [false, true] {
        let results = if reference {
            matcher.scan_reference(b"#000# #064# #129#", &mut scratch)
        } else {
            matcher.scan(b"#000# #064# #129#", &mut scratch)
        }
        .unwrap();
        let cases = [
            (0..0, Some(false)),
            (0..1, Some(true)),
            (1..64, Some(false)),
            (63..65, Some(true)),
            (65..129, Some(false)),
            (129..130, Some(true)),
            (130..130, Some(false)),
            (0..130, Some(true)),
            (130..131, None),
            (usize::MAX..usize::MAX, None),
        ];
        for (range, expected) in &cases {
            assert_eq!(results.contains_any_in_range(range.clone()), *expected);
        }
        assert_eq!(
            results.contains_any_in_range(std::ops::Range { start: 65, end: 64 }),
            None
        );
        for (range, expected) in cases {
            assert_eq!(scratch.contains_any_in_range(130, range), expected);
        }
        assert_eq!(scratch.contains_any_in_range(129, 0..1), None);
        assert!(matcher.scan(b"none", &mut scratch).unwrap().is_empty());
        assert_eq!(scratch.contains_any_in_range(130, 0..130), Some(false));
    }
}

#[test]
fn pattern_range_queries_match_individual_bits_for_all_boundaries() {
    let word_sets = [
        [0; 4],
        [u64::MAX; 4],
        [0xaaaa_aaaa_aaaa_aaaa; 4],
        [1, 1 << 63, 0, 1],
        [1 << 63, 1, 1 << 63, 0],
        [0, 0, 0, u64::MAX],
    ];
    for words in word_sets {
        for stored_words in [0, 1, 2, 4] {
            for pattern_count in [0, 1, 63, 64, 65, 127, 128, 130, 193] {
                let results = MatchResults {
                    matched_words: &words[..stored_words],
                    pattern_count,
                };
                let expected_ids = (0..pattern_count)
                    .map(PatternId)
                    .filter(|&id| results.contains(id))
                    .collect::<Vec<_>>();
                assert_eq!(results.iter().collect::<Vec<_>>(), expected_ids);
                assert_eq!(
                    results.iter().take(2).collect::<Vec<_>>(),
                    expected_ids.iter().copied().take(2).collect::<Vec<_>>()
                );
                for start in 0..=pattern_count {
                    for end in start..=pattern_count {
                        let expected = (start..end).any(|id| results.contains(PatternId(id)));
                        assert_eq!(
                            results.contains_any_in_range(start..end),
                            Some(expected),
                            "words={words:?}, stored={stored_words}, count={pattern_count}, range={start}..{end}"
                        );
                    }
                }
                assert_eq!(results.contains_any_in_range(0..pattern_count + 1), None);
                assert_eq!(
                    results.contains_any_in_range(std::ops::Range { start: 1, end: 0 }),
                    None
                );
                assert_eq!(results.contains_any_in_range(usize::MAX..usize::MAX), None);
            }
        }
    }
}

fn repetition_work_of(source: &str) -> usize {
    let ast = ast::parse::ParserBuilder::new()
        .nest_limit(RegexProgramLimits::default().max_nest_depth)
        .octal(false)
        .build()
        .parse(source)
        .unwrap();
    let hir = hir::translate::TranslatorBuilder::new()
        .utf8(false)
        .unicode(true)
        .build()
        .translate(source, &ast)
        .unwrap();
    hir_metrics(&hir, source.len() + 1).unwrap().repetition_work
}

#[test]
fn nested_counted_repetition_work_is_multiplicative() {
    // Flat repetitions keep their previous estimate.
    assert_eq!(repetition_work_of("a{1000}"), 1000);
    assert_eq!(repetition_work_of("(?:ab)+"), 2);
    // Nested counted repetition composes multiplicatively.
    assert!(repetition_work_of("(?:a{100}){100}") >= 10_000);
}

#[test]
fn nested_counted_repetition_is_rejected_before_the_nfa_build() {
    for source in [
        "(a{1000}){1000}",
        "((a{1000}){1000}){1000}",
        "(?:(?:(?:(?:(?:(?:a{1000}){1000}){1000}){1000}){1000}){1000}){1000}",
    ] {
        let error = CompiledRegexProgram::compile_one(source, RegexProgramLimits::default()).unwrap_err();
        assert!(
            matches!(error, RegexProgramError::RepetitionWorkLimit { actual, limit } if actual > limit),
            "{source}: unexpected {error:?}"
        );
    }
}

#[test]
fn single_level_counted_repetition_within_the_limit_still_compiles() {
    CompiledRegexProgram::compile_one("(?:ab){1,1000}x[a-z]{0,64}", RegexProgramLimits::default()).unwrap();
    CompiledRegexProgram::compile_one("(?:a{10}){10}", RegexProgramLimits::default()).unwrap();
}

fn eager_capture_groups(program: &CompiledCaptureRegexProgram, haystack: &[u8]) -> Vec<Option<Range<usize>>> {
    let mut cache = program.core.vm.create_cache();
    let mut captures = program.core.vm.create_captures();
    program.core.vm.captures(&mut cache, haystack, &mut captures);
    (0..program.capture_len())
        .map(|index| captures.get_group(index).map(|span| span.start..span.end))
        .collect()
}

fn lazy_capture_groups(
    program: &CompiledCaptureRegexProgram,
    haystack: &[u8],
    scratch: &mut RegexCaptureScratch,
) -> Vec<Option<Range<usize>>> {
    let results = program.captures(haystack, scratch).unwrap();
    (0..program.capture_len()).map(|index| results.group(index)).collect()
}

#[test]
fn capture_decision_vm_is_capture_free_and_never_larger() {
    let program =
        CompiledCaptureRegexProgram::compile_one(r"(?P<word>a(b)?)|(c)(d(e))", RegexProgramLimits::default()).unwrap();
    let decision = program.decision.core.vm.get_nfa();
    assert_eq!(decision.group_info().slot_len(), 0, "decision NFA tracks no slots");
    assert!(decision.states().len() < program.core.vm.get_nfa().states().len());
    assert!(Arc::ptr_eq(&program.decision.core.token, &program.core.token));
    assert_eq!(
        program.memory_usage(),
        program.engine_memory_usage() + program.capture_metadata_memory_usage()
    );
    assert!(program.engine_memory_usage() > program.core.retained_bytes);
}

#[test]
fn capture_guard_miss_and_exact_hint_skip_the_vm() {
    let mut guarded = CompiledCaptureRegexProgram::compile_one(r"alpha([0-9])", RegexProgramLimits::default()).unwrap();
    assert!(guarded.decision.add_literal_guard(guard_budget()).unwrap().owned > 0);
    let mut scratch = guarded.create_scratch();
    assert_eq!(scratch.test_built(), (false, false));
    let miss = vec![b'z'; 1024];
    assert!(!guarded.decision.may_match(&miss));
    assert!(!guarded.is_match(&miss, &mut scratch).unwrap());
    assert_eq!(
        scratch.test_built(),
        (false, false),
        "guard miss must not build the VM cache"
    );
    let mut hit = miss.clone();
    hit.extend_from_slice(b"alpha7");
    assert!(guarded.is_match(&hit, &mut scratch).unwrap());
    assert_eq!(
        scratch.test_built(),
        (true, false),
        "guard hit confirms through the decision VM"
    );
    assert_eq!(
        lazy_capture_groups(&guarded, &hit, &mut scratch),
        [Some(1024..1030), Some(1029..1030)]
    );

    let mut exact = CompiledCaptureRegexProgram::compile_one(r"^(abc)$", RegexProgramLimits::default()).unwrap();
    assert!(exact.decision.lower_simple(guard_budget()).unwrap().1);
    let mut scratch = exact.create_scratch();
    assert!(exact.is_match(b"abc", &mut scratch).unwrap());
    assert!(!exact.is_match(b"xabc", &mut scratch).unwrap());
    assert!(!exact.is_match(b"", &mut scratch).unwrap());
    assert_eq!(
        scratch.test_built(),
        (false, false),
        "exact hint must not build the VM cache"
    );
    assert_eq!(
        lazy_capture_groups(&exact, b"abc", &mut scratch),
        [Some(0..3), Some(0..3)]
    );
    assert_eq!(scratch.test_built(), (true, true));
}

#[test]
fn capture_decisions_without_capture_search_never_build_capture_slots() {
    let program =
        CompiledCaptureRegexProgram::compile_one(r"(?:(a?)|(b?)){4}z", RegexProgramLimits::default()).unwrap();
    let mut scratch = program.create_scratch();
    for haystack in [b"abab".as_slice(), b"abz", b"", b"zzzz"] {
        let mut eager = program.core.vm.create_cache();
        assert_eq!(
            program.is_match(haystack, &mut scratch).unwrap(),
            program.core.vm.is_match(&mut eager, haystack)
        );
    }
    assert_eq!(
        scratch.test_built(),
        (true, false),
        "decision-only use builds the decision cache but never the capture slots"
    );
    let foreign = CompiledCaptureRegexProgram::compile_one(r"(a)", RegexProgramLimits::default()).unwrap();
    let mut foreign_scratch = foreign.create_scratch();
    assert_eq!(
        program.is_match(b"z", &mut foreign_scratch),
        Err(RegexScratchError::ScratchShape)
    );
    assert!(program.captures(b"z", &mut foreign_scratch).is_err());
    assert_eq!(
        foreign_scratch.test_built(),
        (false, false),
        "owner check precedes cache creation"
    );
}

#[test]
fn capture_groups_and_decisions_equal_eager_reference_across_mode_switches() {
    let fixtures: [(&str, &[&[u8]]); 6] = [
        (
            r"(a)(b)(c)(d)(e)(f)(g)(h)(i)",
            &[b"abcdefghi", b"xxabcdefghiyy", b"abcdefgh"],
        ),
        (r"(?P<word>a(b)?)|(c)(d(e))", &[b"ab", b"a", b"cde", b"xcdex", b"q"]),
        (r"(?i)(select|union)\s+(all)?", &[b"SELECT ", b"union all", b"selec"]),
        (r"^(\w+)=(.*)$", &[b"key=value", b"=", b"key="]),
        (r"(?-u:(\xFF+))", &[b"\xff\xff", b"abc"]),
        (r"()", &[b"", b"\xff"]),
    ];
    for (source, haystacks) in fixtures {
        let program = CompiledCaptureRegexProgram::compile_one(source, RegexProgramLimits::default()).unwrap();
        let mut scratch = program.create_scratch();
        // Decide, capture, then decide again on the capture-sized cache.
        for round in 0..2 {
            for &short in haystacks {
                for padding in [0_usize, 1024] {
                    let mut input = vec![b'q'; padding];
                    input.extend_from_slice(short);
                    let mut eager = program.core.vm.create_cache();
                    let expected = program.core.vm.is_match(&mut eager, &input);
                    assert_eq!(
                        program.is_match(&input, &mut scratch).unwrap(),
                        expected,
                        "source={source} padding={padding} round={round}"
                    );
                    if expected {
                        assert_eq!(
                            lazy_capture_groups(&program, &input, &mut scratch),
                            eager_capture_groups(&program, &input),
                            "source={source} padding={padding} round={round}"
                        );
                    }
                }
            }
        }
    }
    let ten = CompiledCaptureRegexProgram::compile_one(r"(a)(b)(c)(d)(e)(f)(g)(h)(i)", RegexProgramLimits::default())
        .unwrap();
    let mut scratch = ten.create_scratch();
    let groups = lazy_capture_groups(&ten, b"abcdefghi", &mut scratch);
    assert_eq!(groups.len(), 10, "TX.0..TX.9");
    assert_eq!(groups[0], Some(0..9));
    for (index, group) in groups.iter().enumerate().skip(1) {
        assert_eq!(*group, Some(index - 1..index));
    }
}

#[test]
fn capture_scratch_admission_is_not_larger_than_the_eager_layout() {
    // The lazy wrapper fits the old eager layout (`Option` uses each member's
    // niche), so the transaction precharge did not grow.
    assert!(
        size_of::<RegexCaptureScratch>()
            <= size_of::<Arc<CaptureShapeToken>>() + size_of::<PikeCache>() + size_of::<Captures>()
    );
    for source in [r"(a)", r"(?:(a?)|(b?)|(c?)|(d?)){8}z", r"(?P<word>a(b)?)|(c)(d(e))"] {
        let program = CompiledCaptureRegexProgram::compile_one(source, RegexProgramLimits::default()).unwrap();
        let eager_cache = program.core.vm.create_cache();
        let eager_slots = program.core.vm.create_captures().slots().len();
        let states = program.core.vm.get_nfa().states().len();
        let eager_layout = size_of::<Arc<CaptureShapeToken>>()
            + size_of::<PikeCache>()
            + size_of::<Captures>()
            + eager_cache.memory_usage()
            + eager_slots * size_of::<usize>()
            + states * 4 * FOLLOW_EPSILON_UPPER_BYTES;
        assert!(program.required_scratch_bytes() <= eager_layout, "source={source}");
        // A decision-sized cache regrows to the capture VM inside its capacity.
        let mut scratch = program.create_scratch();
        let _ = program.is_match(b"zzzz", &mut scratch).unwrap();
        let _ = program.captures(b"zzzz", &mut scratch).unwrap();
        let actual = size_of::<RegexCaptureScratch>()
            + scratch.cache.as_ref().unwrap().memory_usage()
            + scratch.captures.as_ref().unwrap().slots().len() * size_of::<usize>();
        assert!(actual <= program.required_scratch_bytes(), "source={source}");
    }
}
