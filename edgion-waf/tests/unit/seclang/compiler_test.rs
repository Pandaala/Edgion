use super::*;
use crate::seclang::{
    analyze_one_semantic_draft, parse_syntax, scan_source, validate_semantic_draft, PhaseDefaults, SemanticLimits,
    SemanticValidationContext, SourceLimits, SyntaxLimits, ValidationLimits,
};

fn context() -> SemanticValidationContext {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap()
}

fn with_validated<T>(text: &str, inspect: impl FnOnce(ValidatedSemanticDocument<'_>) -> T) -> T {
    let source = scan_source("compiler-test", text.as_bytes(), SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).unwrap();
    let validated = validate_semantic_draft(draft, context(), ValidationLimits::default()).unwrap();
    inspect(validated)
}

#[test]
fn original_supported_bundle_keeps_logical_rules_with_shared_physical_regexes() {
    use crate::seclang::{compile_rule_bundles, FrontendLimits, RuleBundleMember, RuleBundleSource};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let bundles = ["standard", "core"].map(|profile| {
        let path = root.join(format!(
            "config/waf/crs-4.25.1-supported-v1/generated/EdgionConfigData_crs-supported-{profile}.yaml"
        ));
        let value: serde_yaml::Value = serde_yaml::from_slice(&std::fs::read(path).unwrap()).unwrap();
        value["spec"]["data"]["config"].clone()
    });
    let roots = bundles.each_ref().map(|bundle| {
        bundle["roots"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|root| root.as_str().unwrap())
            .collect::<Vec<_>>()
    });
    let rules = bundles.each_ref().map(|bundle| {
        bundle["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|member| RuleBundleMember {
                path: member["name"].as_str().unwrap(),
                bytes: member["content"].as_str().unwrap().as_bytes(),
            })
            .collect::<Vec<_>>()
    });
    let assets = bundles.each_ref().map(|bundle| {
        bundle["phraseAssets"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|member| RuleBundleMember {
                path: member["name"].as_str().unwrap(),
                bytes: member["content"].as_str().unwrap().as_bytes(),
            })
            .collect::<Vec<_>>()
    });
    let sources = [0, 1].map(|index| RuleBundleSource {
        identity: bundles[index]["digest"].as_str().unwrap(),
        entrypoints: &roots[index],
        rules: &rules[index],
        phrase_assets: &assets[index],
    });
    let (generation, _) = compile_rule_bundles(&sources, "", context(), FrontendLimits::default())
        .unwrap()
        .into_parts();
    assert_eq!(generation.rule_unit_count(), 567);
    assert_eq!(generation.members.len(), 634);
    assert_eq!(generation.groups.len(), 36);
    let counts = generation
        .groups
        .iter()
        .map(|group| group.matcher.test_regex_counts())
        .fold((0, 0), |(a, b), (logical, physical)| (a + logical, b + physical));
    assert_eq!(counts, (66, 60));
    let guards = generation
        .boolean_regexes
        .iter()
        .map(CompiledRegexProgram::test_guard_stats)
        .chain(generation.all_matchers().map(CompiledMatcher::test_guard_stats))
        .fold((0, 0), |(a, b), (c, d)| (a + c, b + d));
    println!("Supported prefix guards: count={} owned={}", guards.0, guards.1);
    let simple = generation
        .boolean_regexes
        .iter()
        .map(CompiledRegexProgram::test_simple_count)
        .chain(generation.all_matchers().map(CompiledMatcher::test_simple_count))
        .sum::<usize>();
    println!("Supported exact simple programs: count={simple}");
    assert!(!generation.capture_hits.matchers.is_empty());
    let optional_scratch_envelope = generation
        .capture_hits
        .matchers
        .iter()
        .map(|m| {
            m.required_scratch_bytes() - size_of::<crate::matcher::MatchScratch>()
                + size_of::<Option<crate::matcher::MatchScratch>>()
        })
        .sum::<usize>();
    let optional_engine_bytes = generation
        .capture_hits
        .matchers
        .iter()
        .map(|m| m.memory_usage())
        .sum::<usize>();
    println!("Supported capture memory: engine_bytes={optional_engine_bytes} additional_scratch_envelope={optional_scratch_envelope} additional_empty_slot_bytes={}", generation.capture_hits.matchers.len() * size_of::<Option<crate::matcher::MatchScratch>>());
    let capture_patterns = generation
        .capture_hits
        .matchers
        .iter()
        .map(|m| m.test_regex_counts())
        .fold((0, 0), |(a, b), (c, d)| (a + c, b + d));
    println!(
        "Supported capture witness: engines={} logical={} physical={} json_batches={} xml_batches={}",
        generation.capture_hits.matchers.len(),
        capture_patterns.0,
        capture_patterns.1,
        generation.json_inspection.capture_hits.batches.len(),
        generation.xml_inspection.capture_hits.batches.len()
    );
    println!(
        "Supported regex witness: logical={} physical={} retained={} compile_peak={}",
        counts.0,
        counts.1,
        generation.retained_bytes(),
        generation.compilation_peak_bytes()
    );
}

#[test]
fn final_prefix_guard_attachment_preserves_required_generation_at_tight_global_caps() {
    let source = concat!(
        "SecRule REQUEST_URI \"@rx alpha[0-9]\" \"id:701,phase:1,pass\"\n",
        "SecRule REQUEST_URI \"@rx beta[0-9]\" \"id:702,phase:1,pass\"\n",
    );
    let g = with_validated(source, |v| {
        compile_validated(v, None, CompilerLimits::default()).unwrap()
    });
    let (count, owned) = g.groups[0].matcher.test_guard_stats();
    assert_eq!(count, 1);
    assert!(owned > 0);
    let required_matcher = g.matcher_charged_bytes - owned;
    for limits in [
        CompilerLimits {
            max_compiled_matcher_bytes: required_matcher,
            ..CompilerLimits::default()
        },
        CompilerLimits {
            max_generation_bytes: required_matcher + g.compiler_owned_bytes,
            ..CompilerLimits::default()
        },
    ] {
        let tight = with_validated(source, |v| compile_validated(v, None, limits).unwrap());
        assert_eq!(tight.groups[0].matcher.test_guard_stats(), (0, 0));
        assert_eq!(tight.matcher_charged_bytes, required_matcher);
        assert_eq!(tight.rule_unit_count(), 2);
        assert!(tight.compilation_peak_bytes() <= limits.max_compilation_peak_bytes);
    }
}

#[test]
fn final_prefix_guard_attachment_has_exact_standalone_and_group_program_caps() {
    for (source, units) in [
        ("SecRule ARGS:/probe[0-9]/ \"@rx beta[0-9]\" \"id:703,phase:2,pass\"\n", 1),
        (concat!(
            "SecRule REQUEST_URI \"@contains yes\" \"id:703,phase:1,pass,ctl:ruleRemoveTargetByTag=probe;ARGS:/probe[0-9]/\"\n",
            "SecRule ARGS \"@rx beta[0-9]\" \"id:704,phase:2,pass,tag:probe\"\n",
        ), 2),
    ] {
    let mut baseline = with_validated(source, |v| {
        compile_validated(v, None, CompilerLimits::default()).unwrap()
    });
    for standalone in [true, false] {
        let program = if standalone {
            &mut baseline.boolean_regexes[0]
        } else {
            baseline.groups[0].matcher.regex_program_mut().unwrap()
        };
        let (_, owned) = program.test_guard_stats();
        assert!(owned > 0);
        let cap = program.conservative_program_memory_bound();
        for (bound, installs) in [(cap, true), (cap - 1, false)] {
            let mut limits = CompilerLimits::default();
            if standalone {
                limits.regex.max_program_bytes = bound;
            } else {
                limits.matcher.max_regex_program_bytes = bound;
            }
            let mut g = with_validated(source, |v| compile_validated(v, None, limits).unwrap());
            let actual = if standalone {
                &mut g.boolean_regexes[0]
            } else {
                g.groups[0].matcher.regex_program_mut().unwrap()
            };
            assert_eq!(actual.test_guard_stats().0 == 1, installs);
            assert!(actual.conservative_program_memory_bound() <= bound);
            assert_eq!(g.rule_unit_count(), units);
        }
    }
    }
}

#[test]
fn capture_regex_sharing_preserves_occurrence_gates_and_requirement_discharge() {
    let text = concat!(
        "SecRule ARGS \"@rx (?P<word>a)\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"!@rx (?P<word>a)\" \"id:2,phase:2,pass,capture,t:lowercase\"\n",
        "SecRule ARGS \"@rx (?P<word>a)\" \"id:3,phase:2,pass,capture\"\n",
        "SecRuleRemoveById 3\n",
    );
    with_validated(text, |validated| {
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let state = MatcherState::compile(prepared, CompilerLimits::default()).unwrap();
        assert_eq!(state.capture_regexes.len(), 1);
        for plan in &state.operator_plans[..2] {
            assert!(matches!(
                plan.unwrap().operand,
                CompiledOperatorOperand::Regex(RegexPlanRef::Capture(0))
            ));
        }
        assert!(!state.operator_plans[0].unwrap().negated);
        assert!(state.operator_plans[1].unwrap().negated);
        assert!(state.operator_plans[2].is_none());
        assert!(state
            .asset_templates
            .requirements
            .iter()
            .all(|requirement| requirement.discharged));
    });
    with_validated(text, |validated| {
        let limits = CompilerLimits {
            max_regex_programs: 1,
            ..CompilerLimits::default()
        };
        let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
        assert!(matches!(
            MatcherState::compile(prepared, limits),
            Err(CompilerError::RegexProgramLimit { actual: 2, limit: 1 })
        ));
    });
    with_validated(
        "SecRule ARGS \"@rx [\" \"id:1,phase:2,pass,capture\"\nSecRuleRemoveById 1\n",
        |validated| {
            let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
                .prepare()
                .unwrap();
            assert!(matches!(
                MatcherState::compile(prepared, CompilerLimits::default()),
                Err(CompilerError::RegexProgram { .. })
            ));
        },
    );
}

