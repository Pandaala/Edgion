use super::*;

fn ids(results: MatchResults<'_>) -> Vec<usize> {
    results.iter().map(PatternId::as_usize).collect()
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
        3 * size_of::<(PatternId, &str)>() + 3 * size_of::<&str>()
    );
    assert_eq!(projection.retained_map_bytes(), 6 * size_of::<PatternId>());
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
    let boolean_actual = size_of::<RegexBooleanScratch>() + boolean_scratch.cache.memory_usage();
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
        + capture_scratch.cache.memory_usage()
        + capture_scratch.captures.slots().len() * size_of::<usize>();
    assert!(capture_actual <= capture_required);
}

#[test]
fn capture_metadata_limits_accept_exact_and_reject_first_excess() {
    let source = r"(?P<one>a)(?P<two>b)";
    let baseline = CompiledCaptureRegexProgram::compile_one(source, RegexProgramLimits::default()).unwrap();
    let exact_program = baseline.core.program_peak_bound;
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
