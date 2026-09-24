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
            Some(RequestBodyExecutionClass::StreamingStructured),
            Some(RequestBodyExecutionClass::BoundedSnapshot),
            Some(RequestBodyExecutionClass::StreamingRaw),
            None,
            Some(RequestBodyExecutionClass::BoundedSnapshot),
        ]
    );
    let summary = generation.request_body_execution_summary();
    assert_eq!(summary.streaming_raw_rule_units(), 2);
    assert_eq!(summary.streaming_structured_rule_units(), 1);
    assert_eq!(summary.bounded_snapshot_rule_units(), 2);
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