#[test]
fn capture_regex_identity_includes_exact_flags_and_named_group_source() {
    let text = concat!(
        "SecRule ARGS \"@rx (?P<word>a)\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (?P<other>a)\" \"id:2,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (?i)(?P<word>a)\" \"id:3,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (?P<word>a)\" \"id:4,phase:2,pass,capture\"\n",
    );
    with_validated(text, |validated| {
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let state = MatcherState::compile(prepared, CompilerLimits::default()).unwrap();
        assert_eq!(state.capture_regexes.len(), 3);
        let ids = state
            .operator_plans
            .iter()
            .map(|plan| match plan.unwrap().operand {
                CompiledOperatorOperand::Regex(RegexPlanRef::Capture(id)) => id,
                _ => panic!("expected capture regex"),
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, [0, 1, 2, 0]);
    });
}

#[test]
fn capture_regex_interner_skips_compatibility_excluded_units() {
    let text = concat!(
        "SecRule ARGS \"@rx [\" \"id:1,phase:2,pass,capture,initcol:global=key\"\n",
        "SecRule ARGS \"@rx (a)\" \"id:2,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (a)\" \"id:3,phase:2,pass,capture\"\n",
    );
    let source = scan_source("compiler-test", text.as_bytes(), SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).unwrap();
    let validated = crate::seclang::validate_executable_profile(draft, context(), ValidationLimits::default()).unwrap();
    assert!(validated.rule_units().next().unwrap().is_compatibility_excluded());
    let limits = CompilerLimits {
        max_regex_programs: 2,
        ..CompilerLimits::default()
    };
    let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
    let state = MatcherState::compile(prepared, limits).unwrap();
    assert_eq!(state.capture_regexes.len(), 1);
    assert!(state.operator_plans[0].is_none());
    assert!(state
        .asset_templates
        .requirements
        .iter()
        .all(|requirement| requirement.discharged));
}

#[test]
fn capture_regex_interner_charges_all_work_and_carries_it_into_grouping() {
    let text = concat!(
        "SecRule ARGS \"@rx (a)\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (a)\" \"id:2,phase:2,pass,capture\"\n",
    );
    for (limits, error) in [
        (
            CompilerLimits {
                max_group_key_hash_bytes: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupKeyHashByteLimit { limit: 0 },
        ),
        (
            CompilerLimits {
                max_group_table_probes: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupTableProbeLimit { limit: 0 },
        ),
        (
            CompilerLimits {
                max_group_key_record_comparisons: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupKeyRecordComparisonLimit { limit: 0 },
        ),
        (
            CompilerLimits {
                max_group_key_comparison_bytes: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupKeyComparisonByteLimit { limit: 0 },
        ),
    ] {
        with_validated(text, |validated| {
            let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
            assert_eq!(MatcherState::compile(prepared, limits).unwrap_err(), error);
        });
    }
    with_validated(text, |validated| {
        let limits = CompilerLimits::default();
        let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
        let state = MatcherState::compile(prepared, limits).unwrap();
        assert_eq!(state.canonical_work.hash_bytes, 6 + 2 * size_of::<u64>());
        assert_eq!(state.canonical_work.comparison_bytes, 3);
        let limits = CompilerLimits {
            max_group_key_hash_bytes: state.canonical_work.hash_bytes,
            ..limits
        };
        assert!(matches!(
            GroupedMatcherState::compile(state, limits),
            Err(CompilerError::GroupKeyHashByteLimit { .. })
        ));
    });
    with_validated(text, |validated| {
        let limits = CompilerLimits::default();
        let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
        let state = MatcherState::compile(prepared, limits).unwrap();
        let grouped = GroupedMatcherState::compile(state, limits).unwrap();
        let limits = CompilerLimits {
            max_group_key_hash_bytes: grouped.canonical_work.hash_bytes,
            ..limits
        };
        assert!(matches!(
            CompiledGeneration::publish(grouped, limits),
            Err(CompilerError::GroupKeyHashByteLimit { .. })
        ));
    });
}

#[test]
fn capture_regex_interner_collision_checks_bytes_and_keeps_actual_capacity_charged() {
    with_validated("SecRule ARGS \"@rx a\" \"id:1,phase:2,pass,capture\"\n", |validated| {
        let location = validated
            .rule_units()
            .next()
            .unwrap()
            .root()
            .operator()
            .unwrap()
            .location();
        let mut memory = MatcherCompileWork {
            limits: CompilerLimits::default(),
            validated_bytes: 0,
            working_bytes: 0,
            compiler_owned_bytes: 0,
            matcher_charged_bytes: 0,
            peak_bytes: 0,
            requirement_cursors: [0; 8],
        };
        let mut interner = RegexProgramInterner::new(2, &mut memory).unwrap();
        let index_bytes = interner.slots.capacity() * size_of::<CanonicalSlot>()
            + interner.sources.capacity() * size_of::<RegexProgramSource<'_>>();
        assert_eq!(memory.working_bytes, index_bytes);
        assert!(memory.regex_peak_budget().unwrap().0 >= index_bytes);
        let mut programs = Vec::with_capacity(2);
        let mut work = CanonicalWork::default();
        assert_eq!(
            interner
                .compile("a", location, &mut programs, &mut memory, &mut work)
                .unwrap(),
            0
        );
        // Force a full hash collision to exercise the equality barrier independently of FNV.
        let mut hash = FNV1A_OFFSET_BASIS;
        CanonicalWork::default().hash_u64(&mut hash, 0, memory.limits).unwrap();
        CanonicalWork::default()
            .hash_bytes(&mut hash, b"b", memory.limits)
            .unwrap();
        interner.slots.fill(CanonicalSlot::EMPTY);
        let position = hash as usize & (interner.slots.len() - 1);
        interner.slots[position] = CanonicalSlot { hash, id: 0 };
        memory.limits.max_group_key_comparison_bytes = 0;
        assert!(matches!(
            interner.compile("b", location, &mut programs, &mut memory, &mut work),
            Err(CompilerError::GroupKeyComparisonByteLimit { .. })
        ));
        assert_eq!(programs.len(), 1);
        assert_eq!(interner.sources.len(), 1);
        memory.limits = CompilerLimits::default();
        assert_eq!(
            interner
                .compile("b", location, &mut programs, &mut memory, &mut work)
                .unwrap(),
            1
        );
        assert!(work.probes > 2);
        assert_eq!(programs.len(), 2);
        interner.release(&mut memory);
        assert_eq!(memory.working_bytes, 0);
    });
}

#[test]
fn capture_regex_interner_peak_limit_is_exact_with_distinct_programs() {
    let text = concat!(
        "SecRule ARGS \"@rx (a)\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (b)\" \"id:2,phase:2,pass,capture\"\n",
    );
    let peak = with_validated(text, |validated| {
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        MatcherState::compile(prepared, CompilerLimits::default())
            .unwrap()
            .peak_bytes
    });
    for (bound, succeeds) in [(peak, true), (peak - 1, false)] {
        with_validated(text, |validated| {
            let limits = CompilerLimits {
                max_compilation_peak_bytes: bound,
                ..CompilerLimits::default()
            };
            let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
            let result = MatcherState::compile(prepared, limits);
            assert_eq!(result.is_ok(), succeeds, "{result:?}");
            if !succeeds {
                assert!(matches!(result, Err(CompilerError::PeakByteLimit { .. })));
            }
        });
    }
}

#[test]
fn regex_interner_profile_collision_and_failed_build_do_not_share_or_insert() {
    let mut memory = MatcherCompileWork {
        limits: CompilerLimits::default(),
        validated_bytes: 0,
        working_bytes: 0,
        compiler_owned_bytes: 0,
        matcher_charged_bytes: 0,
        peak_bytes: 0,
        requirement_cursors: [0; 8],
    };
    let mut interner = RegexProgramInterner::new(3, &mut memory).unwrap();
    let mut programs = Vec::with_capacity(3);
    let mut work = CanonicalWork::default();
    assert_eq!(
        interner
            .compile_with("a", true, &mut programs, &mut memory, &mut work, |_| Ok(10))
            .unwrap(),
        0
    );
    let mut hash = FNV1A_OFFSET_BASIS;
    CanonicalWork::default().hash_u64(&mut hash, 0, memory.limits).unwrap();
    CanonicalWork::default()
        .hash_bytes(&mut hash, b"a", memory.limits)
        .unwrap();
    interner.slots.fill(CanonicalSlot::EMPTY);
    let position = hash as usize & (interner.slots.len() - 1);
    interner.slots[position] = CanonicalSlot { hash, id: 0 };
    assert_eq!(
        interner
            .compile_with("a", false, &mut programs, &mut memory, &mut work, |_| Ok(20))
            .unwrap(),
        1
    );
    assert_eq!(programs, [10, 20]);
    assert_eq!(
        interner.compile_with("b", false, &mut programs, &mut memory, &mut work, |_| Err(
            CompilerError::PreflightArithmetic
        )),
        Err(CompilerError::PreflightArithmetic)
    );
    assert_eq!(interner.sources.len(), 2);
    assert_eq!(programs.len(), 2);
    assert_eq!(
        interner
            .compile_with("b", false, &mut programs, &mut memory, &mut work, |_| Ok(30))
            .unwrap(),
        2
    );
    interner.release(&mut memory);
    assert_eq!(memory.working_bytes, 0);
}

struct AssetProvider {
    chunks: Vec<&'static [u8]>,
    failure: Option<PhraseAssetFailure>,
    ignore_sink_errors: bool,
}

impl PhraseAssetSource for AssetProvider {
    fn load(
        &mut self,
        request: PhraseAssetRequest<'_>,
        sink: &mut PhraseAssetSink<'_>,
    ) -> Result<(), PhraseAssetFailure> {
        assert!(!format!("{request:?}").contains(request.handle()));
        for chunk in &self.chunks {
            let result = sink.extend_from_slice(chunk);
            if !self.ignore_sink_errors {
                result.map_err(|_| PhraseAssetFailure::Other)?;
            }
        }
        self.failure.map_or(Ok(()), Err)
    }
}

#[test]
fn asset_sink_is_sticky_and_precedes_provider_failure() {
    let text = "SecRule ARGS \"@pmFromFile phrases.data\" \"id:1,phase:2,pass\"\n";
    with_validated(text, |validated| {
        let mut provider = AssetProvider {
            chunks: vec![b"abc", b"def"],
            failure: Some(PhraseAssetFailure::Denied),
            ignore_sink_errors: true,
        };
        let error = CompilerBuilder::new(
            validated,
            Some(&mut provider),
            CompilerLimits {
                max_asset_bytes: 4,
                ..CompilerLimits::default()
            },
        )
        .prepare()
        .unwrap_err();
        assert!(matches!(
            error,
            CompilerError::AssetSink {
                error: PhraseAssetSinkError::AssetByteLimit { actual: 6, limit: 4 },
                ..
            }
        ));
    });
}

#[test]
fn asset_normalization_is_owned_bounded_and_content_free() {
    let text = "SecRule ARGS \"@pmFromFile secret.data\" \"id:1,phase:2,pass\"\n";
    with_validated(text, |validated| {
        let mut provider = AssetProvider {
            chunks: vec![b"  first \r\n# comment\n\tsecond\t\n"],
            failure: None,
            ignore_sink_errors: false,
        };
        let state = CompilerBuilder::new(validated, Some(&mut provider), CompilerLimits::default())
            .prepare()
            .unwrap();
        assert_eq!(state.assets.len(), 1);
        assert_eq!(state.phrases.len(), 2);
        let phrases = state
            .phrases
            .iter()
            .map(|phrase| {
                std::str::from_utf8(
                    &state.asset_phrase_bytes[phrase.bytes.start..phrase.bytes.start + phrase.bytes.len],
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(phrases, ["first", "second"]);
    });
    with_validated(text, |validated| {
        let error = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap_err();
        let rendered = format!("{error:?} {error}").to_ascii_lowercase();
        assert!(!rendered.contains("secret"));
    });
}

#[test]
fn templates_actions_and_setvar_are_owned_and_discharge_once() {
    let text = concat!(
        "SecRule ARGS \"@streq prefix-%{TX.key}\" ",
        "\"id:1,phase:2,pass,msg:'message-%{REQUEST_LINE}',setvar:'tx.%{TX.name}=+1'\"\n",
    );
    let exact_peak = with_validated(text, |validated| {
        let state = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        assert_eq!(state.members.len(), 1);
        assert_eq!(state.setvars.len(), 1);
        assert_eq!(state.macros.len(), 3);
        let exact_peak = state
            .validated
            .retained_charged_bytes()
            .checked_add(state.compiler_working_bytes)
            .and_then(|bytes| bytes.checked_add(state.compiler_owned_bytes))
            .and_then(|bytes| bytes.checked_add(3 * size_of::<(usize, usize)>()))
            .unwrap();
        assert_eq!(state.peak_bytes, exact_peak);
        assert_eq!(
            state
                .requirements
                .iter()
                .filter(|requirement| matches!(
                    requirement.kind,
                    CompileRequirement::RuntimeMacro | CompileRequirement::TransactionSetVar
                ))
                .map(|requirement| requirement.discharged)
                .collect::<Vec<_>>(),
            [true, true, true, true]
        );
        exact_peak
    });
    with_validated(text, |validated| {
        CompilerBuilder::new(
            validated,
            None,
            CompilerLimits {
                max_compilation_peak_bytes: exact_peak,
                ..CompilerLimits::default()
            },
        )
        .prepare()
        .unwrap();
    });
    with_validated(text, |validated| {
        assert!(matches!(
            CompilerBuilder::new(
                validated,
                None,
                CompilerLimits {
                    max_compilation_peak_bytes: exact_peak - 1,
                    ..CompilerLimits::default()
                },
            )
            .prepare(),
            Err(CompilerError::PeakByteLimit { .. })
        ));
    });
}

#[test]
fn validated_marker_and_member_seams_are_owner_bound() {
    let text = concat!(
        "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass,skipAfter:END\"\n",
        "SecMarker END\n",
    );
    with_validated(text, |validated| {
        let marker = validated.markers().next().unwrap();
        let member = validated.rule_by_id(1).unwrap().members().next().unwrap();
        assert_eq!(marker.ordinal(), 0);
        assert_eq!(member.skip_after_marker().unwrap().ordinal(), 0);
        assert!(member.operator().is_some());
        assert_eq!(member.actions().len(), 4);
        assert_eq!(member.effective_transformations().len(), 0);
    });
}

#[test]
fn preflight_limits_precede_assets_and_accept_exact_boundaries() {
    let text = "SecRule ARGS \"@pmFromFile phrases.data\" \"id:1,phase:2,pass,msg:'note'\"\n";
    with_validated(text, |validated| {
        let error = CompilerBuilder::new(
            validated,
            None,
            CompilerLimits {
                max_action_plans: 3,
                ..CompilerLimits::default()
            },
        )
        .prepare()
        .unwrap_err();
        assert_eq!(error, CompilerError::ActionLimit { actual: 4, limit: 3 });
    });
    with_validated("SecRule ARGS \"@streq abc\" \"id:1,phase:2,pass\"\n", |validated| {
        let baseline = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let exact = baseline.compiler_owned_bytes;
        assert!(exact > 0);
        drop(baseline);
        with_validated("SecRule ARGS \"@streq abc\" \"id:1,phase:2,pass\"\n", |validated| {
            CompilerBuilder::new(
                validated,
                None,
                CompilerLimits {
                    max_owned_template_metadata_bytes: exact,
                    max_compiler_owned_non_matcher_bytes: exact,
                    ..CompilerLimits::default()
                },
            )
            .prepare()
            .unwrap();
        });
        with_validated("SecRule ARGS \"@streq abc\" \"id:1,phase:2,pass\"\n", |validated| {
            assert!(matches!(
                CompilerBuilder::new(
                    validated,
                    None,
                    CompilerLimits {
                        max_owned_template_metadata_bytes: exact - 1,
                        max_compiler_owned_non_matcher_bytes: exact - 1,
                        ..CompilerLimits::default()
                    },
                )
                .prepare(),
                Err(CompilerError::OwnedByteLimit { .. })
            ));
        });
    });
}

#[test]
fn asset_grammar_and_phrase_limits_are_deterministic() {
    let text = "SecRule ARGS \"@pmFromFile phrases.data\" \"id:1,phase:2,pass\"\n";
    for (raw, expected) in [
        (&b"one\r"[..], "bare carriage"),
        (&b"a\rb\r\n"[..], "bare carriage"),
        (&b"good\r\nbad\rc\r\n"[..], "bare carriage"),
        (&b" \t\n# ignored\n"[..], "no phrases"),
        (&b"\xff"[..], "invalid UTF-8"),
    ] {
        with_validated(text, |validated| {
            let mut provider = AssetProvider {
                chunks: vec![raw],
                failure: None,
                ignore_sink_errors: false,
            };
            let error = CompilerBuilder::new(validated, Some(&mut provider), CompilerLimits::default())
                .prepare()
                .unwrap_err();
            assert!(format!("{error}").contains(expected));
        });
    }
    with_validated(text, |validated| {
        let mut provider = AssetProvider {
            chunks: vec![b"one\ntwo\n"],
            failure: None,
            ignore_sink_errors: false,
        };
        let error = CompilerBuilder::new(
            validated,
            Some(&mut provider),
            CompilerLimits {
                max_phrases_per_asset: 1,
                ..CompilerLimits::default()
            },
        )
        .prepare()
        .unwrap_err();
        assert!(matches!(
            error,
            CompilerError::AssetPhraseLimit {
                actual: 2,
                limit: 1,
                ..
            }
        ));
    });
}

struct CountingProvider {
    bytes: Vec<u8>,
    calls: usize,
}

impl PhraseAssetSource for CountingProvider {
    fn load(
        &mut self,
        _request: PhraseAssetRequest<'_>,
        sink: &mut PhraseAssetSink<'_>,
    ) -> Result<(), PhraseAssetFailure> {
        self.calls += 1;
        sink.extend_from_slice(&self.bytes)
            .map_err(|_| PhraseAssetFailure::Other)
    }
}

#[test]
fn normalization_peak_includes_live_raw_and_normalized_arenas_exactly() {
    let text = "SecRule ARGS \"@pmFromFile phrases.data\" \"id:1,phase:2,pass\"\n";
    let raw = vec![b'a'; 4_096];
    let (exact_peak, exact_formula) = with_validated(text, |validated| {
        let mut provider = CountingProvider {
            bytes: raw.clone(),
            calls: 0,
        };
        let state = CompilerBuilder::new(validated, Some(&mut provider), CompilerLimits::default())
            .prepare()
            .unwrap();
        assert_eq!(provider.calls, 1);
        let formula = state
            .validated
            .retained_charged_bytes()
            .checked_add(state.compiler_working_bytes)
            .and_then(|bytes| bytes.checked_add(raw.len()))
            .unwrap();
        assert_eq!(state.peak_bytes, formula);
        (state.peak_bytes, formula)
    });
    assert_eq!(exact_peak, exact_formula);
    with_validated(text, |validated| {
        let mut provider = CountingProvider {
            bytes: raw.clone(),
            calls: 0,
        };
        CompilerBuilder::new(
            validated,
            Some(&mut provider),
            CompilerLimits {
                max_compilation_peak_bytes: exact_peak,
                ..CompilerLimits::default()
            },
        )
        .prepare()
        .unwrap();
        assert_eq!(provider.calls, 1);
    });
    with_validated(text, |validated| {
        let mut provider = CountingProvider { bytes: raw, calls: 0 };
        let error = CompilerBuilder::new(
            validated,
            Some(&mut provider),
            CompilerLimits {
                max_compilation_peak_bytes: exact_peak - 1,
                ..CompilerLimits::default()
            },
        )
        .prepare()
        .unwrap_err();
        assert!(matches!(error, CompilerError::PeakByteLimit { .. }));
        assert_eq!(provider.calls, 1);
    });
}

#[test]
fn member_runtime_limit_and_reserve_precede_asset_callback() {
    let text = concat!(
        "SecRule ARGS \"@pmFromFile phrases.data\" ",
        "\"id:1,phase:2,pass,msg:'value-%{TX.key}',logdata:'other-%{TX.key}',",
        "tag:'tag-%{TX.key}',ver:'ver-%{TX.key}'\"\n",
    );
    let exact_preflight_peak = with_validated(text, |validated| {
        let limits = CompilerLimits {
            max_template_runtime_bytes: 10,
            max_rule_runtime_expansion_bytes: 20,
            ..CompilerLimits::default()
        };
        let preflight = KnownPreflight::inspect(&validated, limits).unwrap();
        preflight
            .projected_peak_bytes(validated.retained_charged_bytes(), validated.requirements().len())
            .unwrap()
    });
    with_validated(text, |validated| {
        let mut provider = CountingProvider {
            bytes: b"phrase".to_vec(),
            calls: 0,
        };
        CompilerBuilder::new(
            validated,
            Some(&mut provider),
            CompilerLimits {
                max_template_runtime_bytes: 10,
                max_rule_runtime_expansion_bytes: 20,
                ..CompilerLimits::default()
            },
        )
        .prepare()
        .unwrap();
        assert_eq!(provider.calls, 1);
    });
    with_validated(text, |validated| {
        let mut provider = CountingProvider {
            bytes: b"phrase".to_vec(),
            calls: 0,
        };
        let error = CompilerBuilder::new(
            validated,
            Some(&mut provider),
            CompilerLimits {
                max_template_runtime_bytes: 10,
                max_rule_runtime_expansion_bytes: 19,
                ..CompilerLimits::default()
            },
        )
        .prepare()
        .unwrap_err();
        assert!(matches!(
            error,
            CompilerError::MemberRuntimeByteLimit {
                actual: 20,
                limit: 19,
                ..
            }
        ));
        assert_eq!(provider.calls, 0);
    });
    with_validated(text, |validated| {
        let mut provider = CountingProvider {
            bytes: b"phrase".to_vec(),
            calls: 0,
        };
        let error = CompilerBuilder::new(
            validated,
            Some(&mut provider),
            CompilerLimits {
                max_template_runtime_bytes: usize::MAX,
                max_rule_runtime_expansion_bytes: usize::MAX,
                ..CompilerLimits::default()
            },
        )
        .prepare()
        .unwrap_err();
        assert!(matches!(error, CompilerError::MemberRuntimeByteLimit { .. }));
        assert_eq!(provider.calls, 0);
    });
    with_validated(text, |validated| {
        let mut provider = CountingProvider {
            bytes: b"phrase".to_vec(),
            calls: 0,
        };
        COMPILER_ALLOCATION_FAILURE.with(|failure| failure.set(Some("member template identities")));
        let result = CompilerBuilder::new(
            validated,
            Some(&mut provider),
            CompilerLimits {
                max_template_runtime_bytes: 10,
                max_rule_runtime_expansion_bytes: 20,
                max_compilation_peak_bytes: exact_preflight_peak - 1,
                ..CompilerLimits::default()
            },
        )
        .prepare();
        COMPILER_ALLOCATION_FAILURE.with(|failure| failure.set(None));
        assert!(matches!(result, Err(CompilerError::PeakByteLimit { .. })));
        assert_eq!(provider.calls, 0);
    });
    with_validated(text, |validated| {
        let mut provider = CountingProvider {
            bytes: b"phrase".to_vec(),
            calls: 0,
        };
        COMPILER_ALLOCATION_FAILURE.with(|failure| failure.set(Some("member template identities")));
        let result = CompilerBuilder::new(
            validated,
            Some(&mut provider),
            CompilerLimits {
                max_template_runtime_bytes: 10,
                max_rule_runtime_expansion_bytes: 20,
                max_compilation_peak_bytes: exact_preflight_peak,
                ..CompilerLimits::default()
            },
        )
        .prepare();
        COMPILER_ALLOCATION_FAILURE.with(|failure| failure.set(None));
        assert_eq!(
            result.unwrap_err(),
            CompilerError::Allocation {
                operation: "member template identities"
            }
        );
        assert_eq!(provider.calls, 0);
    });
}

#[test]
fn removed_units_discharge_owned_obligations_but_remain_inactive() {
    let text = concat!(
        "SecRule ARGS \"@streq %{TX.key}\" \"id:1,phase:2,pass,setvar:'tx.name=%{TX.value}'\"\n",
        "SecRuleRemoveById 1\n",
    );
    with_validated(text, |validated| {
        let state = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        assert_eq!(state.members.len(), 1);
        assert!(!state.members[0].active);
        assert!(state.requirements.iter().all(|requirement| {
            !matches!(
                requirement.kind,
                CompileRequirement::RuntimeMacro | CompileRequirement::TransactionSetVar
            ) || requirement.discharged
        }));
    });
}

#[test]
fn compiled_template_bytes_outlive_source_owner_after_candidate_drop() {
    let source = scan_source(
        "ownership-test",
        b"SecRule ARGS \"@streq owned-value\" \"id:1,phase:2,pass\"\n",
        SourceLimits::default(),
    )
    .unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).unwrap();
    let validated = validate_semantic_draft(draft, context(), ValidationLimits::default()).unwrap();
    let state = CompilerBuilder::new(validated, None, CompilerLimits::default())
        .prepare()
        .unwrap();
    let AssetTemplateState {
        validated,
        bytes,
        templates,
        ..
    } = state;
    drop(validated);
    drop(source);
    assert!(!templates.is_empty());
    assert!(bytes.windows(b"owned-value".len()).any(|bytes| bytes == b"owned-value"));
}

#[test]
fn compiler_errors_are_content_free_through_the_source_chain() {
    let text = "SecRule ARGS \"@pmFromFile private-handle.data\" \"id:1,phase:2,pass\"\n";
    with_validated(text, |validated| {
        let error = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap_err();
        let mut rendered = format!("{error:?} {error}");
        let mut source = std::error::Error::source(&error);
        while let Some(error) = source {
            rendered.push_str(&format!(" {error:?} {error}"));
            source = error.source();
        }
        assert!(!rendered.contains("private-handle"));
    });
}

#[test]
fn matcher_stage_compiles_the_fifteen_non_phrase_operator_plans() {
    let text = concat!(
        "SecRule ARGS \"@beginsWith a\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@contains b\" \"id:2,phase:2,pass\"\n",
        "SecRule ARGS \"@endsWith c\" \"id:3,phase:2,pass\"\n",
        "SecRule ARGS \"@streq d\" \"id:4,phase:2,pass\"\n",
        "SecRule ARGS \"@within e\" \"id:5,phase:2,pass\"\n",
        "SecRule ARGS \"@eq 1\" \"id:6,phase:2,pass\"\n",
        "SecRule ARGS \"@ge %{TX.floor}\" \"id:7,phase:2,pass\"\n",
        "SecRule ARGS \"@gt 2\" \"id:8,phase:2,pass\"\n",
        "SecRule ARGS \"@lt 3\" \"id:9,phase:2,pass\"\n",
        "SecRule ARGS \"@ipMatch 127.0.0.1,2001:db8::/32\" \"id:10,phase:2,pass\"\n",
        "SecRule ARGS \"@validateByteRange 9,32-126\" \"id:11,phase:2,pass\"\n",
        "SecRule ARGS \"@validateUrlEncoding\" \"id:12,phase:2,pass\"\n",
        "SecRule ARGS \"@validateUtf8Encoding\" \"id:13,phase:2,pass\"\n",
        "SecRule ARGS \"@unconditionalMatch\" \"id:14,phase:2,pass\"\n",
        "SecRule ARGS \"!@rx ^a(?P<tail>b*)$\" \"id:15,phase:2,pass,capture\"\n",
    );
    with_validated(text, |validated| {
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let state = MatcherState::compile(prepared, CompilerLimits::default()).unwrap();
        assert_eq!(state.operator_plans.iter().flatten().count(), 15);
        assert_eq!(state.ip_networks.len(), 2);
        assert!(state.boolean_regexes.is_empty());
        assert_eq!(state.capture_regexes.len(), 1);
        assert!(state
            .asset_templates
            .requirements
            .iter()
            .all(|requirement| requirement.discharged));
        for (index, relation) in [
            StringRelation::BeginsWith,
            StringRelation::Contains,
            StringRelation::EndsWith,
            StringRelation::Equals,
            StringRelation::Within,
        ]
        .into_iter()
        .enumerate()
        {
            assert!(matches!(
                state.operator_plans[index].unwrap().operand,
                CompiledOperatorOperand::String {
                    relation: actual,
                    template: _,
                } if actual == relation
            ));
        }
        for (index, relation, expected_operand) in [
            (5, IntegerRelation::Equal, CompiledIntegerOperand::Static(1)),
            (
                6,
                IntegerRelation::GreaterOrEqual,
                CompiledIntegerOperand::Template(usize::MAX),
            ),
            (7, IntegerRelation::Greater, CompiledIntegerOperand::Static(2)),
            (8, IntegerRelation::Less, CompiledIntegerOperand::Static(3)),
        ] {
            assert!(matches!(
                state.operator_plans[index].unwrap().operand,
                CompiledOperatorOperand::Integer { relation: actual, operand }
                    if actual == relation
                        && (expected_operand == operand
                            || matches!(
                                (expected_operand, operand),
                                (CompiledIntegerOperand::Template(_), CompiledIntegerOperand::Template(_))
                            ))
            ));
        }
        assert!(matches!(
            state.operator_plans[9].unwrap().operand,
            CompiledOperatorOperand::IpNetworks { start: 0, count: 2 }
        ));
        let CompiledOperatorOperand::ByteSet { words } = state.operator_plans[10].unwrap().operand else {
            panic!("expected byte-set plan");
        };
        for byte in [9_usize, 32, 126] {
            assert_ne!(words[byte / 64] & (1_u64 << (byte % 64)), 0);
        }
        for byte in [8_usize, 31, 127] {
            assert_eq!(words[byte / 64] & (1_u64 << (byte % 64)), 0);
        }
        assert!(matches!(
            state.operator_plans[11].unwrap().operand,
            CompiledOperatorOperand::ValidateUrlEncoding
        ));
        assert!(matches!(
            state.operator_plans[12].unwrap().operand,
            CompiledOperatorOperand::ValidateUtf8Encoding
        ));
        assert!(matches!(
            state.operator_plans[13].unwrap().operand,
            CompiledOperatorOperand::Unconditional
        ));
        let regex = state.operator_plans[14].unwrap();
        assert!(regex.negated);
        assert!(matches!(
            regex.operand,
            CompiledOperatorOperand::Regex(RegexPlanRef::Capture(0))
        ));
    });
}

#[test]
fn matcher_stage_drops_removed_regex_program_after_discharging_it() {
    let text = concat!(
        "SecRule ARGS \"@rx private-pattern\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@rx (capture-pattern)\" \"id:2,phase:2,pass,capture\"\n",
        "SecRuleRemoveById 1\n",
        "SecRuleRemoveById 2\n",
    );
    with_validated(text, |validated| {
        let limits = CompilerLimits {
            max_regex_programs: 0,
            max_compiled_matcher_bytes: 0,
            ..CompilerLimits::default()
        };
        let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
        let state = MatcherState::compile(prepared, limits).unwrap();
        assert_eq!(state.operator_plans, [None, None]);
        assert!(state.boolean_regexes.is_empty());
        assert!(state.capture_regexes.is_empty());
        assert_eq!(state.matcher_charged_bytes, 0);
        assert!(state
            .asset_templates
            .requirements
            .iter()
            .all(|requirement| requirement.discharged));
    });
}

#[test]
fn grouped_matcher_regex_errors_and_count_limits_are_content_free() {
    let text = "SecRule ARGS \"@rx private-[\" \"id:1,phase:2,pass\"\n";
    with_validated(text, |validated| {
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let matched = MatcherState::compile(prepared, CompilerLimits::default()).unwrap();
        let error = GroupedMatcherState::compile(matched, CompilerLimits::default()).unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert!(matches!(
            error,
            CompilerError::PhraseMatcher {
                kind: PhraseMatcherErrorKind::Build,
                ..
            }
        ));
        assert!(!rendered.contains("private"));
    });
    with_validated("SecRule ARGS \"@rx a\" \"id:1,phase:2,pass\"\n", |validated| {
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let limits = CompilerLimits {
            max_regex_programs: 0,
            ..CompilerLimits::default()
        };
        let matched = MatcherState::compile(prepared, limits).unwrap();
        assert!(matches!(
            GroupedMatcherState::compile(matched, limits),
            Err(CompilerError::RegexProgramLimit { actual: 1, limit: 0 })
        ));
    });
}

#[test]
fn matcher_stage_compiles_active_target_selector_identity_once() {
    let text = "SecRule ARGS:/^name$/ \"@eq 1\" \"id:1,phase:2,pass\"\n";
    with_validated(text, |validated| {
        let target_location = validated
            .active_rule_units()
            .next()
            .unwrap()
            .root()
            .final_targets()
            .next()
            .unwrap()
            .location();
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let state = MatcherState::compile(prepared, CompilerLimits::default()).unwrap();
        assert_eq!(state.target_regexes.len(), 1);
        assert_eq!(state.target_regexes[0].identity, target_location);
        assert_eq!(state.target_regexes[0].program, 0);
        assert_eq!(state.boolean_regexes.len(), 1);
        assert!(state.asset_templates.requirements.iter().all(|requirement| {
            requirement.kind == CompileRequirement::PhraseMatcherCompilation || requirement.discharged
        }));
    });
}

#[test]
fn selector_program_sharing_preserves_profiles_occurrences_and_publication() {
    let text = concat!(
        "SecRule ARGS:/^NAME$/ \"@rx a\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS|!ARGS:/^NAME$/ \"@rx b\" \"id:2,phase:2,pass\"\n",
        "SecRule &ARGS:/^NAME$/ \"@eq 1\" \"id:3,phase:2,pass\"\n",
        "SecRule TX:/^NAME$/ \"@streq value\" \"id:4,phase:2,pass\"\n",
        "SecRule ARGS:/^(?i:NAME)$/ \"@streq value\" \"id:5,phase:2,pass\"\n",
    );
    with_validated(text, |validated| {
        let limits = CompilerLimits::default();
        let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
        let state = MatcherState::compile(prepared, limits).unwrap();
        assert_eq!(state.target_regexes.len(), 5);
        assert_eq!(state.boolean_regexes.len(), 3);
        assert_eq!(
            state
                .target_regexes
                .iter()
                .map(|target| target.program)
                .collect::<Vec<_>>(),
            [0, 0, 0, 1, 2]
        );
        assert!(state
            .asset_templates
            .requirements
            .iter()
            .all(|requirement| { requirement.kind == CompileRequirement::RegexCompilation || requirement.discharged }));
        for (id, expected) in [(0, false), (1, true), (2, true)] {
            let program = &state.boolean_regexes[id];
            assert_eq!(
                program.is_match(b"name", &mut program.create_scratch()).unwrap(),
                expected
            );
        }
        let grouped = GroupedMatcherState::compile(state, limits).unwrap();
        assert_eq!(grouped.matchers.boolean_regexes.len(), 3);
        let generation = CompiledGeneration::publish(grouped, limits).unwrap();
        assert_eq!(generation.rule_unit_count(), 5);
    });
    with_validated(text, |validated| {
        let limits = CompilerLimits {
            max_regex_programs: 4,
            ..CompilerLimits::default()
        };
        let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
        assert!(matches!(
            MatcherState::compile(prepared, limits),
            Err(CompilerError::RegexProgramLimit { actual: 5, limit: 4 })
        ));
    });
}

#[test]
fn selector_interner_charges_work_and_has_exact_peak_admission() {
    let text = concat!(
        "SecRule ARGS:/^name$/ \"@streq value\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS:/^name$/ \"@streq value\" \"id:2,phase:2,pass\"\n",
    );
    for (limits, expected) in [
        (
            CompilerLimits {
                max_group_key_hash_bytes: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupKeyHashByteLimit { limit: 0 },
        ),
        (
            CompilerLimits {
                max_group_table_probes: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupTableProbeLimit { limit: 0 },
        ),
        (
            CompilerLimits {
                max_group_key_record_comparisons: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupKeyRecordComparisonLimit { limit: 0 },
        ),
        (
            CompilerLimits {
                max_group_key_comparison_bytes: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupKeyComparisonByteLimit { limit: 0 },
        ),
    ] {
        with_validated(text, |validated| {
            let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
            assert_eq!(MatcherState::compile(prepared, limits).unwrap_err(), expected);
        });
    }
    let peak = with_validated(text, |validated| {
        let limits = CompilerLimits::default();
        let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
        MatcherState::compile(prepared, limits).unwrap().peak_bytes
    });
    for (bound, succeeds) in [(peak, true), (peak - 1, false)] {
        with_validated(text, |validated| {
            let limits = CompilerLimits {
                max_compilation_peak_bytes: bound,
                ..CompilerLimits::default()
            };
            let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
            let result = MatcherState::compile(prepared, limits);
            assert_eq!(result.is_ok(), succeeds, "{result:?}");
            if !succeeds {
                assert!(matches!(result, Err(CompilerError::PeakByteLimit { .. })));
            }
        });
    }
}

#[test]
fn matcher_stage_retained_generation_and_peak_limits_are_exact() {
    let text = "SecRule ARGS \"@rx ^(?P<value>a+)$\" \"id:1,phase:2,pass,capture\"\n";
    let (matcher_bytes, generation_bytes, peak_bytes, program_bytes) = with_validated(text, |validated| {
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let state = MatcherState::compile(prepared, CompilerLimits::default()).unwrap();
        (
            state.matcher_charged_bytes,
            state.compiler_owned_bytes + state.matcher_charged_bytes,
            state.peak_bytes,
            state.capture_regexes[0].conservative_program_memory_bound(),
        )
    });
    for (case, (matcher_limit, generation_limit, peak_limit, program_limit, succeeds)) in [
        (
            matcher_bytes,
            generation_bytes,
            peak_bytes,
            RegexProgramLimits::default().max_program_bytes,
            true,
        ),
        (
            matcher_bytes - 1,
            generation_bytes,
            peak_bytes,
            RegexProgramLimits::default().max_program_bytes,
            false,
        ),
        (
            matcher_bytes,
            generation_bytes - 1,
            peak_bytes,
            RegexProgramLimits::default().max_program_bytes,
            false,
        ),
        (
            matcher_bytes,
            generation_bytes,
            peak_bytes - 1,
            RegexProgramLimits::default().max_program_bytes,
            false,
        ),
        (matcher_bytes, generation_bytes, usize::MAX, program_bytes, true),
        (matcher_bytes, generation_bytes, usize::MAX, program_bytes - 1, false),
    ]
    .into_iter()
    .enumerate()
    {
        with_validated(text, |validated| {
            let limits = CompilerLimits {
                regex: RegexProgramLimits {
                    max_program_bytes: program_limit,
                    ..RegexProgramLimits::default()
                },
                max_compiled_matcher_bytes: matcher_limit,
                max_generation_bytes: generation_limit,
                max_compilation_peak_bytes: peak_limit,
                ..CompilerLimits::default()
            };
            let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
            let result = MatcherState::compile(prepared, limits);
            assert_eq!(result.is_ok(), succeeds, "case {case}: {result:?}");
        });
    }
}

#[test]
fn grouping_stage_canonicalizes_complete_target_and_transformation_plans() {
    let text = concat!(
        "SecRule ARGS:key \"@eq 1\" \"id:1,phase:2,pass,t:lowercase\"\n",
        "SecRule ARGS:key \"@eq 2\" \"id:2,phase:2,pass,t:lowercase\"\n",
        "SecRule ARGS:other \"@eq 3\" \"id:3,phase:2,pass,t:length\"\n",
        "SecRule ARGS:/^name$/ \"@eq 4\" \"id:4,phase:2,pass,t:lowercase\"\n",
    );
    with_validated(text, |validated| {
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let matched = MatcherState::compile(prepared, CompilerLimits::default()).unwrap();
        let grouped = GroupedMatcherState::compile(matched, CompilerLimits::default()).unwrap();

        assert_eq!(grouped.target_plans.len(), 3);
        assert_eq!(grouped.transformation_plans.len(), 2);
        assert_eq!(grouped.target_records.len(), 3);
        assert_eq!(grouped.canonical_transformations.len(), 2);
        assert_eq!(grouped.target_bytes, b"keyother");
        let members = &grouped.matchers.asset_templates.members;
        assert_eq!(members[0].target_plan, members[1].target_plan);
        assert_ne!(members[1].target_plan, members[2].target_plan);
        assert_ne!(members[2].target_plan, members[3].target_plan);
        assert_eq!(members[0].transformation_plan, members[1].transformation_plan);
        assert_eq!(members[0].transformation_plan, members[3].transformation_plan);
        assert_ne!(members[1].transformation_plan, members[2].transformation_plan);
        assert!(matches!(
            grouped.target_records[2].selector,
            CompiledTargetSelector::Regex {
                style: RegexStyle::Bare,
                program: 0
            }
        ));
    });
}

#[test]
fn grouping_stage_enforces_canonical_work_limits() {
    let text = concat!(
        "SecRule ARGS:key \"@eq 1\" \"id:1,phase:2,pass,t:lowercase\"\n",
        "SecRule ARGS:key \"@eq 2\" \"id:2,phase:2,pass,t:lowercase\"\n",
    );
    for (limits, expected) in [
        (
            CompilerLimits {
                max_group_key_hash_bytes: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupKeyHashByteLimit { limit: 0 },
        ),
        (
            CompilerLimits {
                max_group_table_probes: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupTableProbeLimit { limit: 0 },
        ),
        (
            CompilerLimits {
                max_group_key_record_comparisons: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupKeyRecordComparisonLimit { limit: 0 },
        ),
        (
            CompilerLimits {
                max_group_key_comparison_bytes: 0,
                ..CompilerLimits::default()
            },
            CompilerError::GroupKeyComparisonByteLimit { limit: 0 },
        ),
    ] {
        with_validated(text, |validated| {
            let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
            let matched = MatcherState::compile(prepared, limits).unwrap();
            assert_eq!(GroupedMatcherState::compile(matched, limits).unwrap_err(), expected);
        });
    }
}

#[test]
fn grouping_stage_builds_phrase_regex_and_capture_phrase_programs() {
    let text = concat!(
        "SecRule ARGS \"@pm alpha beta\" \"id:1,phase:2,pass,t:lowercase\"\n",
        "SecRule ARGS \"@rx gamma\" \"id:2,phase:2,pass,t:lowercase\"\n",
        "SecRule ARGS \"@pm ALPHA alpha\" \"id:3,phase:2,pass,t:lowercase,capture\"\n",
        "SecRule REQUEST_HEADERS \"@pm delta\" \"id:4,phase:2,pass,t:lowercase\"\n",
    );
    with_validated(text, |validated| {
        let prepared = CompilerBuilder::new(validated, None, CompilerLimits::default())
            .prepare()
            .unwrap();
        let matched = MatcherState::compile(prepared, CompilerLimits::default()).unwrap();
        let grouped = GroupedMatcherState::compile(matched, CompilerLimits::default()).unwrap();

        assert_eq!(grouped.groups.len(), 2);
        assert_eq!(grouped.pattern_mappings.len(), 4);
        assert_eq!(grouped.capture_phrases.len(), 1);
        assert_eq!(grouped.groups[0].mapping_count, 3);
        assert_eq!(grouped.groups[1].mapping_count, 1);
        assert_eq!(grouped.groups[0].key.capture_mode, CaptureMode::None);
        assert_eq!(grouped.groups[0].key.byte_view, ByteView::RawBytes);

        let mut scratch = grouped.groups[0].matcher.create_scratch();
        let ids: Vec<_> = grouped.groups[0]
            .matcher
            .scan(b"BETA gamma", &mut scratch)
            .unwrap()
            .iter()
            .map(|id| id.as_usize())
            .collect();
        assert_eq!(ids, [1, 2]);
        let maps = &grouped.pattern_mappings
            [grouped.groups[0].mapping_start..grouped.groups[0].mapping_start + grouped.groups[0].mapping_count];
        assert_eq!(maps[0].local_ordinal, 0);
        assert_eq!(maps[1].local_ordinal, 1);
        assert_eq!(maps[2].member, 1);

        let captured = grouped.capture_phrases[0].find(b"xxAlPhA").unwrap();
        assert_eq!(captured.pattern_id(), 0);
        assert_eq!((captured.start(), captured.end()), (2, 7));
        assert_eq!(grouped.capture_phrases[0].pattern_count(), 2);
        assert!(matches!(
            grouped.matchers.operator_plans[0].unwrap().operand,
            CompiledOperatorOperand::Matcher {
                group: 0,
                pattern_start: 0,
                pattern_count: 2
            }
        ));
        assert!(matches!(
            grouped.matchers.operator_plans[1].unwrap().operand,
            CompiledOperatorOperand::Matcher {
                group: 0,
                pattern_start: 2,
                pattern_count: 1
            }
        ));
        assert!(matches!(
            grouped.matchers.operator_plans[2].unwrap().operand,
            CompiledOperatorOperand::CapturePhrase(0)
        ));
        assert_eq!(grouped.matchers.boolean_regexes.len(), 0);
        assert!(grouped.compiler_owned_bytes > 0);
        assert!(grouped.matcher_charged_bytes > 0);
        assert!(grouped.peak_bytes >= grouped.compiler_owned_bytes + grouped.matcher_charged_bytes);
    });
}

#[test]
fn grouping_stage_enforces_cumulative_pattern_group_and_source_limits() {
    let text = concat!(
        "SecRule ARGS \"@pm alpha beta\" \"id:1,phase:2,pass\"\n",
        "SecRule REQUEST_HEADERS \"@pm gamma\" \"id:2,phase:2,pass\"\n",
    );
    for (limits, expected) in [
        (
            CompilerLimits {
                max_matcher_patterns: 2,
                ..CompilerLimits::default()
            },
            CompilerError::MatcherPatternLimit { actual: 3, limit: 2 },
        ),
        (
            CompilerLimits {
                max_matcher_source_bytes: 13,
                ..CompilerLimits::default()
            },
            CompilerError::MatcherSourceByteLimit { actual: 14, limit: 13 },
        ),
        (
            CompilerLimits {
                max_matcher_groups: 1,
                ..CompilerLimits::default()
            },
            CompilerError::MatcherGroupLimit { actual: 2, limit: 1 },
        ),
    ] {
        with_validated(text, |validated| {
            let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
            let matched = MatcherState::compile(prepared, limits).unwrap();
            assert_eq!(GroupedMatcherState::compile(matched, limits).unwrap_err(), expected);
        });
    }
}

#[test]
fn grouping_stage_compiles_normalized_external_phrases_in_file_order() {
    let text = "SecRule ARGS \"@pmFromFile phrases.data\" \"id:1,phase:2,pass\"\n";
    with_validated(text, |validated| {
        let mut provider = AssetProvider {
            chunks: vec![b" first \r\n# ignored\r\nsecond\r\nfirst\r\n"],
            failure: None,
            ignore_sink_errors: false,
        };
        let prepared = CompilerBuilder::new(validated, Some(&mut provider), CompilerLimits::default())
            .prepare()
            .unwrap();
        let matched = MatcherState::compile(prepared, CompilerLimits::default()).unwrap();
        let grouped = GroupedMatcherState::compile(matched, CompilerLimits::default()).unwrap();
        assert_eq!(grouped.groups.len(), 1);
        assert_eq!(grouped.groups[0].mapping_count, 3);
        assert_eq!(
            grouped.pattern_mappings[grouped.groups[0].mapping_start..grouped.groups[0].mapping_start + 3]
                .iter()
                .map(|mapping| mapping.local_ordinal)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        let mut scratch = grouped.groups[0].matcher.create_scratch();
        assert_eq!(
            grouped.groups[0]
                .matcher
                .scan(b"FIRST second", &mut scratch)
                .unwrap()
                .iter()
                .map(|id| id.as_usize())
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
    });
}

#[test]
fn group_table_collision_uses_full_key_equality_and_stable_first_ids() {
    let base = CompiledMatcherGroupKey {
        phase: Phase::RequestBody,
        target_plan: 1,
        transformation_plan: 2,
        multi_match: false,
        capture_mode: CaptureMode::None,
        byte_view: ByteView::RawBytes,
    };
    let mutations = [
        CompiledMatcherGroupKey {
            phase: Phase::RequestHeaders,
            ..base
        },
        CompiledMatcherGroupKey { target_plan: 9, ..base },
        CompiledMatcherGroupKey {
            transformation_plan: 9,
            ..base
        },
        CompiledMatcherGroupKey {
            multi_match: true,
            ..base
        },
        CompiledMatcherGroupKey {
            capture_mode: CaptureMode::RegexGroups,
            ..base
        },
        CompiledMatcherGroupKey {
            capture_mode: CaptureMode::PhraseSpans,
            ..base
        },
    ];
    let mut slots = vec![CanonicalSlot::EMPTY; 32];
    let mut keys = Vec::new();
    let mut work = CanonicalWork::default();
    let limits = CompilerLimits::default();
    let forced_collision = 7;
    assert_eq!(
        insert_group_key(base, forced_collision, &mut slots, &mut keys, &mut work, limits).unwrap(),
        0
    );
    for (index, key) in mutations.into_iter().enumerate() {
        assert_eq!(
            insert_group_key(key, forced_collision, &mut slots, &mut keys, &mut work, limits).unwrap(),
            index + 1
        );
    }
    assert_eq!(
        insert_group_key(base, forced_collision, &mut slots, &mut keys, &mut work, limits).unwrap(),
        0
    );
    assert_eq!(keys.len(), 7);
    assert!(work.probes > keys.len());
    assert!(work.record_comparisons > 0);
}

#[test]
fn public_generation_builds_five_marker_inclusive_phase_schedules() {
    let text = concat!(
        "SecRule REQUEST_HEADERS \"@pm one\" \"id:1,phase:1,pass,skipAfter:END\"\n",
        "SecMarker MID\n",
        "SecRule ARGS \"@eq 2\" \"id:2,phase:2,pass\"\n",
        "SecMarker END\n",
        "SecRule REQUEST_HEADERS \"@rx three\" \"id:3,phase:1,pass\"\n",
    );
    let generation = with_validated(text, |validated| {
        compile_validated(validated, None, CompilerLimits::default()).unwrap()
    });
    assert_eq!(generation.rule_unit_count(), 3);
    assert_eq!(generation.marker_count(), 2);
    assert_eq!(generation.matcher_group_count(), 1);
    let phase_one: Vec<_> = generation
        .phase_schedule(Phase::RequestHeaders)
        .map(|entry| (entry.kind(), entry.ordinal()))
        .collect();
    assert_eq!(
        phase_one,
        [
            (CompiledScheduleEntryKind::RuleUnit, 0),
            (CompiledScheduleEntryKind::MarkerSentinel, 0),
            (CompiledScheduleEntryKind::MarkerSentinel, 1),
            (CompiledScheduleEntryKind::RuleUnit, 2),
        ]
    );
    let phase_two: Vec<_> = generation
        .phase_schedule(Phase::RequestBody)
        .map(|entry| (entry.kind(), entry.ordinal()))
        .collect();
    assert_eq!(
        phase_two,
        [
            (CompiledScheduleEntryKind::MarkerSentinel, 0),
            (CompiledScheduleEntryKind::RuleUnit, 1),
            (CompiledScheduleEntryKind::MarkerSentinel, 1),
        ]
    );
    for phase in [Phase::ResponseHeaders, Phase::ResponseBody, Phase::Logging] {
        assert_eq!(
            generation
                .phase_schedule(phase)
                .map(|entry| entry.kind())
                .collect::<Vec<_>>(),
            [
                CompiledScheduleEntryKind::MarkerSentinel,
                CompiledScheduleEntryKind::MarkerSentinel,
            ]
        );
    }
    let units: Vec<_> = generation.rule_units().collect();
    assert_eq!(units.iter().map(|unit| unit.id()).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(units[0].schedule_index(), 0);
    assert_eq!(units[1].schedule_index(), 1);
    assert_eq!(units[2].schedule_index(), 3);
    let first_member = units[0].members().next().unwrap();
    assert_eq!(first_member.skip_after_schedule_index(), Some(2));
    assert_eq!(first_member.transformations(), Some([].as_slice()));
    let target = first_member.target_plan().unwrap().records().next().unwrap();
    assert_eq!(target.collection(), CollectionKind::RequestHeaders);
    assert_eq!(target.modifier(), TargetModifier::None);
    assert_eq!(target.selector_kind(), CompiledTargetSelectorKind::None);
    assert_eq!(target.selector_bytes(), None);
    assert_eq!(target.selector_regex_program_id(), None);
    assert!(generation.compilation_peak_bytes() >= generation.retained_bytes());
}

#[test]
fn public_generation_keeps_chains_single_and_removed_units_invisible() {
    let text = concat!(
        "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass,chain\"\n",
        "SecRule REQUEST_HEADERS \"@pm child\" \"t:lowercase\"\n",
        "SecRule ARGS \"@pm removed\" \"id:2,phase:2,pass\"\n",
        "SecRuleRemoveById 2\n",
        "SecMarker END\n",
    );
    let generation = with_validated(text, |validated| {
        compile_validated(validated, None, CompilerLimits::default()).unwrap()
    });
    assert_eq!(generation.rule_unit_count(), 1);
    let unit = generation.rule_units().next().unwrap();
    assert_eq!(unit.id(), 1);
    assert_eq!(unit.members().len(), 2);
    assert_eq!(
        generation
            .phase_schedule(Phase::RequestBody)
            .map(|entry| entry.kind())
            .collect::<Vec<_>>(),
        [
            CompiledScheduleEntryKind::RuleUnit,
            CompiledScheduleEntryKind::MarkerSentinel,
        ]
    );
}

#[test]
fn public_generation_classifies_phase_two_execution_conservatively() {
    let text = concat!(
        "SecRule REQUEST_BODY \"@pm alpha beta\" \"id:1,phase:2,pass,t:lowercase\"\n",
        "SecRule ARGS \"@rx gamma\" \"id:2,phase:2,pass\"\n",
        "SecRule REQUEST_BODY \"@rx delta\" \"id:3,phase:2,pass\"\n",
        "SecRule REQUEST_HEADERS \"@contains example\" \"id:4,phase:2,pass\"\n",
        "SecRule REQUEST_URI \"@contains /early\" \"id:5,phase:1,pass\"\n",
        "SecRule REQBODY_PROCESSOR \"@streq JSON\" \"id:6,phase:2,pass\"\n",
    );
    let generation = with_validated(text, |validated| {
        compile_validated(validated, None, CompilerLimits::default()).unwrap()
    });
    let classes = generation
        .rule_units()
        .map(|unit| unit.request_body_execution_class())
        .collect::<Vec<_>>();
    assert_eq!(
        classes,
        [
            Some(RequestBodyExecutionClass::StreamingRaw),
            Some(RequestBodyExecutionClass::BoundedSnapshot),
            Some(RequestBodyExecutionClass::BoundedSnapshot),
            Some(RequestBodyExecutionClass::StreamingRaw),
            None,
            Some(RequestBodyExecutionClass::BoundedSnapshot),
        ]
    );
    let summary = generation.request_body_execution_summary();
    assert_eq!(summary.streaming_raw_rule_units(), 2);
    assert_eq!(summary.streaming_structured_rule_units(), 0);
    assert_eq!(summary.bounded_snapshot_rule_units(), 3);
    assert_eq!(summary.unsupported_rule_units(), 0);
    assert!(summary.requires_request_body());
}

#[test]
fn public_generation_schedule_limits_accept_exact_and_reject_first_excess() {
    let text = concat!(
        "SecRule REQUEST_HEADERS \"@eq 1\" \"id:1,phase:1,pass\"\n",
        "SecMarker END\n",
    );
    with_validated(text, |validated| {
        compile_validated(
            validated,
            None,
            CompilerLimits {
                max_schedule_entries: 6,
                max_schedule_entries_per_phase: 2,
                ..CompilerLimits::default()
            },
        )
        .unwrap();
    });
    with_validated(text, |validated| {
        assert_eq!(
            compile_validated(
                validated,
                None,
                CompilerLimits {
                    max_schedule_entries: 5,
                    ..CompilerLimits::default()
                },
            )
            .unwrap_err(),
            CompilerError::ScheduleEntryLimit { actual: 6, limit: 5 }
        );
    });
    with_validated(text, |validated| {
        assert_eq!(
            compile_validated(
                validated,
                None,
                CompilerLimits {
                    max_schedule_entries_per_phase: 1,
                    ..CompilerLimits::default()
                },
            )
            .unwrap_err(),
            CompilerError::PhaseScheduleEntryLimit {
                phase: 1,
                actual: 2,
                limit: 1,
            }
        );
    });
}

#[test]
fn public_generation_final_retained_and_peak_limits_are_exact() {
    let text = concat!(
        "SecRule ARGS:key \"@pm alpha beta\" \"id:1,phase:2,pass,t:lowercase\"\n",
        "SecMarker END\n",
    );
    let baseline = with_validated(text, |validated| {
        compile_validated(validated, None, CompilerLimits::default()).unwrap()
    });
    let compiler = baseline.compiler_owned_bytes();
    let matcher = baseline.matcher_charged_bytes();
    let retained = baseline.retained_bytes();
    let peak = baseline.compilation_peak_bytes();
    assert_eq!(retained, compiler + matcher);
    for (case, (compiler_limit, matcher_limit, generation_limit, peak_limit, succeeds)) in [
        (compiler, matcher, retained, peak, true),
        (compiler - 1, matcher, retained, peak, false),
        (compiler, matcher - 1, retained, peak, false),
        (compiler, matcher, retained - 1, peak, false),
        (compiler, matcher, retained, peak - 1, false),
    ]
    .into_iter()
    .enumerate()
    {
        with_validated(text, |validated| {
            let result = compile_validated(
                validated,
                None,
                CompilerLimits {
                    max_compiler_owned_non_matcher_bytes: compiler_limit,
                    max_compiled_matcher_bytes: matcher_limit,
                    max_generation_bytes: generation_limit,
                    max_compilation_peak_bytes: peak_limit,
                    ..CompilerLimits::default()
                },
            );
            assert_eq!(result.is_ok(), succeeds, "case {case}: {result:?}");
        });
    }
}

#[test]
fn structured_phase_two_rules_require_complete_publication() {
    for (target, operator, actions) in [
        ("ARGS", "@pm attack", ""),
        ("ARGS:json.key", "@rx attack", ",capture"),
        ("ARGS:/^json/", "!@contains attack", ""),
        ("&ARGS", "@eq 0", ""),
        ("ARGS|!ARGS:json.safe", "@pm attack", ""),
        ("ARGS_NAMES", "@contains key", ",t:lowercase"),
        ("ARGS_COMBINED_SIZE", "@gt 1", ""),
        ("FILES", "@contains file", ""),
        ("FILES_NAMES", "@contains file", ""),
        ("FILES_COMBINED_SIZE", "@gt 1", ""),
        ("MULTIPART_PART_HEADERS", "@contains file", ""),
        ("XML:/*", "@contains attack", ""),
        ("XML://@*", "@contains attack", ""),
        ("ARGS", "@pm attack", ",t:urlDecodeUni"),
        ("ARGS", "@pm attack", ",setvar:tx.score=+1"),
        ("ARGS", "@pm attack", ",chain"),
    ] {
        let mut source = format!("SecRule {target} \"{operator}\" \"id:1,phase:2,pass{actions}\"\n");
        if actions == ",chain" {
            source.push_str("SecRule REQUEST_HEADERS \"@contains next\" \"t:none\"\n");
        }
        with_validated(&source, |validated| {
            let generation = compile_validated(validated, None, CompilerLimits::default()).unwrap();
            assert_eq!(
                generation.rule_units().next().unwrap().request_body_execution_class(),
                Some(RequestBodyExecutionClass::BoundedSnapshot),
                "{source}"
            );
        });
    }
}

#[test]
fn body_macros_require_snapshot_even_without_body_targets() {
    for source in [
        "SecRule REQUEST_URI \"@contains /\" \"id:1,phase:2,pass,chain\"\nSecRule REQUEST_HEADERS \"@streq %{ARGS.json.key}\" \"t:none\"\n",
        "SecAction \"id:1,phase:2,pass,setvar:tx.%{ARGS.json.key}=1\"\n",
        "SecAction \"id:1,phase:2,pass,msg:'%{ARGS.json.key}'\"\n",
        "SecRule REQUEST_URI \"@streq %{ARGS.json.key}\" \"id:1,phase:2,pass\"\n",
        "SecRule REQUEST_URI \"@contains /\" \"id:1,phase:2,pass,setvar:tx.value=%{ARGS.json.key}\"\n",
        "SecAction \"id:1,phase:2,pass,setvar:tx.value=%{ARGS.json.key}\"\n",
    ] {
        with_validated(source, |validated| {
            let generation = compile_validated(validated, None, CompilerLimits::default()).unwrap();
            let summary = generation.request_body_execution_summary();
            assert!(summary.requires_request_body());
            assert_eq!(summary.streaming_raw_rule_units(), 0, "{source}");
            assert_eq!(summary.streaming_structured_rule_units(), 0, "{source}");
            assert_eq!(summary.bounded_snapshot_rule_units(), 1, "{source}");
        });
    }
}

#[test]
fn unavailable_phase_two_targets_dominate_supported_lanes() {
    for target in ["XML:/root", "ARGS|XML:/root", "REQUEST_BODY|XML:/root"] {
        let source = format!("SecRule {target} \"@pm attack\" \"id:1,phase:2,pass\"\n");
        with_validated(&source, |validated| {
            let generation = compile_validated(validated, None, CompilerLimits::default()).unwrap();
            assert_eq!(
                generation.rule_units().next().unwrap().request_body_execution_class(),
                Some(RequestBodyExecutionClass::Unsupported),
                "{source}"
            );
        });
    }
}

#[test]
fn chain_child_structured_target_requires_snapshot_for_entire_unit() {
    with_validated(
        "SecRule REQUEST_URI \"@contains /\" \"id:1,phase:2,pass,chain\"\nSecRule ARGS \"@pm attack\" \"t:none\"\n",
        |validated| {
            let generation = compile_validated(validated, None, CompilerLimits::default()).unwrap();
            assert_eq!(
                generation
                    .request_body_execution_summary()
                    .bounded_snapshot_rule_units(),
                1
            );
            assert_eq!(
                generation.request_body_execution_summary().streaming_raw_rule_units(),
                0
            );
        },
    );
}

#[test]
fn optional_capture_batches_fall_back_at_required_generation_budgets() {
    let source = concat!(
        "SecRule ARGS \"@rx (evil)\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (attack)\" \"id:2,phase:2,pass,capture\"\n",
    );
    let full = with_validated(source, |v| {
        compile_validated(v, None, CompilerLimits::default()).unwrap()
    });
    assert_eq!(full.capture_hits.matchers.len(), 1);
    let required_limits = CompilerLimits {
        max_regex_programs: 2,
        ..CompilerLimits::default()
    };
    let required = with_validated(source, |v| compile_validated(v, None, required_limits).unwrap());
    assert!(required.capture_hits.matchers.is_empty());
    for limits in [
        required_limits,
        CompilerLimits {
            max_matcher_groups: 0,
            ..CompilerLimits::default()
        },
        CompilerLimits {
            max_matcher_patterns: 0,
            ..CompilerLimits::default()
        },
        CompilerLimits {
            max_matcher_source_bytes: 0,
            ..CompilerLimits::default()
        },
        CompilerLimits {
            max_compiler_owned_non_matcher_bytes: required.compiler_owned_bytes,
            ..CompilerLimits::default()
        },
        CompilerLimits {
            max_compiled_matcher_bytes: required.matcher_charged_bytes,
            ..CompilerLimits::default()
        },
        CompilerLimits {
            max_generation_bytes: required.retained_bytes(),
            ..CompilerLimits::default()
        },
        CompilerLimits {
            max_compilation_peak_bytes: required.compilation_peak_bytes(),
            max_regex_programs: 2,
            ..CompilerLimits::default()
        },
    ] {
        let g = with_validated(source, |v| compile_validated(v, None, limits).unwrap());
        assert!(g.capture_hits.matchers.is_empty());
        assert!(g.json_inspection.capture_hits.bindings.is_empty());
        assert_eq!(g.retained_bytes(), required.retained_bytes());
        assert!(g.compilation_peak_bytes() <= limits.max_compilation_peak_bytes);
    }
    assert!(full.retained_bytes() > required.retained_bytes());
    // A refused metadata projection must not become a reported allocation peak.
    let no_space = with_validated(source, |v| {
        compile_validated(
            v,
            None,
            CompilerLimits {
                max_compiler_owned_non_matcher_bytes: required.compiler_owned_bytes,
                ..CompilerLimits::default()
            },
        )
        .unwrap()
    });
    assert!(no_space.compilation_peak_bytes() <= required.compilation_peak_bytes());
}

#[test]
fn capture_batch_projection_refusal_does_not_hide_other_target_groups() {
    let large = "x".repeat(5000);
    for large_first in [false, true] {
        let small = "SecRule ARGS \"@rx (a+)\" \"id:1,phase:2,pass,capture\"\nSecRule ARGS \"@rx (b+)\" \"id:2,phase:2,pass,capture\"\n";
        let large_rules = format!("SecRule ARGS_NAMES \"@rx ({large}a)\" \"id:3,phase:2,pass,capture\"\nSecRule ARGS_NAMES \"@rx ({large}b)\" \"id:4,phase:2,pass,capture\"\n");
        let source = if large_first {
            format!("{large_rules}{small}")
        } else {
            format!("{small}{large_rules}")
        };
        let baseline = with_validated(&source, |v| {
            compile_validated(
                v,
                None,
                CompilerLimits {
                    max_regex_programs: 4,
                    ..CompilerLimits::default()
                },
            )
            .unwrap()
        });
        let g = with_validated(&source, |v| {
            compile_validated(
                v,
                None,
                CompilerLimits {
                    max_compilation_peak_bytes: baseline.compilation_peak_bytes() + 32 * 1024 * 1024,
                    ..CompilerLimits::default()
                },
            )
            .unwrap()
        });
        assert_eq!(g.capture_hits.matchers.len(), 1);
        assert_eq!(g.json_inspection.capture_hits.batches.len(), 1);
        assert_eq!(g.json_inspection.capture_hits.batches[0].matcher, g.groups.len());
        for unit in &g.units {
            let member = unit.member_start;
            let mode = if unit.id < 3 {
                g.json_inspection.members[member].value
            } else {
                g.json_inspection.members[member].name
            };
            let json_inspection_plan::JsonMemberMode::StaticFact(group) = mode else {
                panic!("static capture predicate expected");
            };
            assert_eq!(g.json_inspection.capture_hits.bindings[group].is_some(), unit.id < 3);
        }
        assert!(g.compilation_peak_bytes() <= baseline.compilation_peak_bytes() + 32 * 1024 * 1024);
    }
}

#[test]
fn capture_batch_retained_engine_rejection_has_no_phantom_charge_or_binding() {
    let source = concat!(
        "SecRule ARGS \"@rx (a+)\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (b+)\" \"id:2,phase:2,pass,capture\"\n",
        "SecRule ARGS_NAMES \"@rx ([a-z]{200})\" \"id:3,phase:2,pass,capture\"\n",
        "SecRule ARGS_NAMES \"@rx ([A-Z]{200})\" \"id:4,phase:2,pass,capture\"\n",
    );
    let baseline = with_validated(source, |v| {
        compile_validated(
            v,
            None,
            CompilerLimits {
                max_regex_programs: 4,
                ..CompilerLimits::default()
            },
        )
        .unwrap()
    });
    let full = with_validated(source, |v| {
        compile_validated(v, None, CompilerLimits::default()).unwrap()
    });
    assert_eq!(full.capture_hits.matchers.len(), 2);
    let budget = baseline.matcher_charged_bytes + full.capture_hits.matchers[0].memory_usage();
    let g = with_validated(source, |v| {
        compile_validated(
            v,
            None,
            CompilerLimits {
                max_compiled_matcher_bytes: budget,
                ..CompilerLimits::default()
            },
        )
        .unwrap()
    });
    assert_eq!(g.capture_hits.matchers.len(), 1);
    assert_eq!(g.matcher_charged_bytes, budget);
    assert_eq!(g.json_inspection.capture_hits.bindings.iter().flatten().count(), 2);
    assert_eq!(g.json_inspection.capture_hits.batches[0].matcher, g.groups.len());
    let engine_refusal = with_validated(source, |v| {
        compile_validated(
            v,
            None,
            CompilerLimits {
                matcher: MatcherLimits {
                    max_regex_nfa_bytes: 1,
                    ..MatcherLimits::default()
                },
                ..CompilerLimits::default()
            },
        )
        .unwrap()
    });
    assert!(engine_refusal.capture_hits.matchers.is_empty());
    assert_eq!(engine_refusal.retained_bytes(), baseline.retained_bytes());
}

#[test]
fn capture_batch_peak_splitting_keeps_compact_ids_and_earlier_alias_ownership() {
    let prefix = "x".repeat(700);
    for duplicate in [false, true] {
        let suffixes = if duplicate {
            ['a', 'b', 'a', 'c']
        } else {
            ['a', 'b', 'c', 'd']
        };
        let source = suffixes
            .iter()
            .enumerate()
            .map(|(id, suffix)| {
                format!(
                    "SecRule ARGS \"@rx ({prefix}{suffix})\" \"id:{},phase:2,pass,capture\"\n",
                    id + 1
                )
            })
            .collect::<String>();
        let g = with_validated(&source, |v| {
            compile_validated(v, None, CompilerLimits::default()).unwrap()
        });
        assert_eq!(g.capture_hits.matchers.len(), if duplicate { 1 } else { 2 });
        assert_eq!(
            g.json_inspection.capture_hits.batches.len(),
            g.capture_hits.matchers.len()
        );
        for (index, batch) in g.json_inspection.capture_hits.batches.iter().enumerate() {
            assert_eq!(batch.matcher, g.groups.len() + index);
            assert_eq!(batch.count, 2);
        }
        for (member, record) in g.json_inspection.members.iter().enumerate() {
            let json_inspection_plan::JsonMemberMode::StaticFact(group) = record.value else {
                panic!("static capture predicate expected");
            };
            assert_eq!(
                g.json_inspection.capture_hits.bindings[group].is_some(),
                !duplicate || member != 3
            );
        }
        assert!(g.compilation_peak_bytes() <= CompilerLimits::default().max_compilation_peak_bytes);
    }
}

#[test]
fn static_selection_prunes_exclusive_plans_but_preserves_shared_target_regex() {
    use crate::seclang::WafRuleSelection;
    let source = concat!(
        "SecRule ARGS:/^drop$/ \"@rx (drop)\" \"id:1,phase:2,pass,capture,tag:drop\"\n",
        "SecRule ARGS:/^shared$/ \"@rx same\" \"id:2,phase:2,pass,tag:keep\"\n",
        "SecRule ARGS:/^shared$/ \"@rx same\" \"id:3,phase:2,pass,tag:drop\"\n",
        "SecRule XML:/* \"@contains value\" \"id:4,phase:2,pass,tag:drop\"\n",
        "SecRule REQUEST_URI \"@contains /probe\" \"id:5,phase:1,pass,tag:keep\"\n",
    );
    let baseline = with_validated(source, |validated| {
        compile_validated(validated, None, CompilerLimits::default()).unwrap()
    });
    let program = serde_yaml::from_str::<WafRuleSelection>("exclude: {tag: drop}")
        .unwrap()
        .prepare()
        .unwrap();
    let selected = with_validated(source, |mut validated| {
        let mut work = CanonicalWork::default();
        let decision = program
            .select(&validated, &mut work, CompilerLimits::default())
            .unwrap();
        validated
            .apply_selection_mask(&decision.mask, &mut work, CompilerLimits::default())
            .unwrap();
        compile_validated(validated, None, CompilerLimits::default()).unwrap()
    });
    assert_eq!(selected.rule_units().map(|unit| unit.id()).collect::<Vec<_>>(), [2, 5]);
    assert!(baseline.capture_regexes.len() > selected.capture_regexes.len());
    assert!(selected.capture_regexes.is_empty());
    assert!(baseline.json_inspection.groups.len() > selected.json_inspection.groups.len());
    assert!(baseline.xml_inspection.xml_text.len() > selected.xml_inspection.xml_text.len());
    assert!(selected.xml_inspection.xml_text.is_empty());
    assert!(baseline.inspection_activation.steps.len() > selected.inspection_activation.steps.len());
    assert!(selected.capture_hits.matchers.is_empty());
    assert!(baseline.matcher_charged_bytes() > selected.matcher_charged_bytes());
    // One shared selector is still usable by the selected rule, while the drop-only selector disappears.
    let records: Vec<_> = selected
        .rule_units()
        .flat_map(|unit| unit.members())
        .flat_map(|member| member.target_plan().into_iter().flat_map(|plan| plan.records()))
        .collect();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.selector_regex_program_id().is_some())
            .count(),
        1
    );
}

impl<'source> MatcherState<'source> {
    fn compile(state: AssetTemplateState<'source>, limits: CompilerLimits) -> Result<Self, CompilerError> {
        Self::compile_with_work(state, limits, CanonicalWork::default())
    }
}

impl CompiledGeneration {
    fn publish(state: GroupedMatcherState<'_>, limits: CompilerLimits) -> Result<Self, CompilerError> {
        Self::publish_with_work(state, limits).map(|result| result.0)
    }
}

fn canonical_capacity_work(limits: CompilerLimits) -> MatcherCompileWork {
    MatcherCompileWork {
        limits,
        validated_bytes: 11,
        working_bytes: 13,
        compiler_owned_bytes: 0,
        matcher_charged_bytes: 17,
        peak_bytes: 0,
        requirement_cursors: [0; 8],
    }
}

#[test]
fn canonical_capacity_admission_counts_spare_storage_and_growth_overlap() {
    let mut work = canonical_capacity_work(CompilerLimits::default());
    let mut values = Vec::<u64>::new();
    work.reserve_canonical(&mut values, 0, "test arena").unwrap();
    assert_eq!(work.compiler_owned_bytes, 0);
    work.reserve_canonical(&mut values, 4, "test arena").unwrap();
    let old_bytes = values.capacity() * size_of::<u64>();
    assert_eq!(work.compiler_owned_bytes, old_bytes);
    values.push(1);
    work.reserve_canonical(&mut values, 1, "test arena").unwrap();
    assert_eq!(work.compiler_owned_bytes, old_bytes);
    values.resize(values.capacity(), 1);
    work.reserve_canonical(&mut values, 1, "test arena").unwrap();
    let new_bytes = values.capacity() * size_of::<u64>();
    assert_eq!(work.compiler_owned_bytes, new_bytes);
    assert_eq!(work.peak_bytes, 11 + 13 + 17 + old_bytes + new_bytes);
    assert_eq!(values, vec![1; old_bytes / size_of::<u64>()]);
    assert_eq!(
        work.reserve_canonical(&mut values, usize::MAX, "test arena"),
        Err(CompilerError::PreflightArithmetic)
    );
}

#[test]
fn canonical_capacity_limits_admit_exact_sizes_and_reject_before_growth() {
    for boundary in 0..3 {
        for short in [false, true] {
            let mut values = vec![1_u64; 4];
            let old = values.capacity() * size_of::<u64>();
            let new = (values.len() + 1) * size_of::<u64>();
            let mut limits = CompilerLimits::default();
            let deficit = usize::from(short);
            match boundary {
                0 => limits.max_compiler_owned_non_matcher_bytes = new - deficit,
                1 => limits.max_generation_bytes = new + 17 - deficit,
                _ => limits.max_compilation_peak_bytes = 11 + 13 + 17 + old + new - deficit,
            }
            let mut work = canonical_capacity_work(limits);
            work.compiler_owned_bytes = old;
            let result = work.reserve_canonical(&mut values, 1, "test arena");
            assert_eq!(result.is_ok(), !short, "boundary {boundary}: {result:?}");
            if short {
                assert_eq!(values.capacity() * size_of::<u64>(), old);
                let expected = match boundary {
                    0 => CompilerError::OwnedByteLimit { limit: new - 1 },
                    1 => CompilerError::GenerationByteLimit { limit: new + 17 - 1 },
                    _ => CompilerError::PeakByteLimit {
                        limit: 11 + 13 + 17 + old + new - 1,
                    },
                };
                assert_eq!(result, Err(expected));
            } else {
                assert_eq!(work.compiler_owned_bytes, values.capacity() * size_of::<u64>());
            }
        }
    }
}

fn grouped_arena_capacity(grouped: &GroupedMatcherState<'_>) -> usize {
    grouped.target_bytes.capacity()
        + grouped.target_records.capacity() * size_of::<CompiledTargetRecord>()
        + grouped.target_plans.capacity() * size_of::<CompiledTargetPlan>()
        + grouped.canonical_transformations.capacity() * size_of::<TransformationKind>()
        + grouped.transformation_plans.capacity() * size_of::<CompiledTransformationPlan>()
}

#[test]
fn canonical_grouping_retains_all_five_capacities_after_deduplication() {
    for text in [
        concat!(
            "SecRule REQUEST_HEADERS:key \"@eq 1\" \"id:1,phase:1,pass,t:lowercase\"\n",
            "SecRule REQUEST_HEADERS:key \"@eq 2\" \"id:2,phase:1,pass,t:lowercase\"\n",
            "SecRule REQUEST_HEADERS:other \"@eq 3\" \"id:3,phase:1,pass\"\n",
        ),
        concat!(
            "SecRule XML:/* \"@eq 1\" \"id:1,phase:2,pass,t:length,chain\"\n",
            "SecRule XML:/* \"@eq 2\" \"t:length\"\n",
            "SecRule XML://@* \"@eq 3\" \"id:3,phase:2,pass\"\n",
        ),
    ] {
        with_validated(text, |validated| {
            let limits = CompilerLimits::default();
            let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
            let matched = MatcherState::compile(prepared, limits).unwrap();
            let incoming = matched.compiler_owned_bytes;
            let replaced = matched.asset_templates.transformations.len() * size_of::<TransformationKind>();
            let grouped = GroupedMatcherState::compile(matched, limits).unwrap();
            assert!(grouped.groups.is_empty());
            assert!(grouped.pattern_mappings.is_empty());
            assert_eq!(
                grouped.compiler_owned_bytes,
                incoming - replaced + grouped_arena_capacity(&grouped)
            );
            assert_eq!(grouped.target_plans.len(), 2);
            assert_eq!(grouped.target_records.len(), 2);
            assert_eq!(grouped.transformation_plans.len(), 2);
            assert_eq!(grouped.canonical_transformations.len(), 1);
            assert!(grouped.target_plans.capacity() >= 3);
            let members = &grouped.matchers.asset_templates.members;
            assert_eq!(members[0].target_plan, members[1].target_plan);
            assert_ne!(members[1].target_plan, members[2].target_plan);
        });
    }
}

#[test]
fn canonical_publication_keeps_capacity_charge_and_accounts_ctl_appends() {
    for ctl in [
        "",
        ",ctl:ruleRemoveTargetByTag=probe;ARGS:x",
        ",ctl:ruleRemoveTargetByTag=probe;ARGS:long-selector,ctl:ruleRemoveTargetByTag=probe;XML:/*",
    ] {
        let text = format!(
            "SecRule REQUEST_HEADERS:key \"@eq 1\" \"id:1,phase:1,pass,t:lowercase{ctl}\"\n\
             SecRule REQUEST_HEADERS:key \"@eq 2\" \"id:2,phase:1,pass,t:lowercase\"\n"
        );
        with_validated(&text, |validated| {
            let limits = CompilerLimits::default();
            let prepared = CompilerBuilder::new(validated, None, limits).prepare().unwrap();
            let matched = MatcherState::compile(prepared, limits).unwrap();
            let incoming = matched.compiler_owned_bytes;
            let replaced = matched.asset_templates.transformations.len() * size_of::<TransformationKind>();
            let grouped = GroupedMatcherState::compile(matched, limits).unwrap();
            let old_capacity = grouped.target_bytes.capacity();
            let initial_capacity = grouped_arena_capacity(&grouped);
            let (generation, _) = CompiledGeneration::publish_with_work(grouped, limits).unwrap();
            let final_capacity = generation.target_bytes.capacity()
                + generation.target_records.capacity() * size_of::<CompiledTargetRecord>()
                + generation.target_plans.capacity() * size_of::<CompiledTargetPlan>()
                + generation.transformations.capacity() * size_of::<TransformationKind>()
                + generation.transformation_plans.capacity() * size_of::<CompiledTransformationPlan>();
            assert_eq!(
                generation.compiler_owned_bytes(),
                incoming - replaced
                    + final_capacity
                    + 2 * size_of::<CompiledRuleUnit>()
                    + 2 * size_of::<CompiledScheduleEntry>()
                    + (generation.json_inspection.members.capacity() + generation.xml_inspection.members.capacity())
                        * size_of::<json_inspection_plan::JsonMemberPlan>()
            );
            assert_eq!(generation.retained_bytes(), generation.compiler_owned_bytes());
            if ctl.contains("long-selector") {
                assert!(generation.target_bytes.capacity() > old_capacity);
                assert_eq!(generation.target_bytes, b"keylong-selector/*");
                assert!(final_capacity > initial_capacity);
            } else {
                assert_eq!(generation.target_bytes.capacity(), old_capacity);
                assert_eq!(final_capacity, initial_capacity);
                assert_eq!(
                    generation.target_bytes,
                    if ctl.is_empty() {
                        b"key".as_slice()
                    } else {
                        b"keyx".as_slice()
                    }
                );
            }
            for action in &generation.actions {
                if let CompiledActionOperand::Ctl(CompiledCtl::RuleRemoveTargetByTag {
                    selector: CompiledCtlSelector::Resolved(selector),
                    ..
                }) = action.operand
                {
                    let (CompiledTargetSelector::Literal(range) | CompiledTargetSelector::Path(range)) = selector
                    else {
                        panic!("expected byte selector");
                    };
                    let bytes = byte_range(&generation.target_bytes, range).unwrap();
                    assert!(matches!(bytes, b"x" | b"long-selector" | b"/*"));
                }
            }
        });
    }
}
