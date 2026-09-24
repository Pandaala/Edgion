use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use super::*;
use crate::seclang::{parse_syntax, scan_source, SourceLimits, SyntaxLimits};

fn action_macro_capable(kind: ActionKind) -> bool {
    matches!(
        kind,
        ActionKind::Msg | ActionKind::LogData | ActionKind::SetVar | ActionKind::InitCol
    )
}

fn with_draft<T>(text: &str, inspect: impl FnOnce(&SemanticDraft<'_>) -> T) -> Result<T, SemanticError> {
    with_draft_limits(text, SemanticLimits::default(), inspect)
}

fn with_draft_limits<T>(
    text: &str,
    limits: SemanticLimits,
    inspect: impl FnOnce(&SemanticDraft<'_>) -> T,
) -> Result<T, SemanticError> {
    let source = scan_source("test.conf", text.as_bytes(), SourceLimits::default()).expect("source");
    let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
    analyze_semantic_draft_inner(vec![syntax], limits, false, false).map(|draft| inspect(&draft))
}

fn crs_validation_context() -> SemanticValidationContext {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).expect("valid test defaults")
}

fn with_validated<T>(
    text: &str,
    context: SemanticValidationContext,
    limits: ValidationLimits,
    inspect: impl FnOnce(&ValidatedSemanticDocument<'_>) -> T,
) -> Result<T, SemanticValidationError> {
    let source = scan_source("test.conf", text.as_bytes(), SourceLimits::default()).expect("source");
    let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
    let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).expect("semantic draft");
    validate_semantic_draft(draft, context, limits).map(|validated| inspect(&validated))
}

fn with_full_draft<T>(text: &str, inspect: impl FnOnce(&SemanticDraft<'_>) -> T) -> Result<T, SemanticError> {
    let source = scan_source("test.conf", text.as_bytes(), SourceLimits::default()).expect("source");
    let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
    analyze_one_semantic_draft(syntax, SemanticLimits::default()).map(|draft| inspect(&draft))
}

fn one_value(text: &str) -> Result<LocalActionValue, SemanticError> {
    with_draft(text, |draft| draft.actions().next().expect("action").local_value())
}

fn valid_operator_argument(kind: OperatorKind) -> &'static str {
    match kind {
        OperatorKind::DetectSqli
        | OperatorKind::DetectXss
        | OperatorKind::UnconditionalMatch
        | OperatorKind::ValidateUrlEncoding
        | OperatorKind::ValidateUtf8Encoding => "",
        OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt => "1",
        OperatorKind::IpMatch => "192.0.2.1",
        OperatorKind::ValidateByteRange => "0-255",
        _ => "x",
    }
}

#[test]
fn registries_have_frozen_cardinality_and_case_policy() {
    assert_eq!(
        (
            CollectionKind::ALL.len(),
            OperatorKind::ALL.len(),
            ActionKind::ALL.len(),
            TransformationKind::ALL.len()
        ),
        (33, 19, 23, 20)
    );
    for &kind in CollectionKind::ALL {
        assert_eq!(
            CollectionKind::lookup_ascii_insensitive(kind.as_str().to_ascii_lowercase().as_bytes()),
            Some(kind)
        );
    }
    for &kind in ActionKind::ALL {
        assert_eq!(
            ActionKind::lookup_ascii_insensitive(kind.as_str().to_ascii_uppercase().as_bytes()),
            Some(kind)
        );
    }
    for &kind in TransformationKind::ALL {
        assert_eq!(
            TransformationKind::lookup_ascii_insensitive(kind.as_str().to_ascii_uppercase().as_bytes()),
            Some(kind)
        );
    }
    let mut rules = String::new();
    for (index, kind) in OperatorKind::ALL.iter().enumerate() {
        rules.push_str(&format!(
            "SecRule ARGS \"@{} {}\" \"id:{}\"\n",
            kind.as_str(),
            valid_operator_argument(*kind),
            index + 1
        ));
    }
    assert_eq!(
        with_draft(&rules, |draft| draft
            .operators()
            .map(|record| record.kind())
            .collect::<Vec<_>>()),
        Ok(OperatorKind::ALL.to_vec())
    );
    assert_eq!(
        with_draft("SecRule ARGS \"@rx x\" \"id:1\"\n", |draft| draft
            .operators()
            .next()
            .unwrap()
            .kind()),
        Ok(OperatorKind::Rx)
    );
    assert!(matches!(
        with_draft("SecRule ARGS \"@RX x\" \"id:1\"\n", |_| ()),
        Err(SemanticError::UnknownOperator { .. })
    ));
}

#[test]
fn classifies_typed_records_and_separates_requirements_from_capabilities() {
    let summary = with_draft(
            "SecRule RESPONSE_BODY \"@rx %{TX.pattern}\" \"id:001,phase:request,severity:WARNING,status:403,skip:1,t:lowercase,block,ctl:auditEngine=Off\"\n",
            |draft| {
                (
                    draft.target_count(), draft.operator_count(), draft.action_count(),
                    draft.actions().find_map(|action| action.transformation()),
                    draft.requirements().map(|record| record.requirement()).collect::<Vec<_>>(),
                    draft.unavailable_capabilities().map(|record| record.capability()).collect::<Vec<_>>(),
                )
            },
        ).expect("semantic draft");
    assert_eq!(summary.0..=summary.2, 1..=8);
    assert_eq!(summary.3, Some(TransformationKind::Lowercase));
    assert_eq!(
        summary.4,
        [
            CompileRequirement::RegexCompilation,
            CompileRequirement::BlockDefaultDisruptive
        ]
    );
    assert_eq!(
        summary.5,
        [UnavailableCapability::FullResponseBody, UnavailableCapability::Ctl]
    );
}

#[test]
fn scalar_boundaries_and_profiles_are_exact() {
    assert_eq!(one_value("SecAction \"id:0001\"\n"), Ok(LocalActionValue::Id(1)));
    assert_eq!(
        one_value("SecAction \"phase:logging\"\n"),
        Ok(LocalActionValue::Phase(Phase::Logging))
    );
    assert_eq!(
        one_value("SecAction \"severity:CrItIcAl\"\n"),
        Ok(LocalActionValue::Severity(Severity::Critical))
    );
    assert_eq!(
        one_value("SecAction \"status:0599\"\n"),
        Ok(LocalActionValue::Status(599))
    );
    assert_eq!(
        one_value(&format!("SecAction \"skip:{}\"\n", u32::MAX)),
        Ok(LocalActionValue::Skip(u32::MAX))
    );
    for input in [
        "SecAction \"id:0\"\n",
        "SecAction \"phase:Request\"\n",
        "SecAction \"severity:08\"\n",
        "SecAction \"status:100\"\n",
        "SecAction \"status:101\"\n",
        "SecAction \"status:199\"\n",
        "SecAction \"status:600\"\n",
        "SecAction \"skip:0\"\n",
    ] {
        assert!(one_value(input).is_err(), "{input}");
    }
    assert!(matches!(
        one_value("SecAction \"id:%{TX.id}\"\n"),
        Err(SemanticError::MacroScalar {
            kind: ScalarKind::Id,
            ..
        })
    ));
    assert!(matches!(
        one_value("SecAction \"skip:4294967296\"\n"),
        Err(SemanticError::ScalarOverflow {
            kind: ScalarKind::Skip,
            ..
        })
    ));
}

fn with_fold<T>(text: &str, inspect: impl FnOnce(&SemanticDraft<'_>) -> T) -> Result<T, SemanticError> {
    let source = scan_source("fold.conf", text.as_bytes(), SourceLimits::default()).expect("source");
    let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
    analyze_semantic_draft_inner(vec![syntax], SemanticLimits::default(), true, false).map(|draft| inspect(&draft))
}

#[test]
fn local_fold_exposes_roles_presence_logging_and_normalized_transformations() {
    let summary = with_fold(
        "SecRule ARGS \"@rx x\" \"id:1,block,phase:1,log,nolog,auditlog,t:lowercase,t:none,t:length,chain\"\n\
             SecRule ARGS \"@rx y\"\n\
             SecAction \"id:2,pass,t:none\"\n",
        |draft| {
            let folds = draft.folded_rules().collect::<Vec<_>>();
            assert_eq!(draft.directive(0).unwrap().local_rule_fold().unwrap().id(), Some(1));
            (
                folds.iter().map(|fold| fold.envelope()).collect::<Vec<_>>(),
                folds.iter().map(|fold| fold.role()).collect::<Vec<_>>(),
                folds.iter().map(|fold| fold.action_presence()).collect::<Vec<_>>(),
                folds[0].log_override(),
                folds[0].audit_log_override(),
                folds[0]
                    .effective_transformations()
                    .map(|action| action.transformation().unwrap())
                    .collect::<Vec<_>>(),
                folds[0].reset_action().unwrap().transformation(),
                draft
                    .requirements()
                    .filter(|record| record.requirement() == CompileRequirement::DefaultPhase)
                    .count(),
            )
        },
    )
    .unwrap();
    assert_eq!(
        summary.0,
        [
            LocalRuleEnvelope::Rule,
            LocalRuleEnvelope::Rule,
            LocalRuleEnvelope::Action
        ]
    );
    assert_eq!(summary.1[0], Some(ProvisionalRuleRole::Root));
    assert_eq!(
        summary.1[1],
        Some(ProvisionalRuleRole::Child {
            predecessor: DirectiveLocation {
                document_index: 0,
                directive_index: 0
            },
            root: DirectiveLocation {
                document_index: 0,
                directive_index: 0
            },
        })
    );
    assert_eq!(summary.1[2], None);
    assert_eq!(
        summary.2,
        [
            FoldActionPresence::PresentNonempty,
            FoldActionPresence::Absent,
            FoldActionPresence::PresentNonempty
        ]
    );
    assert_eq!((summary.3, summary.4), (Some(false), Some(true)));
    assert_eq!(summary.5, [TransformationKind::Length]);
    assert_eq!(summary.6, Some(TransformationKind::None));
    assert_eq!(summary.7, 1);
}

#[test]
fn fold_adjacency_crosses_documents_and_non_rules_interrupt_it() {
    let first = scan_source(
        "first",
        b"SecRule ARGS \"@rx x\" \"id:1,block,phase:1,chain\"\n",
        SourceLimits::default(),
    )
    .unwrap();
    let second = scan_source("second", b"SecRule ARGS \"@rx y\"\n", SourceLimits::default()).unwrap();
    let draft = analyze_semantic_draft(
        vec![
            parse_syntax(&first, SyntaxLimits::default()).unwrap(),
            parse_syntax(&second, SyntaxLimits::default()).unwrap(),
        ],
        SemanticLimits::default(),
    )
    .unwrap();
    assert!(matches!(
        draft.folded_rules().nth(1).unwrap().role(),
        Some(ProvisionalRuleRole::Child {
            predecessor: DirectiveLocation { document_index: 0, .. },
            ..
        })
    ));

    assert!(matches!(
        with_fold(
            "SecRule ARGS \"@rx x\" \"id:1,block,phase:1,chain\"\nSecMarker END\nSecRule ARGS \"@rx y\"\n",
            |_| ()
        ),
        Err(SemanticError::FoldMissing {
            kind: FoldRequiredKind::Id,
            ..
        })
    ));
}

#[test]
fn fold_cardinality_secaction_and_status_pass_errors_are_typed_and_redacted() {
    let cases = [
        ("SecAction \"id:1,id:2,pass\"\n", "FoldDuplicate"),
        ("SecAction \"id:1,pass,capture\"\n", "SecActionForbidden"),
        ("SecAction \"id:1,pass,t:lowercase\"\n", "SecActionForbidden"),
        ("SecAction \"id:1,pass,status:403\"\n", "StatusPass"),
        ("SecAction \"pass\"\n", "FoldMissing"),
        ("SecAction \"id:1\"\n", "FoldMissing"),
    ];
    for (source, family) in cases {
        let error = with_fold(source, |_| ()).unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert!(rendered.contains(family), "{rendered}");
        assert!(!rendered.contains("403"));
    }
}

#[test]
fn fold_cross_category_error_precedence_is_frozen() {
    let cases = [
        ("SecAction \"capture,id:1,id:2\"\n", FoldErrorFamily::Duplicate),
        ("SecAction \"capture,pass,status:403\"\n", FoldErrorFamily::Forbidden),
        ("SecAction \"pass,status:403\"\n", FoldErrorFamily::MissingId),
        ("SecAction \"id:1,status:403\"\n", FoldErrorFamily::MissingDisruptive),
        ("SecAction \"id:1,pass,status:403\"\n", FoldErrorFamily::StatusPass),
    ];
    for (source, expected) in cases {
        let error = with_fold(source, |_| ()).unwrap_err();
        let actual = match error {
            SemanticError::FoldDuplicate { .. } => FoldErrorFamily::Duplicate,
            SemanticError::SecActionForbidden { .. } => FoldErrorFamily::Forbidden,
            SemanticError::FoldMissing {
                kind: FoldRequiredKind::Id,
                ..
            } => FoldErrorFamily::MissingId,
            SemanticError::FoldMissing {
                kind: FoldRequiredKind::Disruptive,
                ..
            } => FoldErrorFamily::MissingDisruptive,
            SemanticError::StatusPass { .. } => FoldErrorFamily::StatusPass,
            other => panic!("unexpected fold error: {other:?}"),
        };
        assert_eq!(actual, expected, "{source}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoldErrorFamily {
    Duplicate,
    Forbidden,
    MissingId,
    MissingDisruptive,
    StatusPass,
}

#[test]
fn later_content_error_precedes_early_effective_aggregate_excess() {
    let source = scan_source(
        "precedence",
        b"SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,t:lowercase\"\nSecAction \"id:2,id:3,pass\"\n",
        SourceLimits::default(),
    )
    .unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(matches!(
        analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                fold: FoldLimits {
                    max_effective_transformations: 0,
                    ..FoldLimits::default()
                },
                ..SemanticLimits::default()
            }
        ),
        Err(SemanticError::FoldDuplicate {
            kind: FoldSingletonKind::Id,
            ..
        })
    ));
}

#[test]
fn dangling_chain_invalid_child_and_present_empty_are_deferred() {
    assert!(with_fold("SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n", |_| ()).is_ok());
    let presence = with_fold(
        "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n\
             SecRule ARGS \"@rx y\" \"id:2,phase:2,deny,skip:1,msg:x,chain\"\n\
             SecRule ARGS \"@rx z\" \"\"\n",
        |draft| {
            draft
                .folded_rules()
                .map(|fold| fold.action_presence())
                .collect::<Vec<_>>()
        },
    )
    .unwrap();
    assert_eq!(
        presence,
        [
            FoldActionPresence::PresentNonempty,
            FoldActionPresence::PresentNonempty,
            FoldActionPresence::PresentEmpty,
        ]
    );
}

fn with_graph<T>(text: &str, inspect: impl FnOnce(&SemanticDraft<'_>) -> T) -> Result<T, SemanticError> {
    let source = scan_source("graph.conf", text.as_bytes(), SourceLimits::default()).expect("source");
    let syntax = parse_syntax(&source, SyntaxLimits::default()).expect("syntax");
    analyze_one_semantic_draft(syntax, SemanticLimits::default()).map(|draft| inspect(&draft))
}

#[test]
fn final_graph_exposes_owner_bound_roles_phases_chains_markers_and_resolution() {
    let summary = with_graph(
        "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain,skipAfter:END\"\n\
             SecRule ARGS \"@rx y\" \"t:lowercase\"\n\
             SecRule ARGS \"@rx z\" \"id:2,pass\"\n\
             SecAction \"id:3,pass,phase:2\"\n\
             SecMarker END\n",
        |draft| {
            let rules = draft.final_rules().collect::<Vec<_>>();
            let chain = draft.chains().next().unwrap();
            let resolution = draft.resolved_skip_afters().next().unwrap();
            (
                rules.iter().map(|rule| rule.role()).collect::<Vec<_>>(),
                rules.iter().map(|rule| rule.phase()).collect::<Vec<_>>(),
                chain
                    .members()
                    .map(|member| (member.ordinal(), member.rule().location()))
                    .collect::<Vec<_>>(),
                draft
                    .markers()
                    .map(|marker| (marker.name().to_owned(), marker.location()))
                    .collect::<Vec<_>>(),
                (
                    resolution.referrer(),
                    resolution.source_action().kind(),
                    resolution.target().name().to_owned(),
                ),
                draft.directive(0).unwrap().final_rule().unwrap().location(),
                draft.folded_rules().next().unwrap().final_rule().unwrap().location(),
            )
        },
    )
    .unwrap();
    assert!(matches!(summary.0[0], FinalRuleRole::ChainRoot { .. }));
    assert!(matches!(summary.0[1], FinalRuleRole::ChainChild { ordinal: 1, .. }));
    assert_eq!(summary.0[2], FinalRuleRole::StandaloneRule);
    assert_eq!(summary.0[3], FinalRuleRole::Action);
    assert!(matches!(
        summary.1[0],
        FinalPhaseState::Explicit {
            phase: Phase::RequestHeaders,
            ..
        }
    ));
    assert!(matches!(
        summary.1[1],
        FinalPhaseState::Inherited {
            phase: Phase::RequestHeaders,
            ..
        }
    ));
    assert!(matches!(summary.1[2], FinalPhaseState::RequiresDefault { .. }));
    assert_eq!(summary.2.len(), 2);
    assert_eq!(summary.3[0].0, "END");
    assert_eq!(summary.4 .1, ActionEnvelopeKind::Action(ActionKind::SkipAfter));
    assert_eq!(summary.4 .2, "END");
    assert_eq!(
        summary.5,
        DirectiveLocation {
            document_index: 0,
            directive_index: 0
        }
    );
    assert_eq!(summary.6, summary.5);
}

#[test]
fn semantic_targets_and_exception_selectors_preserve_owner_bound_source_views() {
    let summary = with_draft(
        r#"SecRule ARGS|REQUEST_HEADERS:name|XML:/x|REQUEST_HEADERS:/a\/b/|REQUEST_HEADERS:'/c/' "@rx x"
SecRuleUpdateTargetById 1-2 ARGS
SecRuleRemoveById 3 4-5
"#,
        |draft| {
            let targets = draft.targets().copied().collect::<Vec<_>>();
            let update = draft.directive(1).unwrap();
            let remove = draft.directive(2).unwrap();
            (
                targets
                    .iter()
                    .map(|target| {
                        (
                            target.collection_spelling().text().to_owned(),
                            target.selector_kind(),
                            target.selector().map(|value| value.text().to_owned()),
                            target.regex_body().map(|value| value.text().to_owned()),
                            target.span(),
                        )
                    })
                    .collect::<Vec<_>>(),
                update
                    .id_selectors()
                    .iter()
                    .map(|value| value.kind())
                    .collect::<Vec<_>>(),
                update.targets().len(),
                remove
                    .id_selectors()
                    .iter()
                    .map(|value| value.kind())
                    .collect::<Vec<_>>(),
                remove.targets().len(),
            )
        },
    )
    .unwrap();
    assert_eq!(summary.0.len(), 6);
    assert_eq!(summary.0[0].1, SelectorKind::None);
    assert_eq!((summary.0[0].2.as_deref(), summary.0[0].3.as_deref()), (None, None));
    assert_eq!(summary.0[1].1, SelectorKind::Literal);
    assert_eq!(
        (summary.0[1].2.as_deref(), summary.0[1].3.as_deref()),
        (Some("name"), None)
    );
    assert_eq!(summary.0[2].1, SelectorKind::Path);
    assert_eq!(
        (summary.0[2].2.as_deref(), summary.0[2].3.as_deref()),
        (Some("/x"), None)
    );
    assert_eq!(summary.0[3].1, SelectorKind::Regex(crate::seclang::RegexStyle::Bare));
    assert_eq!(
        (summary.0[3].2.as_deref(), summary.0[3].3.as_deref()),
        (Some("/a\\/b/"), Some("a\\/b"))
    );
    assert_eq!(
        summary.0[4].1,
        SelectorKind::Regex(crate::seclang::RegexStyle::SingleQuoted)
    );
    assert_eq!(
        (summary.0[4].2.as_deref(), summary.0[4].3.as_deref()),
        (Some("'/c/'"), Some("c"))
    );
    assert_eq!(summary.1, [crate::seclang::IdSelectorKind::Range { start: 1, end: 2 }]);
    assert_eq!(summary.2, 1);
    assert_eq!(
        summary.3,
        [
            crate::seclang::IdSelectorKind::Single(3),
            crate::seclang::IdSelectorKind::Range { start: 4, end: 5 }
        ]
    );
    assert_eq!(summary.4, 0);
    assert!(summary
        .0
        .iter()
        .all(|value| value.4.start.byte_offset < value.4.end.byte_offset));

    with_draft("SecRule REQUEST_HEADERS:na\\\nme \"@rx x\"\n", |draft| {
        let target = *draft.targets().next().unwrap();
        let selector = target.selector().unwrap();
        assert_eq!(selector.text(), "name");
        assert_eq!((selector.span().start.line, selector.span().end.line), (1, 2));
        assert_eq!((target.span().start.line, target.span().end.line), (1, 2));
        let line = &draft.documents[0].source().lines()[0];
        let line_start = line.text().as_ptr() as usize;
        let line_end = line_start + line.text().len();
        assert!((line_start..line_end).contains(&(selector.text().as_ptr() as usize)));
    })
    .unwrap();
}

#[test]
fn remove_envelope_interrupts_chains_and_target_metadata_charge_is_exact() {
    assert!(matches!(
            with_graph(
                "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRuleRemoveById 1\nSecRule ARGS \"@rx y\" \"id:2,pass,phase:1\"\n",
                |_| ()
            ),
            Err(SemanticError::DanglingChain { .. })
        ));

    let source = scan_source(
        "target-charge",
        b"SecRule ARGS|REQUEST_HEADERS:name \"@rx x\"",
        SourceLimits::default(),
    )
    .unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let inherited = syntax.retained_charged_bytes();
    let exact = analyze_semantic_draft_inner(vec![syntax], SemanticLimits::default(), false, false)
        .unwrap()
        .semantic_charged_bytes();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(analyze_semantic_draft_inner(
        vec![syntax],
        SemanticLimits {
            max_owned_bytes: exact,
            max_retained_bytes: inherited + exact,
            ..SemanticLimits::default()
        },
        false,
        false,
    )
    .is_ok());
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(matches!(
        analyze_semantic_draft_inner(
            vec![syntax],
            SemanticLimits {
                max_owned_bytes: exact - 1,
                max_retained_bytes: usize::MAX,
                ..SemanticLimits::default()
            },
            false,
            false,
        ),
        Err(SemanticError::PreflightOwnedByteLimit { .. } | SemanticError::OwnedByteLimit { .. })
    ));
    #[cfg(target_pointer_width = "64")]
    assert_eq!(size_of::<SemanticTarget<'_>>(), 328);
}

#[test]
fn graph_rejects_every_child_restriction_and_dangling_interruption() {
    let forbidden = [
        "id:2",
        "phase:2",
        "block",
        "deny",
        "pass",
        "skip:1",
        "skipAfter:END",
        "status:403",
        "severity:error",
        "msg:x",
        "logdata:x",
        "tag:x",
        "ver:x",
    ];
    for action in forbidden {
        let source = format!(
            "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"{action}\"\nSecMarker END\n"
        );
        assert!(
            matches!(with_graph(&source, |_| ()), Err(SemanticError::ChildAction { .. })),
            "{action}"
        );
    }
    for interruption in ["", "SecMarker X\n", "SecAction \"id:2,pass,phase:1\"\n"] {
        let source = format!("SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n{interruption}");
        assert!(matches!(
            with_graph(&source, |_| ()),
            Err(SemanticError::DanglingChain { .. })
        ));
    }
    assert!(with_graph(
            "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"t:lowercase,capture,multiMatch,setvar:TX.a=1,log\"\n",
            |_| ()
        ).is_ok());
}

#[test]
fn child_structural_candidates_follow_textual_action_order() {
    let forbidden_first =
        "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"skipAfter:END,chain\"\n";
    let error = with_graph(forbidden_first, |_| ()).unwrap_err();
    assert!(matches!(
        error,
        SemanticError::ChildAction {
            kind: ActionKind::SkipAfter,
            position
        } if position.byte_offset == forbidden_first.find("skipAfter").unwrap()
    ));

    let relationship_first =
        "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"chain,skipAfter:END\"\n";
    let error = with_graph(relationship_first, |_| ()).unwrap_err();
    assert!(matches!(
        error,
        SemanticError::DanglingChain { position }
            if position.byte_offset == relationship_first.rfind("chain").unwrap()
    ));
}

#[test]
fn invalid_separated_chain_roots_have_linear_projection_work() {
    const ROOTS: usize = 1_024;
    let mut text = String::new();
    for id in 1..=ROOTS {
        text.push_str(&format!(
            "SecRule ARGS \"@rx x\" \"id:{id},pass,phase:1,chain\"\nSecMarker M{id}\n"
        ));
    }
    let source = scan_source("linear.conf", text.as_bytes(), SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let draft = analyze_semantic_draft_inner(vec![syntax], SemanticLimits::default(), true, false).unwrap();
    let projection = project_chains(&draft.folds, &draft.actions, GraphLimits::default());
    assert_eq!(projection.inspected_folds, draft.folds.len());
    assert_eq!(projection.chain_count, ROOTS);
    assert_eq!(projection.chain_members, ROOTS);
}

#[test]
fn marker_namespace_and_forward_resolution_are_exact_and_redacted() {
    assert!(with_graph("SecMarker Case\nSecMarker case\n", |draft| draft.markers().len()).is_ok());
    assert!(with_graph("SecMarker 123\nSecMarker \"a\tb\"\nSecMarker café\n", |_| ()).is_ok());
    for source in ["SecMarker SAME\nSecMarker \"SAME\"\n", "SecMarker \"bad\u{7f}\"\n"] {
        assert!(with_graph(source, |_| ()).is_err());
    }
    assert!(matches!(
        with_graph("SecMarker END\nSecAction \"id:1,pass,phase:1,skipAfter:END\"\n", |_| ()),
        Err(SemanticError::NonForwardSkipAfter { .. })
    ));
    let error = with_graph("SecAction \"id:1,pass,phase:1,skipAfter:SECRET\"\n", |_| ()).unwrap_err();
    assert!(matches!(error, SemanticError::UnresolvedSkipAfter { .. }));
    assert!(!format!("{error:?} {error}").contains("SECRET"));
}

#[test]
fn generated_chain_lengths_and_cross_document_many_to_one_resolution_match_model() {
    for length in 2..=8_usize {
        let mut source = String::from("SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n");
        for ordinal in 1..length {
            let actions = if ordinal + 1 == length { "" } else { " \"chain\"" };
            source.push_str(&format!("SecRule ARGS \"@rx x\"{actions}\n"));
        }
        let ordinals = with_graph(&source, |draft| {
            draft
                .chains()
                .next()
                .unwrap()
                .members()
                .map(|member| member.ordinal())
                .collect::<Vec<_>>()
        })
        .unwrap();
        assert_eq!(ordinals, (0..length).collect::<Vec<_>>());
    }
    let first = scan_source(
        "first",
        b"SecAction \"id:1,pass,phase:1,skipAfter:END\"\nSecAction \"id:2,pass,phase:2,skipAfter:END\"\n",
        SourceLimits::default(),
    )
    .unwrap();
    let second = scan_source("second", b"SecMarker END\n", SourceLimits::default()).unwrap();
    let draft = analyze_semantic_draft(
        vec![
            parse_syntax(&first, SyntaxLimits::default()).unwrap(),
            parse_syntax(&second, SyntaxLimits::default()).unwrap(),
        ],
        SemanticLimits::default(),
    )
    .unwrap();
    assert_eq!(draft.resolved_skip_afters().len(), 2);
    assert!(draft
        .resolved_skip_afters()
        .all(|resolution| resolution.target().location().document_index() == 1));
}

#[test]
fn graph_limits_accept_exact_and_reject_first_excess() {
    let text =
        "SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain,skipAfter:END\"\nSecRule ARGS \"@rx y\"\nSecMarker END\n";
    let source = scan_source("limits", text.as_bytes(), SourceLimits::default()).unwrap();
    let exact = GraphLimits {
        max_chains: 1,
        max_chain_members: 2,
        max_members_per_chain: 2,
        max_markers: 1,
        max_marker_name_bytes: 3,
        max_marker_name_bytes_total: 3,
        max_skip_after_refs: 1,
        max_registry_work_bytes: MARKER_WORK_ENTRY_CHARGE + 6,
    };
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(analyze_one_semantic_draft(
        syntax,
        SemanticLimits {
            graph: exact,
            ..SemanticLimits::default()
        }
    )
    .is_ok());
    for graph in [
        GraphLimits { max_chains: 0, ..exact },
        GraphLimits {
            max_chain_members: 1,
            ..exact
        },
        GraphLimits {
            max_members_per_chain: 1,
            ..exact
        },
        GraphLimits {
            max_markers: 0,
            ..exact
        },
        GraphLimits {
            max_marker_name_bytes: 2,
            ..exact
        },
        GraphLimits {
            max_marker_name_bytes_total: 2,
            ..exact
        },
        GraphLimits {
            max_skip_after_refs: 0,
            ..exact
        },
        GraphLimits {
            max_registry_work_bytes: MARKER_WORK_ENTRY_CHARGE + 5,
            ..exact
        },
    ] {
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                graph,
                ..SemanticLimits::default()
            }
        )
        .is_err());
    }
}

#[test]
fn graph_error_categories_and_child_skip_work_precedence_are_frozen() {
    assert!(matches!(
        with_graph(
            "SecMarker X\nSecMarker X\nSecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\n",
            |_| ()
        ),
        Err(SemanticError::DanglingChain { .. })
    ));
    assert!(matches!(
        with_graph(
            "SecAction \"id:1,pass,phase:1,skipAfter:MISSING\"\nSecMarker X\nSecMarker X\n",
            |_| ()
        ),
        Err(SemanticError::DuplicateMarker { .. })
    ));
    let source = scan_source(
            "child-work",
            b"SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,chain\"\nSecRule ARGS \"@rx y\" \"skipAfter:END\"\nSecMarker END\n",
            SourceLimits::default(),
        ).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(matches!(
        analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                graph: GraphLimits {
                    max_skip_after_refs: 0,
                    max_registry_work_bytes: MARKER_WORK_ENTRY_CHARGE + 3,
                    ..GraphLimits::default()
                },
                ..SemanticLimits::default()
            }
        ),
        Err(SemanticError::ChildAction {
            kind: ActionKind::SkipAfter,
            ..
        })
    ));
}

#[test]
fn graph_owned_and_retained_charges_have_exact_boundaries() {
    let source = scan_source(
        "graph-charge",
        b"SecAction \"id:1,pass,phase:1\"\nSecMarker END\n",
        SourceLimits::default(),
    )
    .unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let inherited = syntax.retained_charged_bytes();
    let exact = analyze_one_semantic_draft(syntax, SemanticLimits::default())
        .unwrap()
        .semantic_charged_bytes();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(analyze_one_semantic_draft(
        syntax,
        SemanticLimits {
            max_owned_bytes: exact,
            max_retained_bytes: inherited + exact,
            ..SemanticLimits::default()
        }
    )
    .is_ok());
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(analyze_one_semantic_draft(
        syntax,
        SemanticLimits {
            max_owned_bytes: exact - 1,
            max_retained_bytes: usize::MAX,
            ..SemanticLimits::default()
        }
    )
    .is_err());
}

#[test]
fn fold_limits_accept_exact_and_reject_first_excess() {
    let source = scan_source(
        "limits",
        b"SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,t:none,t:lowercase\"\n",
        SourceLimits::default(),
    )
    .unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(analyze_one_semantic_draft(
        syntax,
        SemanticLimits {
            fold: FoldLimits {
                max_folded_rules: 1,
                max_effective_transformations: 1
            },
            ..SemanticLimits::default()
        }
    )
    .is_ok());
    for fold in [
        FoldLimits {
            max_folded_rules: 0,
            max_effective_transformations: 1,
        },
        FoldLimits {
            max_folded_rules: 1,
            max_effective_transformations: 0,
        },
    ] {
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                fold,
                ..SemanticLimits::default()
            }
        )
        .is_err());
    }
}

#[test]
fn transformation_reset_fold_matches_suffix_model() {
    let choices = ["t:none", "t:lowercase", "t:length"];
    for mut model in 0_usize..243 {
        let mut entries = Vec::new();
        let mut expected = Vec::new();
        for _ in 0..5 {
            let choice = choices[model % choices.len()];
            model /= choices.len();
            entries.push(choice);
            match choice {
                "t:none" => expected.clear(),
                "t:lowercase" => expected.push(TransformationKind::Lowercase),
                _ => expected.push(TransformationKind::Length),
            }
        }
        let source = format!("SecRule ARGS \"@rx x\" \"id:1,pass,phase:1,{}\"\n", entries.join(","));
        let actual = with_fold(&source, |draft| {
            draft
                .folded_rules()
                .next()
                .unwrap()
                .effective_transformations()
                .map(|action| action.transformation().unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn every_fold_singleton_and_group_rejects_its_second_member() {
    for duplicate in [
        "id:2",
        "phase:2",
        "severity:warning",
        "status:404",
        "msg:b",
        "logdata:b",
        "ver:b",
        "skipAfter:END",
        "chain",
        "capture",
        "multiMatch",
        "deny",
    ] {
        let source = format!(
                "SecRule ARGS \"@rx x\" \"id:1,phase:1,severity:error,status:403,msg:a,logdata:a,ver:a,skip:1,chain,capture,multiMatch,block,{duplicate}\"\n"
            );
        assert!(matches!(
            with_fold(&source, |_| ()),
            Err(SemanticError::FoldDuplicate { .. })
        ));
    }
    assert!(with_fold(
            "SecAction \"id:1,pass,phase:1,tag:a,tag:b,setvar:TX.a=1,setvar:TX.b=2,log,nolog,auditlog,noauditlog,t:none,t:none\"\n",
            |_| ()
        )
        .is_ok());
}

#[test]
fn fold_owned_and_retained_charges_have_exact_boundaries() {
    let text = b"SecAction \"id:1,block\"\n";
    let source = scan_source("charge", text, SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let inherited = syntax.retained_charged_bytes();
    let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).unwrap();
    assert_eq!(
        draft
            .requirements()
            .map(|record| record.requirement())
            .collect::<Vec<_>>(),
        [
            CompileRequirement::BlockDefaultDisruptive,
            CompileRequirement::DefaultPhase
        ]
    );
    let exact = draft.semantic_charged_bytes();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(analyze_one_semantic_draft(
        syntax,
        SemanticLimits {
            max_owned_bytes: exact,
            max_retained_bytes: inherited + exact,
            ..SemanticLimits::default()
        }
    )
    .is_ok());
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(analyze_one_semantic_draft(
        syntax,
        SemanticLimits {
            max_owned_bytes: exact - 1,
            max_retained_bytes: usize::MAX,
            ..SemanticLimits::default()
        }
    )
    .is_err());
}

#[test]
fn every_ordinary_action_obeys_its_value_presence_contract() {
    let forbidden = [
        ActionKind::AuditLog,
        ActionKind::Block,
        ActionKind::Capture,
        ActionKind::Chain,
        ActionKind::Deny,
        ActionKind::Log,
        ActionKind::MultiMatch,
        ActionKind::NoAuditLog,
        ActionKind::NoLog,
        ActionKind::Pass,
    ];
    for kind in forbidden {
        assert!(with_draft(&format!("SecAction \"{}\"\n", kind.as_str()), |_| ()).is_ok());
        assert!(matches!(
            with_draft(&format!("SecAction \"{}:x\"\n", kind.as_str()), |_| ()),
            Err(SemanticError::ValueContract {
                expected: ValueContract::Forbidden,
                ..
            })
        ));
    }
    let required = [
        (ActionKind::Id, "1"),
        (ActionKind::Phase, "2"),
        (ActionKind::Severity, "3"),
        (ActionKind::Status, "403"),
        (ActionKind::Skip, "1"),
        (ActionKind::Ctl, "auditEngine=Off"),
        (ActionKind::InitCol, "global=key"),
        (ActionKind::LogData, "x"),
        (ActionKind::Msg, "x"),
        (ActionKind::SetVar, "TX.key=1"),
        (ActionKind::SkipAfter, "x"),
        (ActionKind::Tag, "x"),
        (ActionKind::Ver, "x"),
    ];
    for (kind, value) in required {
        assert!(with_draft(&format!("SecAction \"{}:{}\"\n", kind.as_str(), value), |_| ()).is_ok());
        assert!(matches!(
            with_draft(&format!("SecAction \"{}\"\n", kind.as_str()), |_| ()),
            Err(SemanticError::ValueContract {
                expected: ValueContract::Required,
                ..
            })
        ));
        assert!(matches!(
            with_draft(&format!("SecAction \"{}:\"\n", kind.as_str()), |_| ()),
            Err(SemanticError::ValueContract {
                expected: ValueContract::Required,
                ..
            })
        ));
    }
}

#[test]
fn every_action_operand_is_typed_and_owner_bound() {
    let source = concat!(
        "SecAction \"auditlog\"\n",
        "SecAction \"id:7\"\n",
        "SecAction \"t:lowercase\"\n",
        "SecAction \"msg:prefix%{TX.value}suffix\"\n",
        "SecAction \"tag:%{BROKEN\"\n",
        "SecAction \"setvar:TX.pre%{TX.key}post=+%{TX.delta}\"\n",
        "SecAction \"initcol:ip=%{REMOTE_ADDR}\"\n",
        "SecAction \"ctl:auditEngine=RelevantOnly\"\n",
    );
    with_draft(source, |draft| {
        let mut actions = draft.actions();
        assert!(matches!(
            actions.next().unwrap().operand(),
            ActionOperandView::NoArgument
        ));
        assert!(matches!(
            actions.next().unwrap().operand(),
            ActionOperandView::Local(LocalActionValue::Id(7))
        ));
        assert!(matches!(
            actions.next().unwrap().operand(),
            ActionOperandView::Transformation(TransformationKind::Lowercase)
        ));
        let ActionOperandView::Template(message) = actions.next().unwrap().operand() else {
            panic!("message template")
        };
        assert_eq!(
            (message.text(), message.expansion_count()),
            ("prefix%{TX.value}suffix", 1)
        );
        assert!(matches!(
            actions.next().unwrap().operand(),
            ActionOperandView::Literal(value) if value.text() == "%{BROKEN"
        ));
        let ActionOperandView::SetVar(setvar) = actions.next().unwrap().operand() else {
            panic!("setvar")
        };
        assert_eq!(
            (
                setvar.collection(),
                setvar.operation(),
                setvar.key().text(),
                setvar.constant()
            ),
            (CollectionKind::Tx, SetVarOperation::Add, "pre%{TX.key}post", None)
        );
        assert_eq!(
            setvar
                .key()
                .segments()
                .map(|segment| segment.text())
                .collect::<String>(),
            setvar.key().text()
        );
        assert_eq!(setvar.key().segments().len(), 3);
        let value = setvar.value().unwrap();
        assert_eq!((value.text(), value.expansion_count()), ("%{TX.delta}", 1));
        assert_eq!(
            value.segments().map(|segment| segment.text()).collect::<String>(),
            value.text()
        );
        let ActionOperandView::InitCol(initcol) = actions.next().unwrap().operand() else {
            panic!("initcol")
        };
        assert_eq!(
            (initcol.collection(), initcol.key().text()),
            (InitColCollection::Ip, "%{REMOTE_ADDR}")
        );
        assert!(matches!(
            actions.next().unwrap().operand(),
            ActionOperandView::Ctl(CtlView::AuditEngine(AuditEngineMode::RelevantOnly))
        ));
        assert!(actions.next().is_none());
    })
    .unwrap();
    with_draft("SecAction \"ctl:ruleRemoveTargetByTag=tag;ARGS:/^name$/\"\n", |draft| {
        let ActionOperandView::Ctl(CtlView::RuleRemoveTargetByTag { target, .. }) =
            draft.actions().next().unwrap().operand()
        else {
            panic!("regex ctl target")
        };
        assert_eq!(
            target.selector_kind(),
            SelectorKind::Regex(crate::seclang::RegexStyle::Bare)
        );
        assert_eq!(target.regex_body().unwrap().text(), "^name$");
        assert!(draft.requirements().next().is_none());
        assert_eq!(draft.unavailable_capabilities().len(), 0);
    })
    .unwrap();
}

#[test]
fn setvar_operations_arithmetic_and_first_equals_are_exact() {
    let source = concat!(
        "SecAction \"setvar:TX.assign=a=b\"\n",
        "SecAction \"setvar:TX.add=+9223372036854775807\"\n",
        "SecAction \"setvar:TX.subtract=-5\"\n",
        "SecAction \"setvar:!TX.%{TX.key}\"\n",
    );
    let values = with_draft(source, |draft| {
        draft
            .actions()
            .map(|action| {
                let ActionOperandView::SetVar(setvar) = action.operand() else {
                    panic!("setvar")
                };
                (
                    setvar.operation(),
                    setvar.key().text().to_owned(),
                    setvar.value().map(|value| value.text().to_owned()),
                    setvar.constant(),
                )
            })
            .collect::<Vec<_>>()
    })
    .unwrap();
    assert_eq!(
        values,
        [
            (
                SetVarOperation::Assign,
                "assign".to_owned(),
                Some("a=b".to_owned()),
                None
            ),
            (
                SetVarOperation::Add,
                "add".to_owned(),
                Some("9223372036854775807".to_owned()),
                Some(i64::MAX),
            ),
            (
                SetVarOperation::Subtract,
                "subtract".to_owned(),
                Some("5".to_owned()),
                Some(5)
            ),
            (SetVarOperation::Delete, "%{TX.key}".to_owned(), None, None),
        ]
    );
    for source in [
        "SecAction \"setvar:TX.key\"\n",
        "SecAction \"setvar:IP.key=1\"\n",
        "SecAction \"setvar:!TX.key=1\"\n",
        "SecAction \"setvar:TX.=1\"\n",
        "SecAction \"setvar:TX.key=+\"\n",
    ] {
        assert!(
            matches!(with_draft(source, |_| ()), Err(SemanticError::SetVarSyntax { .. })),
            "{source}"
        );
    }
    let overflow_source = "SecAction \"setvar:TX.key=+9223372036854775808\"\n";
    let number_offset = overflow_source.find("922").unwrap();
    assert!(matches!(
        with_draft(overflow_source, |_| ()),
        Err(SemanticError::IntegerOperand {
            kind: IntegerOperandErrorKind::Overflow,
            position: SourcePosition { line: 1, column, byte_offset },
        }) if byte_offset == number_offset && column == number_offset + 1
    ));
}

#[test]
fn initcol_and_ctl_commands_keep_only_unsupported_actions_unavailable() {
    let source = concat!(
        "SecAction \"initcol:GLOBAL=key\"\n",
        "SecAction \"ctl:auditEngine=oN\"\n",
        "SecAction \"ctl:forceRequestBodyVariable=off\"\n",
        "SecAction \"ctl:requestBodyProcessor=json\"\n",
        "SecAction \"ctl:ruleRemoveByTag=criterion\"\n",
        "SecAction \"ctl:ruleRemoveTargetByTag=tag;XML:/root/node\"\n",
    );
    with_draft(source, |draft| {
            assert_eq!(draft.structured_action_count(), 6);
            assert_eq!(draft.ctl_target_count(), 1);
            let operands = draft.actions().map(|action| action.operand()).collect::<Vec<_>>();
            assert!(matches!(operands[0], ActionOperandView::InitCol(_)));
            assert!(matches!(operands[1], ActionOperandView::Ctl(CtlView::AuditEngine(AuditEngineMode::On))));
            assert!(matches!(
                operands[2],
                ActionOperandView::Ctl(CtlView::ForceRequestBodyVariable(OnOff::Off))
            ));
            assert!(matches!(
                operands[3],
                ActionOperandView::Ctl(CtlView::RequestBodyProcessor(RequestBodyProcessor::Json))
            ));
            assert!(matches!(operands[4], ActionOperandView::Ctl(CtlView::RuleRemoveByTag(value)) if value.text() == "criterion"));
            let ActionOperandView::Ctl(CtlView::RuleRemoveTargetByTag { tag, target }) = operands[5] else {
                panic!("target ctl")
            };
            assert_eq!(tag.text(), "tag");
            assert_eq!(target.collection(), CollectionKind::Xml);
            assert_eq!(target.selector_kind(), SelectorKind::Path);
            assert_eq!(target.selector().unwrap().text(), "/root/node");
            assert_eq!(
                draft
                    .unavailable_capabilities()
                    .map(|record| record.capability())
                    .collect::<Vec<_>>(),
                [
                    UnavailableCapability::InitCol,
                    UnavailableCapability::Ctl,
                ]
            );
        })
        .unwrap();
}

#[test]
fn structured_action_grammars_reject_upstream_coercions_and_spacing() {
    for source in [
        "SecAction \"initcol:resource=key\"\n",
        "SecAction \"initcol:global=\"\n",
        "SecAction \"ctl:AuditEngine=On\"\n",
        "SecAction \"ctl:ruleEngine=Off\"\n",
        "SecAction \"ctl:requestBodyProcessor=text\"\n",
        "SecAction \"ctl:'auditEngine =On'\"\n",
        "SecAction \"ctl:'auditEngine= On'\"\n",
        "SecAction \"ctl:ruleRemoveByTag=a;b\"\n",
        "SecAction \"ctl:ruleRemoveTargetByTag=tagARGS\"\n",
        "SecAction \"ctl:ruleRemoveTargetByTag=tag;ARGS;TX\"\n",
        "SecAction \"ctl:'ruleRemoveTargetByTag=tag;ARGS '\"\n",
        "SecAction \"ctl:ruleRemoveTargetByTag=tag;!ARGS\"\n",
        "SecAction \"ctl:ruleRemoveTargetByTag=tag;ARGS|TX\"\n",
        "SecAction \"ctl:ruleRemoveTargetByTag=tag;UNKNOWN\"\n",
    ] {
        assert!(with_draft(source, |_| ()).is_err(), "{source}");
    }
    let error = with_draft("SecAction \"ctl:ruleRemoveTargetByTag=private;UNKNOWN\"\n", |_| ()).unwrap_err();
    let mut source = std::error::Error::source(&error);
    let rendered = format!("{error:?} {error}").to_ascii_lowercase();
    assert!(!rendered.contains("private") && !rendered.contains("unknown"));
    while let Some(cause) = source {
        let rendered = format!("{cause:?} {cause}").to_ascii_lowercase();
        assert!(!rendered.contains("private") && !rendered.contains("unknown"));
        source = cause.source();
    }
}

#[test]
fn structured_action_limits_are_exact_and_slices_keep_continuation_provenance() {
    let source = "SecAction \"setvar:TX.é%{TX.key}=left\\\nright\"\nSecAction \"ctl:ruleRemoveTargetByTag=tag;REQUEST_FILENAME\"\n";
    let exact = OperandLimits {
        max_structured_actions: 2,
        max_ctl_targets: 1,
        max_setvar_key_bytes: 11,
        max_ctl_value_bytes: 20,
        ..OperandLimits::default()
    };
    with_draft_limits(
        source,
        SemanticLimits {
            operands: exact,
            ..SemanticLimits::default()
        },
        |draft| {
            let ActionOperandView::SetVar(setvar) = draft.actions().next().unwrap().operand() else {
                panic!("setvar")
            };
            assert_eq!(setvar.key().text(), "é%{TX.key}");
            assert_eq!(setvar.value().unwrap().text(), "leftright");
            assert_eq!(setvar.value().unwrap().span().start.line, 1);
            assert_eq!(setvar.value().unwrap().span().end.line, 2);
        },
    )
    .unwrap();
    for operands in [
        OperandLimits {
            max_structured_actions: 1,
            ..exact
        },
        OperandLimits {
            max_ctl_targets: 0,
            ..exact
        },
        OperandLimits {
            max_setvar_key_bytes: 10,
            ..exact
        },
        OperandLimits {
            max_ctl_value_bytes: 19,
            ..exact
        },
    ] {
        assert!(with_draft_limits(
            source,
            SemanticLimits {
                operands,
                ..SemanticLimits::default()
            },
            |_| ()
        )
        .is_err());
    }

    let owned_source = scan_source(
        "structured-owned",
        b"SecAction \"setvar:TX.key=%{TX.value}\"\n",
        SourceLimits::default(),
    )
    .unwrap();
    let inherited = parse_syntax(&owned_source, SyntaxLimits::default())
        .unwrap()
        .retained_charged_bytes();
    let exact_owned = with_draft("SecAction \"setvar:TX.key=%{TX.value}\"\n", |draft| {
        draft.semantic_charged_bytes()
    })
    .unwrap();
    let syntax = parse_syntax(&owned_source, SyntaxLimits::default()).unwrap();
    assert!(analyze_semantic_draft_inner(
        vec![syntax],
        SemanticLimits {
            max_owned_bytes: exact_owned,
            max_retained_bytes: inherited + exact_owned,
            ..SemanticLimits::default()
        },
        false,
        false,
    )
    .is_ok());
    let syntax = parse_syntax(&owned_source, SyntaxLimits::default()).unwrap();
    assert!(matches!(
        analyze_semantic_draft_inner(
            vec![syntax],
            SemanticLimits {
                max_owned_bytes: exact_owned - 1,
                max_retained_bytes: inherited + exact_owned,
                ..SemanticLimits::default()
            },
            false,
            false,
        ),
        Err(SemanticError::OwnedByteLimit { .. } | SemanticError::PreflightOwnedByteLimit { .. })
    ));
}

#[test]
fn generated_setvar_template_slices_reconstruct_exactly() {
    for index in 0..128usize {
        let key = format!("key{index}%{{TX.suffix}}tail");
        let value = format!("left%{{TX.value}}right={index}");
        let source = format!("SecAction \"setvar:TX.{key}={value}\"\n");
        with_draft(&source, |draft| {
            let ActionOperandView::SetVar(setvar) = draft.actions().next().unwrap().operand() else {
                panic!("generated setvar")
            };
            assert_eq!(setvar.operation(), SetVarOperation::Assign);
            assert_eq!(setvar.key().text(), key);
            assert_eq!(setvar.value().unwrap().text(), value);
            for slice in [setvar.key(), setvar.value().unwrap()] {
                assert_eq!(
                    slice.segments().map(|segment| segment.text()).collect::<String>(),
                    slice.text()
                );
                assert_eq!(slice.expansion_count(), 1);
            }
        })
        .unwrap();
    }
}

#[test]
fn templates_parse_exact_sources_keys_adjacency_and_literal_percent_forms() {
    let result = with_draft(
            "SecAction \"msg:%literal} %{tX.Key}%{ARGS.name}%{REQUEST_HEADERS.Content-Type}%{MATCHED_VAR}%{MATCHED_VAR_NAME}%{REMOTE_ADDR}%{REQUEST_LINE}\"\n",
            |draft| {
                let template = draft.actions().next().unwrap().template().unwrap();
                let segments = template.segments().map(|segment| {
                    let expansion = match segment.kind() {
                        TemplateSegmentKind::Literal => None,
                        TemplateSegmentKind::Expansion(value) => Some((value.source(), value.source_spelling().text().to_owned(), value.key().map(|key| key.text().to_owned()))),
                    };
                    (segment.text().to_owned(), expansion)
                }).collect::<Vec<_>>();
                (template.text().to_owned(), template.expansion_count(), segments)
            },
        ).unwrap();
    assert_eq!(result.1, 7);
    assert_eq!(result.2.len(), 8);
    assert_eq!(result.2[0].0, "%literal} ");
    assert_eq!(
        result.2[1].1,
        Some((MacroSource::Tx, "tX".to_owned(), Some("Key".to_owned())))
    );
    assert!(result.0.ends_with("%{REQUEST_LINE}"));

    let adjacent = with_draft("SecAction \"msg:%{TX.a}%{ARGS.b}\"\n", |draft| {
        let template = draft.actions().next().unwrap().template().unwrap();
        (template.segments().len(), template.expansion_count())
    })
    .unwrap();
    assert_eq!(adjacent, (2, 2));
    assert!(matches!(
        with_draft("SecRule ARGS \"@eq\" \"id:1\"\n", |_| ()),
        Err(SemanticError::OperatorOperandPresence {
            expected: OperatorOperandContract::NonEmpty,
            ..
        })
    ));
}

#[test]
fn malformed_macro_errors_are_typed_directional_and_redacted() {
    let cases = [
        ("%{TX.x", MacroErrorKind::Unclosed),
        ("%{}", MacroErrorKind::EmptyBody),
        ("%{TX.%{ARGS.x}}", MacroErrorKind::Nested),
        ("%{TX.x!}", MacroErrorKind::InvalidCharacter),
        ("%{SECRET.x}", MacroErrorKind::UnknownSource),
        ("%{TX}", MacroErrorKind::MissingKey),
        ("%{REMOTE_ADDR.x}", MacroErrorKind::ForbiddenKey),
    ];
    for (value, kind) in cases {
        let source = format!("SecAction \"msg:prefix{value}\"\n");
        let error = with_draft(&source, |_| ()).unwrap_err();
        assert!(matches!(error, SemanticError::MacroSyntax { kind: actual, .. } if actual == kind));
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("SECRET") && !rendered.contains("prefix"));
        let mut cause = std::error::Error::source(&error);
        while let Some(source) = cause {
            assert!(!format!("{source:?} {source}").contains("SECRET"));
            cause = source.source();
        }
    }
}

#[test]
fn macro_capability_matrix_is_exact_and_static_fields_keep_macro_text_literal() {
    let mut source = String::new();
    for (index, &kind) in OperatorKind::ALL.iter().enumerate() {
        let operand = if operator_macro_capable(kind) {
            if matches!(
                kind,
                OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt
            ) {
                "1"
            } else {
                "literal"
            }
        } else if matches!(
            kind,
            OperatorKind::DetectSqli
                | OperatorKind::DetectXss
                | OperatorKind::UnconditionalMatch
                | OperatorKind::ValidateUrlEncoding
                | OperatorKind::ValidateUtf8Encoding
        ) {
            ""
        } else {
            valid_operator_argument(kind)
        };
        source.push_str(&format!(
            "SecRule ARGS \"@{} {operand}\" \"id:{}\"\n",
            kind.as_str(),
            index + 1
        ));
    }
    for &kind in ActionKind::ALL {
        let entry = match kind {
            ActionKind::AuditLog
            | ActionKind::Block
            | ActionKind::Capture
            | ActionKind::Chain
            | ActionKind::Deny
            | ActionKind::Log
            | ActionKind::MultiMatch
            | ActionKind::NoAuditLog
            | ActionKind::NoLog
            | ActionKind::Pass => kind.as_str().to_owned(),
            ActionKind::Id => "id:1".to_owned(),
            ActionKind::Phase => "phase:2".to_owned(),
            ActionKind::Severity => "severity:3".to_owned(),
            ActionKind::Status => "status:403".to_owned(),
            ActionKind::Skip => "skip:1".to_owned(),
            ActionKind::SetVar => "setvar:TX.key=literal".to_owned(),
            ActionKind::InitCol => "initcol:global=literal".to_owned(),
            ActionKind::Msg | ActionKind::LogData => format!("{}:literal", kind.as_str()),
            ActionKind::Ctl => "ctl:auditEngine=Off".to_owned(),
            _ => format!("{}:%{{BROKEN", kind.as_str()),
        };
        source.push_str(&format!("SecAction \"{entry}\"\n"));
    }
    let (operators, actions) = with_draft(&source, |draft| {
        (
            draft
                .operators()
                .map(|operator| (operator.kind(), operator.template().is_some()))
                .collect::<Vec<_>>(),
            draft
                .actions()
                .skip(OperatorKind::ALL.len())
                .map(|action| (action.kind(), action.template().is_some()))
                .collect::<Vec<_>>(),
        )
    })
    .unwrap();
    assert_eq!(operators.len(), OperatorKind::ALL.len());
    for ((kind, has_template), expected) in operators.into_iter().zip(OperatorKind::ALL) {
        assert_eq!(kind, *expected);
        assert_eq!(has_template, operator_macro_capable(*expected), "{expected:?}");
    }
    assert_eq!(actions.len(), ActionKind::ALL.len());
    for ((kind, has_template), expected) in actions.into_iter().zip(ActionKind::ALL) {
        assert_eq!(kind, ActionEnvelopeKind::Action(*expected));
        assert_eq!(has_template, action_macro_capable(*expected), "{expected:?}");
    }
}

#[test]
fn every_operator_contract_publishes_one_typed_owner_bound_operand() {
    let source = concat!(
        "SecRule ARGS \"@detectSQLi\" \"id:1\"\n",
        "SecRule ARGS \"\" \"id:2\"\n",
        "SecRule ARGS \"@contains needle\" \"id:3\"\n",
        "SecRule ARGS \"@eq +001\" \"id:4\"\n",
        "SecRule ARGS \"@pm alpha\tbeta alpha\" \"id:5\"\n",
        "SecRule ARGS \"@pmFromFile rules/a.data rules/b.data\" \"id:6\"\n",
        "SecRule ARGS \"@ipMatch 192.0.2.129/24,2001:db8::1/64\" \"id:7\"\n",
        "SecRule ARGS \"@validateByteRange 0, 2-4,4\" \"id:8\"\n",
    );
    with_draft(source, |draft| {
        let mut operators = draft.operators();
        assert!(matches!(
            operators.next().unwrap().operand(),
            OperatorOperandView::NoArgument
        ));
        let OperatorOperandView::Regex(regex) = operators.next().unwrap().operand() else {
            panic!("implicit rx operand")
        };
        assert_eq!(regex.text(), "");
        let OperatorOperandView::StringTemplate(template) = operators.next().unwrap().operand() else {
            panic!("string template operand")
        };
        assert_eq!(template.text(), "needle");
        let OperatorOperandView::IntegerTemplate { template, constant } = operators.next().unwrap().operand() else {
            panic!("integer template operand")
        };
        assert_eq!((template.text(), constant), ("+001", Some(1)));
        let OperatorOperandView::PhraseList(phrases) = operators.next().unwrap().operand() else {
            panic!("phrase list operand")
        };
        assert_eq!(phrases.text().text(), "alpha\tbeta alpha");
        assert_eq!(
            phrases.phrases().map(|phrase| phrase.text()).collect::<Vec<_>>(),
            ["alpha", "beta", "alpha"]
        );
        let OperatorOperandView::ExternalAssets(assets) = operators.next().unwrap().operand() else {
            panic!("external assets operand")
        };
        assert_eq!(assets.text().text(), "rules/a.data rules/b.data");
        assert_eq!(
            assets
                .assets()
                .map(|asset| (asset.document_index(), asset.handle()))
                .collect::<Vec<_>>(),
            [(0, "rules/a.data"), (0, "rules/b.data")]
        );
        let OperatorOperandView::IpNetworks(networks) = operators.next().unwrap().operand() else {
            panic!("IP network operand")
        };
        assert_eq!(networks.text().text(), "192.0.2.129/24,2001:db8::1/64");
        assert_eq!(
            networks
                .networks()
                .map(|network| (network.address(), network.prefix()))
                .collect::<Vec<_>>(),
            [("192.0.2.0".parse().unwrap(), 24), ("2001:db8::".parse().unwrap(), 64),]
        );
        let OperatorOperandView::ByteSet(bytes) = operators.next().unwrap().operand() else {
            panic!("byte-set operand")
        };
        assert_eq!(bytes.text().text(), "0, 2-4,4");
        assert_eq!(bytes.member_count(), 3);
        assert!(bytes.contains(0) && bytes.contains(2) && bytes.contains(3) && bytes.contains(4));
        assert!(!bytes.contains(1) && !bytes.contains(5));
        assert!(operators.next().is_none());
    })
    .unwrap();
}

#[test]
fn strict_integer_templates_accept_i64_edges_and_reject_coercions() {
    for (value, expected) in [
        ("+000", 0),
        ("9223372036854775807", i64::MAX),
        ("-9223372036854775808", i64::MIN),
    ] {
        let source = format!("SecRule ARGS \"@eq {value}\" \"id:1\"\n");
        let actual = with_draft(&source, |draft| {
            let OperatorOperandView::IntegerTemplate { constant, .. } = draft.operators().next().unwrap().operand()
            else {
                panic!("integer operand")
            };
            constant
        });
        assert_eq!(actual, Ok(Some(expected)));
    }
    let macro_value = with_draft("SecRule ARGS \"@eq %{TX.value}\" \"id:1\"\n", |draft| {
        let OperatorOperandView::IntegerTemplate { constant, .. } = draft.operators().next().unwrap().operand() else {
            panic!("integer operand")
        };
        constant
    });
    assert_eq!(macro_value, Ok(None));
    for (value, kind) in [
        ("+", IntegerOperandErrorKind::Invalid),
        ("1 2", IntegerOperandErrorKind::Invalid),
        ("1x", IntegerOperandErrorKind::Invalid),
        ("9223372036854775808", IntegerOperandErrorKind::Overflow),
        ("-9223372036854775809", IntegerOperandErrorKind::Overflow),
    ] {
        let source = format!("SecRule ARGS \"@eq {value}\" \"id:1\"\n");
        assert!(matches!(
            with_draft(&source, |_| ()),
            Err(SemanticError::IntegerOperand { kind: actual, .. }) if actual == kind
        ));
    }
}

#[test]
fn phrase_asset_ip_and_byte_grammars_fail_closed_without_echoing_content() {
    let cases = [
        ("SecRule ARGS \"@pm safe|private\" \"id:1\"\n", "PhraseSyntax"),
        ("SecRule ARGS \"@pmFromFile /private.data\" \"id:1\"\n", "UnsafeAsset"),
        ("SecRule ARGS \"@pmFromFile ../private.data\" \"id:1\"\n", "UnsafeAsset"),
        ("SecRule ARGS \"@pmFromFile https:private\" \"id:1\"\n", "UnsafeAsset"),
        (
            "SecRule ARGS \"@ipMatch private.example\" \"id:1\"\n",
            "IpNetworkSyntax",
        ),
        ("SecRule ARGS \"@ipMatch 192.0.2.1/33\" \"id:1\"\n", "IpNetworkSyntax"),
        ("SecRule ARGS \"@validateByteRange 9-1\" \"id:1\"\n", "ByteSetSyntax"),
        ("SecRule ARGS \"@validateByteRange 256\" \"id:1\"\n", "ByteSetSyntax"),
    ];
    for (source, family) in cases {
        let error = with_draft(source, |_| ()).unwrap_err();
        assert!(format!("{error:?}").contains(family));
        let rendered = format!("{error:?} {error}").to_ascii_lowercase();
        assert!(!rendered.contains("private") && !rendered.contains("example"));
    }

    let phrase = with_draft("SecRule ARGS \"@pm ab|cd\" \"id:1\"\n", |_| ()).unwrap_err();
    assert!(matches!(
        phrase,
        SemanticError::PhraseSyntax {
            position: SourcePosition {
                line: 1,
                column: 21,
                byte_offset: 20,
            }
        }
    ));
    let integer = with_draft("SecRule ARGS \"@eq 12x\" \"id:1\"\n", |_| ()).unwrap_err();
    assert!(matches!(
        integer,
        SemanticError::IntegerOperand {
            position: SourcePosition {
                line: 1,
                column: 21,
                byte_offset: 20,
            },
            ..
        }
    ));
}

#[test]
fn captured_upstream_operand_divergences_remain_explicit() {
    let cases = [
        // Edgion and ModSecurity accept multiple handles; Coraza treats this as one raw path.
        ("SecRule ARGS \"@pmFromFile a.data b.data\" \"id:1\"\n", true),
        // ModSecurity can fetch HTTPS assets; this parsing profile deliberately forbids schemes and IO.
        ("SecRule ARGS \"@pmFromFile https:data\" \"id:1\"\n", false),
        // Upstream phrase parsers support Snort hex forms; partial interpretation is forbidden here.
        ("SecRule ARGS \"@pm a|20|b\" \"id:1\"\n", false),
        // Coraza integer conversion can coerce invalid text to zero; this profile is strict.
        ("SecRule ARGS \"@eq invalid\" \"id:1\"\n", false),
    ];
    for (source, accepted) in cases {
        assert_eq!(with_draft(source, |_| ()).is_ok(), accepted, "{source}");
    }
}

#[test]
fn operator_list_limits_accept_exact_and_reject_first_excess() {
    let phrases = "SecRule ARGS \"@pm a bb\" \"id:1\"\n";
    let exact = OperandLimits {
        max_phrases_per_operator: 2,
        max_phrases: 2,
        max_phrase_bytes: 3,
        ..OperandLimits::default()
    };
    assert!(with_draft_limits(
        phrases,
        SemanticLimits {
            operands: exact,
            ..SemanticLimits::default()
        },
        |_| ()
    )
    .is_ok());
    for (limits, family) in [
        (
            OperandLimits {
                max_phrases_per_operator: 1,
                ..exact
            },
            "OperatorPhraseLimit",
        ),
        (
            OperandLimits {
                max_phrases: 1,
                ..exact
            },
            "PhraseLimit",
        ),
        (
            OperandLimits {
                max_phrase_bytes: 2,
                ..exact
            },
            "PhraseByteLimit",
        ),
    ] {
        let error = with_draft_limits(
            phrases,
            SemanticLimits {
                operands: limits,
                ..SemanticLimits::default()
            },
            |_| (),
        )
        .unwrap_err();
        assert!(format!("{error:?}").contains(family));
    }

    let assets = "SecRule ARGS \"@pmFromFile a b\" \"id:1\"\n";
    for limits in [
        OperandLimits {
            max_assets_per_operator: 1,
            ..OperandLimits::default()
        },
        OperandLimits {
            max_assets: 1,
            ..OperandLimits::default()
        },
        OperandLimits {
            max_asset_bytes: 0,
            ..OperandLimits::default()
        },
    ] {
        assert!(with_draft_limits(
            assets,
            SemanticLimits {
                operands: limits,
                ..SemanticLimits::default()
            },
            |_| ()
        )
        .is_err());
    }
    assert!(matches!(
        with_draft_limits(
            "SecRule ARGS \"@ipMatch 192.0.2.1,198.51.100.1\" \"id:1\"\n",
            SemanticLimits {
                operands: OperandLimits {
                    max_networks_per_operator: 1,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::OperatorNetworkLimit { limit: 1, .. })
    ));
    assert!(matches!(
        with_draft_limits(
            "SecRule ARGS \"@validateByteRange 1,2\" \"id:1\"\n",
            SemanticLimits {
                operands: OperandLimits {
                    max_byte_range_members: 1,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::ByteRangeMemberLimit { limit: 1, .. })
    ));

    assert!(matches!(
        with_draft_limits(
            "SecRule ARGS \"@ipMatch 192.0.2.1\" \"id:1\"\nSecRule ARGS \"@ipMatch 198.51.100.1\" \"id:2\"\n",
            SemanticLimits {
                operands: OperandLimits {
                    max_networks_per_operator: 1,
                    max_networks: 1,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::NetworkLimit { limit: 1, .. })
    ));
}

#[test]
fn external_asset_validation_visits_each_decoded_byte_once() {
    let handle = "a".repeat(OperandLimits::default().max_asset_bytes);
    let operand = std::iter::repeat_n(handle.as_str(), OperandLimits::default().max_assets_per_operator)
        .collect::<Vec<_>>()
        .join(" ");
    let shape = walk_asset_words(operand.bytes(), OperandLimits::default().max_asset_bytes, |_| {}).unwrap();
    assert_eq!(shape.count, OperandLimits::default().max_assets_per_operator);
    assert_eq!(
        shape.payload_bytes,
        OperandLimits::default().max_assets_per_operator * OperandLimits::default().max_asset_bytes
    );
    assert_eq!(shape.inspected_bytes, operand.len());

    let source = format!("SecRule ARGS \"@pmFromFile {operand}\" \"id:1\"\n");
    assert_eq!(
        with_draft(&source, |draft| draft.external_asset_count()),
        Ok(OperandLimits::default().max_assets_per_operator)
    );
}

#[test]
fn operator_requirements_and_b2_error_precedence_are_exact() {
    let requirements = with_draft(
        "SecRule ARGS \"@pm a\" \"id:1\"\nSecRule ARGS \"@pmFromFile a.data\" \"id:2\"\n",
        |draft| {
            draft
                .requirements()
                .map(|record| record.requirement())
                .collect::<Vec<_>>()
        },
    )
    .unwrap();
    assert_eq!(
        requirements,
        [
            CompileRequirement::PhraseMatcherCompilation,
            CompileRequirement::ExternalPhraseAsset,
            CompileRequirement::PhraseMatcherCompilation,
        ]
    );
    let constrained = SemanticLimits {
        operands: OperandLimits {
            max_templates: 0,
            max_segments: 0,
            max_expansions: 0,
            ..OperandLimits::default()
        },
        ..SemanticLimits::default()
    };
    assert!(matches!(
        with_draft_limits("SecRule ARGS \"@eq\" \"id:1\"\n", constrained, |_| ()),
        Err(SemanticError::OperatorOperandPresence { .. })
    ));
    assert!(matches!(
        with_draft_limits("SecRule ARGS \"@eq bad\" \"id:1\"\n", constrained, |_| ()),
        Err(SemanticError::IntegerOperand { .. })
    ));

    let count_first = SemanticLimits {
        operands: OperandLimits {
            max_assets_per_operator: 0,
            max_networks_per_operator: 0,
            ..OperandLimits::default()
        },
        ..SemanticLimits::default()
    };
    assert!(matches!(
        with_draft_limits(
            "SecRule ARGS \"@pmFromFile ../private.data safe.data\" \"id:1\"\n",
            count_first,
            |_| ()
        ),
        Err(SemanticError::UnsafeAsset { .. })
    ));
    assert!(matches!(
        with_draft_limits(
            "SecRule ARGS \"@pmFromFile too-long ../private.data\" \"id:1\"\n",
            SemanticLimits {
                operands: OperandLimits {
                    max_asset_bytes: 1,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::UnsafeAsset { .. })
    ));
    assert!(matches!(
        with_draft_limits(
            "SecRule ARGS \"@ipMatch invalid,192.0.2.1\" \"id:1\"\n",
            count_first,
            |_| ()
        ),
        Err(SemanticError::IpNetworkSyntax { .. })
    ));
}

#[test]
fn decoded_copy_limit_is_shared_by_b1_templates_and_b2_static_operands() {
    for source in [
        concat!(
            "SecRule ARGS \"@rx a\\\"b\" \"id:1\"\n",
            "SecRule ARGS \"@contains c\\\"d\" \"id:2\"\n",
        ),
        concat!(
            "SecRule ARGS \"@contains c\\\"d\" \"id:1\"\n",
            "SecRule ARGS \"@rx a\\\"b\" \"id:2\"\n",
        ),
    ] {
        assert_eq!(
            with_draft_limits(
                source,
                SemanticLimits {
                    operands: OperandLimits {
                        max_decoded_bytes: 6,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |draft| draft.decoded_operand_bytes()
            ),
            Ok(6)
        );
        assert!(matches!(
            with_draft_limits(
                source,
                SemanticLimits {
                    operands: OperandLimits {
                        max_decoded_bytes: 5,
                        ..OperandLimits::default()
                    },
                    ..SemanticLimits::default()
                },
                |_| ()
            ),
            Err(SemanticError::DecodedOperandByteLimit { limit: 5, .. })
        ));
    }
}

#[test]
fn escaped_utf8_continued_phrase_ranges_keep_exact_source_provenance() {
    with_draft("SecRule ARGS \"@pm é\\\nleft\\\"right tail\" \"id:1\"\n", |draft| {
        let OperatorOperandView::PhraseList(phrases) = draft.operators().next().unwrap().operand() else {
            panic!("phrase list")
        };
        let phrases = phrases.phrases().collect::<Vec<_>>();
        assert_eq!(
            phrases.iter().map(|phrase| phrase.text()).collect::<Vec<_>>(),
            ["éleft\"right", "tail"]
        );
        assert_eq!(phrases[0].span().start.line, 1);
        assert_eq!(phrases[0].span().end.line, 2);
        assert_eq!(phrases[1].span().start.line, 2);
    })
    .unwrap();
}

#[test]
fn template_expansion_and_arena_limits_are_exact_and_atomic() {
    let exact_value = "%{TX.x}".repeat(1_024);
    let exact = format!("SecAction \"msg:{exact_value}\"\n");
    assert_eq!(with_draft(&exact, |draft| draft.expansion_count()), Ok(1_024));
    let excess = format!("SecAction \"msg:{}\"\n", "%{TX.x}".repeat(1_025));
    assert!(matches!(
        with_draft(&excess, |_| ()),
        Err(SemanticError::TemplateExpansionLimit { limit: 1_024, .. })
    ));

    let literal = "SecAction \"msg:abc\"\n";
    for limits in [
        OperandLimits {
            max_templates: 0,
            ..OperandLimits::default()
        },
        OperandLimits {
            max_segments: 0,
            ..OperandLimits::default()
        },
        OperandLimits {
            max_operand_bytes: 2,
            ..OperandLimits::default()
        },
        OperandLimits {
            max_scanned_bytes: 2,
            ..OperandLimits::default()
        },
    ] {
        assert!(with_draft_limits(
            literal,
            SemanticLimits {
                operands: limits,
                ..SemanticLimits::default()
            },
            |_| ()
        )
        .is_err());
    }
    assert_eq!(
        with_draft(literal, |draft| (
            draft.template_count(),
            draft.template_segment_count()
        )),
        Ok((1, 1))
    );

    let segmented = "SecAction \"msg:a%{TX.x}b\"\n";
    assert!(with_draft_limits(
        segmented,
        SemanticLimits {
            operands: OperandLimits {
                max_segments: 3,
                max_expansions: 1,
                ..OperandLimits::default()
            },
            ..SemanticLimits::default()
        },
        |_| ()
    )
    .is_ok());
    assert!(matches!(
        with_draft_limits(
            segmented,
            SemanticLimits {
                operands: OperandLimits {
                    max_segments: 2,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::TemplateSegmentLimit { limit: 2, .. })
    ));

    let (owned, retained) = with_draft(segmented, |draft| {
        (draft.semantic_charged_bytes(), draft.retained_charged_bytes())
    })
    .unwrap();
    assert!(with_draft_limits(
        segmented,
        SemanticLimits {
            max_owned_bytes: owned,
            max_retained_bytes: retained,
            ..SemanticLimits::default()
        },
        |_| ()
    )
    .is_ok());
    assert!(matches!(
        with_draft_limits(
            segmented,
            SemanticLimits {
                max_owned_bytes: owned - 1,
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::OwnedByteLimit { .. })
    ));
    assert!(matches!(
        with_draft_limits(
            segmented,
            SemanticLimits {
                max_retained_bytes: retained - 1,
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::RetainedByteLimit { .. })
    ));

    let escaped = "SecRule ARGS \"@eq a\\\"b%{TX.key}\" \"id:1\"\n";
    assert!(with_draft_limits(
        escaped,
        SemanticLimits {
            operands: OperandLimits {
                max_decoded_bytes: 12,
                ..OperandLimits::default()
            },
            ..SemanticLimits::default()
        },
        |_| ()
    )
    .is_ok());
    assert!(matches!(
        with_draft_limits(
            escaped,
            SemanticLimits {
                operands: OperandLimits {
                    max_decoded_bytes: 11,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::DecodedOperandByteLimit { limit: 11, .. })
    ));

    let cumulative = "SecAction \"msg:abc\"\nSecAction \"msg:def\"\n";
    assert!(with_draft_limits(
        cumulative,
        SemanticLimits {
            operands: OperandLimits {
                max_scanned_bytes: 6,
                ..OperandLimits::default()
            },
            ..SemanticLimits::default()
        },
        |_| ()
    )
    .is_ok());
    assert!(matches!(
        with_draft_limits(
            cumulative,
            SemanticLimits {
                operands: OperandLimits {
                    max_scanned_bytes: 5,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::ScannedOperandByteLimit { limit: 5, .. })
    ));

    let cumulative_macros = "SecAction \"msg:%{TX.a}\"\nSecAction \"msg:%{TX.b}\"\n";
    assert!(with_draft_limits(
        cumulative_macros,
        SemanticLimits {
            operands: OperandLimits {
                max_templates: 2,
                max_expansions: 2,
                ..OperandLimits::default()
            },
            ..SemanticLimits::default()
        },
        |_| ()
    )
    .is_ok());
    assert!(matches!(
        with_draft_limits(
            cumulative_macros,
            SemanticLimits {
                operands: OperandLimits {
                    max_templates: 1,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::TemplateLimit { limit: 1, .. })
    ));
    assert!(matches!(
        with_draft_limits(
            cumulative_macros,
            SemanticLimits {
                operands: OperandLimits {
                    max_expansions: 1,
                    ..OperandLimits::default()
                },
                ..SemanticLimits::default()
            },
            |_| ()
        ),
        Err(SemanticError::ExpansionLimit { limit: 1, .. })
    ));
}

#[test]
fn decoded_operator_and_action_subranges_preserve_source_provenance() {
    let operator = with_draft("SecRule ARGS \"@eq a\\\"b%{TX.key}\" \"id:1\"\n", |draft| {
        let template = draft.operators().next().unwrap().template().unwrap();
        let segments = template
            .segments()
            .map(|segment| (segment.text().to_owned(), segment.span()))
            .collect::<Vec<_>>();
        (template.text().to_owned(), draft.decoded_operand_bytes(), segments)
    })
    .unwrap();
    assert_eq!((operator.0.as_str(), operator.1), ("a\"b%{TX.key}", 12));
    assert_eq!(operator.2[0].0, "a\"b");
    assert!(operator.2[0].1.end.column > operator.2[0].1.start.column + operator.2[0].0.len());

    let action = with_draft("SecAction \"msg:é\\\n%{TX.key}\"\n", |draft| {
        draft
            .actions()
            .next()
            .unwrap()
            .template()
            .unwrap()
            .segments()
            .map(|segment| (segment.text().to_owned(), segment.span()))
            .collect::<Vec<_>>()
    })
    .unwrap();
    assert_eq!(action.len(), 2);
    assert_eq!(action[0].0, "é");
    assert_eq!(action[1].1.start.line, 2);

    let continued_operator = with_draft("SecRule ARGS \"@eq é\\\n\\\"x%{TX.key}\" \"id:1\"\n", |draft| {
        draft
            .operators()
            .next()
            .unwrap()
            .template()
            .unwrap()
            .segments()
            .map(|segment| (segment.text().to_owned(), segment.span()))
            .collect::<Vec<_>>()
    })
    .unwrap();
    assert_eq!(continued_operator[0].0, "é\"x");
    assert_eq!(continued_operator[0].1.start.line, 1);
    assert_eq!(continued_operator[0].1.end.line, 2);
    assert_eq!(continued_operator[1].1.start.line, 2);
}

#[test]
fn long_literal_percent_input_remains_one_bounded_segment() {
    let value = "%x}".repeat(100_000);
    let source = format!("SecAction \"msg:{value}\"\n");
    let result = with_draft(&source, |draft| {
        let template = draft.actions().next().unwrap().template().unwrap();
        (
            template.segments().len(),
            template.expansion_count(),
            draft.scanned_operand_bytes(),
        )
    })
    .unwrap();
    assert_eq!(result, (1, 0, value.len()));
}

#[test]
fn bounded_generated_templates_reconstruct_exactly_without_empty_segments() {
    for mask in 0u16..256 {
        let mut value = String::new();
        for bit in 0..8 {
            if mask & (1 << bit) == 0 {
                value.push_str("lit%");
            } else {
                value.push_str("%{TX.k}");
            }
        }
        let source = format!("SecAction \"msg:{value}\"\n");
        let rebuilt = with_draft(&source, |draft| {
            let template = draft.actions().next().unwrap().template().unwrap();
            let segments = template
                .segments()
                .map(|segment| segment.text().to_owned())
                .collect::<Vec<_>>();
            (template.text().to_owned(), segments)
        })
        .unwrap();
        assert_eq!(rebuilt.0, value);
        assert!(rebuilt.1.iter().all(|segment| !segment.is_empty()));
        assert_eq!(rebuilt.1.concat(), value);
    }
}

#[test]
fn empty_and_multiple_documents_preserve_caller_order() {
    let empty = analyze_semantic_draft(Vec::new(), SemanticLimits::default()).expect("empty");
    assert_eq!((empty.document_count(), empty.directive_count()), (0, 0));
    let first = scan_source("first", b"SecAction \"id:1\"\n", SourceLimits::default()).unwrap();
    let second = scan_source("second", b"SecAction \"id:2\"\n", SourceLimits::default()).unwrap();
    let docs = vec![
        parse_syntax(&first, SyntaxLimits::default()).unwrap(),
        parse_syntax(&second, SyntaxLimits::default()).unwrap(),
    ];
    let draft = analyze_semantic_draft_inner(docs, SemanticLimits::default(), false, false).unwrap();
    assert_eq!(
        draft.actions().map(|action| action.local_value()).collect::<Vec<_>>(),
        [LocalActionValue::Id(1), LocalActionValue::Id(2)]
    );
    assert_eq!(
        draft.directives().map(|view| view.document_index()).collect::<Vec<_>>(),
        [0, 1]
    );
}

#[test]
fn preflight_limits_precede_content_and_error_is_redacted() {
    let source = scan_source(
        "redacted",
        b"SecAction \"secret_action:value\"\n",
        SourceLimits::default(),
    )
    .unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let limits = SemanticLimits {
        max_directives: 0,
        ..SemanticLimits::default()
    };
    assert!(matches!(
        analyze_one_semantic_draft(syntax, limits),
        Err(SemanticError::DirectiveLimit { limit: 0 })
    ));
    let error = with_draft("SecAction \"secret_action:value\"\n", |_| ()).unwrap_err();
    assert!(!format!("{error:?} {error}").contains("secret_action"));
}

#[test]
fn exact_owned_and_retained_limits_are_enforced() {
    let source = scan_source("limits", b"SecAction \"id:1\"\n", SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let inherited = syntax.retained_charged_bytes();
    let fixed = size_of::<SyntaxDocument<'_>>()
        + size_of::<DirectiveSemanticRecord>()
        + size_of::<SemanticActionRecord<'_>>()
        + size_of::<ActionOperandRecord<'_>>();
    let mut limits = SemanticLimits {
        max_owned_bytes: fixed,
        max_retained_bytes: inherited + fixed,
        ..SemanticLimits::default()
    };
    let draft = analyze_semantic_draft_inner(vec![syntax], limits, false, false).unwrap();
    assert_eq!(draft.semantic_charged_bytes(), fixed);
    let source = scan_source("limits", b"SecAction \"id:1\"\n", SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    limits.max_owned_bytes -= 1;
    assert!(matches!(
        analyze_semantic_draft_inner(vec![syntax], limits, false, false),
        Err(SemanticError::PreflightOwnedByteLimit { .. })
    ));
}

#[test]
fn document_rule_and_node_limits_accept_exact_and_reject_first_excess() {
    let empty_sources = [
        scan_source("a", b"", SourceLimits::default()).unwrap(),
        scan_source("b", b"", SourceLimits::default()).unwrap(),
        scan_source("c", b"", SourceLimits::default()).unwrap(),
    ];
    let documents = empty_sources[..2]
        .iter()
        .map(|source| parse_syntax(source, SyntaxLimits::default()).unwrap())
        .collect();
    assert!(analyze_semantic_draft(
        documents,
        SemanticLimits {
            max_documents: 2,
            ..SemanticLimits::default()
        }
    )
    .is_ok());
    let documents = empty_sources
        .iter()
        .map(|source| parse_syntax(source, SyntaxLimits::default()).unwrap())
        .collect();
    assert!(matches!(
        analyze_semantic_draft(
            documents,
            SemanticLimits {
                max_documents: 2,
                ..SemanticLimits::default()
            }
        ),
        Err(SemanticError::DocumentLimit { limit: 2 })
    ));

    let rules = scan_source(
        "rules",
        b"SecRule ARGS \"@eq 1\" \"id:1\"\nSecRule ARGS \"@eq 1\" \"id:2\"\n",
        SourceLimits::default(),
    )
    .unwrap();
    let syntax = parse_syntax(&rules, SyntaxLimits::default()).unwrap();
    assert!(analyze_semantic_draft_inner(
        vec![syntax],
        SemanticLimits {
            max_rules: 2,
            ..SemanticLimits::default()
        },
        false,
        false,
    )
    .is_ok());
    let syntax = parse_syntax(&rules, SyntaxLimits::default()).unwrap();
    assert!(matches!(
        analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                max_rules: 1,
                ..SemanticLimits::default()
            }
        ),
        Err(SemanticError::RuleLimit { limit: 1 })
    ));

    let nodes = scan_source("nodes", b"SecRule ARGS \"@eq 1\" \"id:1\"\n", SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&nodes, SyntaxLimits::default()).unwrap();
    assert!(analyze_semantic_draft_inner(
        vec![syntax],
        SemanticLimits {
            max_nodes: 3,
            ..SemanticLimits::default()
        },
        false,
        false,
    )
    .is_ok());
    let syntax = parse_syntax(&nodes, SyntaxLimits::default()).unwrap();
    assert!(matches!(
        analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                max_nodes: 2,
                ..SemanticLimits::default()
            }
        ),
        Err(SemanticError::NodeLimit { limit: 2 })
    ));
}

#[test]
fn document_slots_are_charged_and_caller_capacity_is_normalized() {
    let source = scan_source("empty", b"", SourceLimits::default()).unwrap();
    let slot = size_of::<SyntaxDocument<'_>>();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    assert!(matches!(
        analyze_one_semantic_draft(
            syntax,
            SemanticLimits {
                max_owned_bytes: slot - 1,
                ..SemanticLimits::default()
            }
        ),
        Err(SemanticError::PreflightOwnedByteLimit { .. })
    ));
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let draft = analyze_one_semantic_draft(
        syntax,
        SemanticLimits {
            max_owned_bytes: slot,
            ..SemanticLimits::default()
        },
    )
    .unwrap();
    assert_eq!(draft.semantic_charged_bytes(), slot);

    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let mut documents = Vec::with_capacity(4_096);
    documents.push(syntax);
    let caller_backing = documents.as_ptr();
    let draft = analyze_semantic_draft(
        documents,
        SemanticLimits {
            max_owned_bytes: slot,
            ..SemanticLimits::default()
        },
    )
    .unwrap();
    assert_eq!(draft.semantic_charged_bytes(), slot);
    assert_ne!(draft.documents.as_ptr(), caller_backing);
}

#[test]
fn aggregate_multidocument_and_dynamic_charges_have_exact_boundaries() {
    let first = scan_source("first", b"SecAction \"id:1\"\n", SourceLimits::default()).unwrap();
    let second = scan_source("second", b"SecAction \"id:2\"\n", SourceLimits::default()).unwrap();
    let inherited = parse_syntax(&first, SyntaxLimits::default())
        .unwrap()
        .retained_charged_bytes()
        + parse_syntax(&second, SyntaxLimits::default())
            .unwrap()
            .retained_charged_bytes();
    let fixed = 2
        * (size_of::<SyntaxDocument<'_>>()
            + size_of::<DirectiveSemanticRecord>()
            + size_of::<SemanticActionRecord<'_>>()
            + size_of::<ActionOperandRecord<'_>>());
    let documents = vec![
        parse_syntax(&first, SyntaxLimits::default()).unwrap(),
        parse_syntax(&second, SyntaxLimits::default()).unwrap(),
    ];
    assert!(analyze_semantic_draft_inner(
        documents,
        SemanticLimits {
            max_owned_bytes: fixed,
            max_retained_bytes: inherited + fixed,
            ..SemanticLimits::default()
        },
        false,
        false,
    )
    .is_ok());
    let documents = vec![
        parse_syntax(&first, SyntaxLimits::default()).unwrap(),
        parse_syntax(&second, SyntaxLimits::default()).unwrap(),
    ];
    assert!(matches!(
        analyze_semantic_draft_inner(
            documents,
            SemanticLimits {
                max_owned_bytes: fixed,
                max_retained_bytes: inherited + fixed - 1,
                ..SemanticLimits::default()
            },
            false,
            false,
        ),
        Err(SemanticError::RetainedByteLimit { .. })
    ));

    for (text, dynamic_size, capability) in [
        (
            "SecRule ARGS \"@rx x\" \"id:1\"\n",
            size_of::<RequirementRecord>() + size_of::<StaticOperandRecord<'_>>(),
            false,
        ),
        (
            "SecRule RESPONSE_BODY \"@validateUrlEncoding\" \"id:1\"\n",
            size_of::<CapabilityRecord>(),
            true,
        ),
    ] {
        let source = scan_source("dynamic", text.as_bytes(), SourceLimits::default()).unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        let inherited = syntax.retained_charged_bytes();
        let fixed = size_of::<SyntaxDocument<'_>>()
            + size_of::<DirectiveSemanticRecord>()
            + size_of::<SemanticTarget<'_>>()
            + size_of::<SemanticOperatorRecord<'_>>()
            + size_of::<OperatorOperandRecord>()
            + size_of::<SemanticActionRecord<'_>>()
            + size_of::<ActionOperandRecord<'_>>();
        let draft = analyze_semantic_draft_inner(
            vec![syntax],
            SemanticLimits {
                max_owned_bytes: fixed + dynamic_size,
                max_retained_bytes: inherited + fixed + dynamic_size,
                ..SemanticLimits::default()
            },
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            (draft.requirements().len(), draft.unavailable_capabilities().len()),
            if capability { (0, 1) } else { (1, 0) }
        );
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(matches!(
            analyze_semantic_draft_inner(
                vec![syntax],
                SemanticLimits {
                    max_owned_bytes: fixed + dynamic_size - 1,
                    max_retained_bytes: usize::MAX,
                    ..SemanticLimits::default()
                },
                false,
                false,
            ),
            Err(SemanticError::OwnedByteLimit { .. })
        ));
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        assert!(matches!(
            analyze_semantic_draft_inner(
                vec![syntax],
                SemanticLimits {
                    max_owned_bytes: usize::MAX,
                    max_retained_bytes: inherited + fixed + dynamic_size - 1,
                    ..SemanticLimits::default()
                },
                false,
                false,
            ),
            Err(SemanticError::RetainedByteLimit { .. })
        ));
    }
}

#[test]
fn response_body_capability_follows_positive_or_count_atoms_only() {
    let result = with_draft(
        "SecRule RESPONSE_BODY|!RESPONSE_BODY|&RESPONSE_BODY \"@eq 1\" \"id:1\"\n",
        |draft| {
            (
                draft.targets().map(|target| target.modifier()).collect::<Vec<_>>(),
                draft
                    .unavailable_capabilities()
                    .filter(|record| record.capability() == UnavailableCapability::FullResponseBody)
                    .count(),
            )
        },
    )
    .unwrap();
    assert_eq!(
        result.0,
        [TargetModifier::None, TargetModifier::Exclude, TargetModifier::Count]
    );
    assert_eq!(result.1, 2);
    let exclusion = with_draft("SecRule !RESPONSE_BODY \"@eq 1\" \"id:1\"\n", |draft| {
        draft.unavailable_capabilities().len()
    })
    .unwrap();
    assert_eq!(exclusion, 0);
}

#[test]
fn semantic_error_families_are_atomic_and_redacted_through_source_chain() {
    let cases = [
        "SecRule SECRET_COLLECTION \"@eq 1\" \"id:1\"\n",
        "SecRule ARGS \"@secret_operator 1\" \"id:1\"\n",
        "SecAction \"secret_action:value\"\n",
        "SecAction \"t:secret_transform\"\n",
        "SecAction \"id:secret_scalar\"\n",
        "SecAction \"id:%{SECRET.scalar}\"\n",
        "SecAction \"block:secret_value\"\n",
    ];
    for text in cases {
        let error = with_draft(text, |_| ()).unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.to_ascii_lowercase().contains("secret"));
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            let rendered = format!("{cause:?} {cause}");
            assert!(!rendered.to_ascii_lowercase().contains("secret"));
            source = cause.source();
        }
    }
    assert_eq!(with_draft("SecAction \"id:7\"\n", |draft| draft.action_count()), Ok(1));
}

#[test]
fn validation_context_and_default_resolution_are_typed_and_exact() {
    assert!(matches!(
        SemanticValidationContext::try_new(
            None,
            &[
                PhaseDefaults::pass(Phase::RequestBody, true, true),
                PhaseDefaults::pass(Phase::RequestBody, false, false),
            ],
        ),
        Err(DefaultsContextError::DuplicatePhase {
            phase: Phase::RequestBody
        })
    ));
    for status in [99, 100, 101, 199, 600] {
        assert!(matches!(
            PhaseDefaults::try_deny(Phase::RequestBody, status, true, true),
            Err(DefaultsContextError::InvalidStatus {
                phase: Phase::RequestBody
            })
        ));
    }
    for status in [200, 599] {
        assert!(PhaseDefaults::try_deny(Phase::RequestBody, status, true, true).is_ok());
    }

    let deny = PhaseDefaults::try_deny(Phase::RequestBody, 403, true, false).unwrap();
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &[deny]).unwrap();
    let state = with_validated(
        "SecRule ARGS \"@eq 1\" \"id:1,block,status:451,nolog,auditlog\"\n",
        context,
        ValidationLimits::default(),
        |validated| {
            let state = validated.rule_by_id(1).unwrap().root().resolved_state();
            (
                state.phase(),
                state.disruptive(),
                state.status(),
                state.log(),
                state.audit_log(),
            )
        },
    )
    .unwrap();
    assert_eq!(
        state,
        (Phase::RequestBody, ResolvedDisruptive::Deny, Some(451), false, true)
    );

    let missing = SemanticValidationContext::try_new(None, &[deny]).unwrap();
    assert!(matches!(
        with_validated(
            "SecAction \"id:1,pass\"\n",
            missing,
            ValidationLimits::default(),
            |_| (),
        ),
        Err(SemanticValidationError::MissingImplicitPhase { .. })
    ));
    let pass = SemanticValidationContext::try_new(
        Some(Phase::RequestBody),
        &[PhaseDefaults::pass(Phase::RequestBody, true, true)],
    )
    .unwrap();
    assert!(matches!(
        with_validated(
            "SecAction \"id:1,block,status:451\"\n",
            pass,
            ValidationLimits::default(),
            |_| (),
        ),
        Err(SemanticValidationError::BlockStatusPass { .. })
    ));
}

#[test]
fn sealed_validation_views_preserve_units_order_updates_and_removals() {
    let text = concat!(
        "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
        "SecRule REQUEST_HEADERS \"@eq 1\" \"id:2,phase:2,pass\"\n",
        "SecRuleUpdateTargetById 1-2 TX:added\n",
        "SecRuleRemoveById 2\n",
    );
    let evidence = with_validated(
        text,
        crs_validation_context(),
        ValidationLimits::default(),
        |validated| {
            let units = validated
                .rule_units()
                .map(|unit| (unit.id(), unit.is_active(), unit.members().len()))
                .collect::<Vec<_>>();
            let active = validated.active_rule_units().map(|unit| unit.id()).collect::<Vec<_>>();
            let update = validated.applied_target_updates().next().unwrap();
            let removal = validated.applied_rule_removals().next().unwrap();
            (
                units,
                active,
                validated.rule_by_id(2).is_none(),
                update.affected_units().map(|unit| unit.id()).collect::<Vec<_>>(),
                removal.affected_units().map(|unit| unit.id()).collect::<Vec<_>>(),
                validated.rule_by_id(1).unwrap().root().final_targets().len(),
            )
        },
    )
    .unwrap();
    assert_eq!(evidence.0, [(1, true, 1), (2, false, 1)]);
    assert_eq!(evidence.1, [1]);
    assert!(evidence.2);
    assert_eq!(evidence.3, [1, 2]);
    assert_eq!(evidence.4, [2]);
    assert_eq!(evidence.5, 2);
}

#[test]
fn exception_selectors_enforce_order_activity_and_whole_chain_removal() {
    let cases = [
        (
            concat!(
                "SecRuleUpdateTargetById 1 TX:x\n",
                "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
            ),
            "forward",
        ),
        (
            concat!(
                "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
                "SecRuleRemoveById 1\n",
                "SecRuleUpdateTargetById 1 TX:x\n",
            ),
            "inactive",
        ),
        (
            concat!(
                "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
                "SecRuleRemoveById 1\n",
                "SecRuleRemoveById 1\n",
            ),
            "empty",
        ),
    ];
    for (text, expected) in cases {
        let error = with_validated(text, crs_validation_context(), ValidationLimits::default(), |_| ()).unwrap_err();
        assert!(
            matches!(
                (&error, expected),
                (SemanticValidationError::ForwardIdSelector { .. }, "forward")
                    | (SemanticValidationError::InactiveUpdate { .. }, "inactive")
                    | (SemanticValidationError::EmptyRemoval { .. }, "empty")
            ),
            "unexpected {error:?}"
        );
    }

    let chain = concat!(
        "SecRule ARGS \"@eq 1\" \"id:3,phase:2,pass,chain\"\n",
        "SecRule REQUEST_HEADERS \"@eq 1\" \"\"\n",
        "SecRuleRemoveById 3\n",
    );
    let result = with_validated(
        chain,
        crs_validation_context(),
        ValidationLimits::default(),
        |validated| {
            let unit = validated.rule_units().next().unwrap();
            (
                unit.members().len(),
                unit.is_active(),
                validated.active_rule_units().len(),
            )
        },
    )
    .unwrap();
    assert_eq!(result, (2, false, 0));
}

#[test]
fn selector_union_counts_work_and_first_discovery_order_are_exact() {
    let text = concat!(
        "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@eq 1\" \"id:3,phase:2,pass\"\n",
        "SecRuleRemoveById 1-3 3\n",
    );
    let exact = ValidationLimits {
        max_raw_matches_per_directive: 3,
        max_distinct_matches_per_directive: 2,
        max_raw_matches: 3,
        max_distinct_matches: 2,
        max_selector_work: 11,
        ..ValidationLimits::default()
    };
    let affected = with_validated(text, crs_validation_context(), exact, |validated| {
        validated
            .applied_rule_removals()
            .next()
            .unwrap()
            .affected_units()
            .map(|unit| unit.id())
            .collect::<Vec<_>>()
    })
    .unwrap();
    assert_eq!(affected, [1, 3]);
    for (limits, expected) in [
        (
            ValidationLimits {
                max_raw_matches_per_directive: 2,
                ..exact
            },
            "raw",
        ),
        (
            ValidationLimits {
                max_distinct_matches_per_directive: 1,
                ..exact
            },
            "distinct",
        ),
        (
            ValidationLimits {
                max_selector_work: 10,
                ..exact
            },
            "work",
        ),
    ] {
        let error = with_validated(text, crs_validation_context(), limits, |_| ()).unwrap_err();
        assert!(
            matches!(
                (&error, expected),
                (SemanticValidationError::RawMatchDirectiveLimit { .. }, "raw")
                    | (SemanticValidationError::DistinctMatchDirectiveLimit { .. }, "distinct")
                    | (SemanticValidationError::SelectorWorkLimit { .. }, "work")
            ),
            "unexpected {error:?}"
        );
    }
}

#[test]
fn target_exclusions_require_an_applicable_positive_base() {
    for (initial, exclusion) in [("ARGS", "!ARGS:any"), ("ARGS:name", "!ARGS:name")] {
        let text =
            format!("SecRule {initial} \"@eq 1\" \"id:1,phase:2,pass\"\nSecRuleUpdateTargetById 1 {exclusion}\n");
        assert!(with_validated(&text, crs_validation_context(), ValidationLimits::default(), |_| (),).is_ok());
    }
    for (initial, exclusion) in [("ARGS:name", "!ARGS:other"), ("&ARGS", "!ARGS")] {
        let text =
            format!("SecRule {initial} \"@eq 1\" \"id:1,phase:2,pass\"\nSecRuleUpdateTargetById 1 {exclusion}\n");
        assert!(matches!(
            with_validated(&text, crs_validation_context(), ValidationLimits::default(), |_| (),),
            Err(SemanticValidationError::MissingExclusionBase { .. })
        ));
    }
}

#[test]
fn duplicate_ids_are_global_even_when_the_first_unit_is_removed() {
    let error = with_validated(
        concat!(
            "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
            "SecRuleRemoveById 1\n",
            "SecAction \"id:1,phase:2,pass\"\n",
        ),
        crs_validation_context(),
        ValidationLimits::default(),
        |_| (),
    )
    .unwrap_err();
    assert!(matches!(error, SemanticValidationError::DuplicateRuleId { .. }));
}

#[test]
fn collection_phase_and_selector_tables_cover_every_registered_collection() {
    let request_body = [
        CollectionKind::Args,
        CollectionKind::ArgsNames,
        CollectionKind::Files,
        CollectionKind::FilesCombinedSize,
        CollectionKind::FilesNames,
        CollectionKind::MultipartPartHeaders,
        CollectionKind::RequestBody,
        CollectionKind::RequestBodyLength,
        CollectionKind::Xml,
    ];
    let response_headers = [CollectionKind::ResponseHeaders, CollectionKind::ResponseStatus];
    for &collection in CollectionKind::ALL {
        let expected = if request_body.contains(&collection) {
            Phase::RequestBody
        } else if response_headers.contains(&collection) {
            Phase::ResponseHeaders
        } else if collection == CollectionKind::ResponseBody {
            Phase::ResponseBody
        } else {
            Phase::RequestHeaders
        };
        assert_eq!(minimum_collection_phase(collection), expected, "{collection:?}");
    }
    let selectable = [
        CollectionKind::Args,
        CollectionKind::ArgsGet,
        CollectionKind::ArgsGetNames,
        CollectionKind::ArgsNames,
        CollectionKind::Files,
        CollectionKind::FilesNames,
        CollectionKind::MatchedVars,
        CollectionKind::MultipartPartHeaders,
        CollectionKind::RequestCookies,
        CollectionKind::RequestCookiesNames,
        CollectionKind::RequestHeaders,
        CollectionKind::RequestHeadersNames,
        CollectionKind::ResponseHeaders,
        CollectionKind::Tx,
        CollectionKind::Xml,
    ];
    for &collection in CollectionKind::ALL {
        assert_eq!(
            collection_accepts_selector(collection),
            selectable.contains(&collection)
        );
    }
}

#[test]
fn target_regex_requirements_follow_final_target_identity_only() {
    let text = concat!(
        "SecRule ARGS:/first/ \"@rx value\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@eq 1\" \"id:2,phase:2,pass\"\n",
        "SecRuleUpdateTargetById 1-2 ARGS:/added/\n",
        "SecRuleRemoveById 1\n",
    );
    let requirements = with_validated(
        text,
        crs_validation_context(),
        ValidationLimits::default(),
        |validated| {
            validated
                .requirements()
                .map(|requirement| (requirement.kind(), requirement.target().is_some()))
                .collect::<Vec<_>>()
        },
    )
    .unwrap();
    assert_eq!(
        requirements,
        [
            (CompileRequirement::RegexCompilation, false),
            (CompileRequirement::TargetSelectorRegexCompilation, true),
        ]
    );
}

#[test]
fn removed_units_cannot_hide_unavailable_capabilities() {
    let error = with_validated(
        concat!(
            "SecRule RESPONSE_BODY \"@validateUrlEncoding\" \"id:1,phase:4,pass\"\n",
            "SecRuleRemoveById 1\n",
        ),
        crs_validation_context(),
        ValidationLimits::default(),
        |_| (),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        SemanticValidationError::UnavailableCapability {
            capability: UnavailableCapability::FullResponseBody,
            ..
        }
    ));
}

#[test]
fn request_body_processor_is_header_available_but_body_bytes_are_not() {
    assert!(with_validated(
        "SecRule REQBODY_PROCESSOR \"@streq JSON\" \"id:1,phase:1,pass\"\n",
        crs_validation_context(),
        ValidationLimits::default(),
        |_| (),
    )
    .is_ok());
    assert!(matches!(
        with_validated(
            "SecRule REQUEST_BODY \"@streq value\" \"id:1,phase:1,pass\"\n",
            crs_validation_context(),
            ValidationLimits::default(),
            |_| (),
        ),
        Err(SemanticValidationError::CollectionPhase {
            collection: CollectionKind::RequestBody,
            phase: Phase::RequestHeaders,
            ..
        })
    ));
}

#[test]
fn validation_limits_have_exact_success_and_first_excess_boundaries() {
    let text = "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n";
    let charge = with_validated(
        text,
        crs_validation_context(),
        ValidationLimits::default(),
        |validated| (validated.validation_charged_bytes(), validated.retained_charged_bytes()),
    )
    .unwrap();
    assert!(with_validated(
        text,
        crs_validation_context(),
        ValidationLimits {
            max_owned_bytes: charge.0,
            max_retained_bytes: charge.1,
            ..ValidationLimits::default()
        },
        |_| (),
    )
    .is_ok());
    assert!(matches!(
        with_validated(
            text,
            crs_validation_context(),
            ValidationLimits {
                max_owned_bytes: charge.0 - 1,
                ..ValidationLimits::default()
            },
            |_| (),
        ),
        Err(SemanticValidationError::OwnedByteLimit { .. })
    ));
    assert!(matches!(
        with_validated(
            text,
            crs_validation_context(),
            ValidationLimits {
                max_rule_units: 0,
                ..ValidationLimits::default()
            },
            |_| (),
        ),
        Err(SemanticValidationError::RuleUnitLimit { limit: 0 })
    ));
}

#[test]
fn validation_work_exception_target_and_comparison_limits_are_exact() {
    let simple = "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n";
    let working = validation_working_bytes(1, 1, 1, 1, 0, false, ValidationLimits::default()).unwrap();
    assert!(with_validated(
        simple,
        crs_validation_context(),
        ValidationLimits {
            max_working_bytes: working,
            ..ValidationLimits::default()
        },
        |_| (),
    )
    .is_ok());
    assert!(matches!(
        with_validated(
            simple,
            crs_validation_context(),
            ValidationLimits {
                max_working_bytes: working - 1,
                ..ValidationLimits::default()
            },
            |_| (),
        ),
        Err(SemanticValidationError::WorkingByteLimit { .. })
    ));
    for (limits, expected) in [
        (
            ValidationLimits {
                max_final_targets_per_rule: 0,
                ..ValidationLimits::default()
            },
            "per-rule",
        ),
        (
            ValidationLimits {
                max_final_target_references: 0,
                ..ValidationLimits::default()
            },
            "aggregate",
        ),
    ] {
        let error = with_validated(simple, crs_validation_context(), limits, |_| ()).unwrap_err();
        assert!(
            matches!(
                (&error, expected),
                (SemanticValidationError::FinalTargetLimit { .. }, "per-rule")
                    | (SemanticValidationError::FinalTargetReferenceLimit { .. }, "aggregate")
            ),
            "unexpected {error:?}"
        );
    }

    let exception = concat!(
        "SecRule ARGS:name \"@eq 1\" \"id:1,phase:2,pass\"\n",
        "SecRuleUpdateTargetById 1 !ARGS:name\n",
    );
    assert!(with_validated(
        exception,
        crs_validation_context(),
        ValidationLimits {
            max_id_selectors: 1,
            max_applied_exceptions: 1,
            max_target_comparisons: 1,
            max_target_comparison_bytes: 8,
            ..ValidationLimits::default()
        },
        |_| (),
    )
    .is_ok());
    for (limits, expected) in [
        (
            ValidationLimits {
                max_id_selectors: 0,
                ..ValidationLimits::default()
            },
            "selectors",
        ),
        (
            ValidationLimits {
                max_applied_exceptions: 0,
                ..ValidationLimits::default()
            },
            "exceptions",
        ),
        (
            ValidationLimits {
                max_target_comparisons: 0,
                ..ValidationLimits::default()
            },
            "comparisons",
        ),
        (
            ValidationLimits {
                max_target_comparison_bytes: 7,
                ..ValidationLimits::default()
            },
            "comparison-bytes",
        ),
    ] {
        let error = with_validated(exception, crs_validation_context(), limits, |_| ()).unwrap_err();
        assert!(
            matches!(
                (&error, expected),
                (SemanticValidationError::IdSelectorLimit { .. }, "selectors")
                    | (SemanticValidationError::ExceptionLimit { .. }, "exceptions")
                    | (SemanticValidationError::TargetComparisonLimit { .. }, "comparisons")
                    | (
                        SemanticValidationError::TargetComparisonByteLimit { .. },
                        "comparison-bytes"
                    )
            ),
            "unexpected {error:?}"
        );
    }
}

#[test]
fn overlay_preflight_bound_covers_zero_per_directive_cumulative_and_composed_shapes() {
    let rules = concat!(
        "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@eq 1\" \"id:2,phase:2,pass\"\n",
        "SecRule ARGS \"@eq 1\" \"id:3,phase:2,pass\"\n",
    );
    let cases = [
            (rules.to_owned(), 3, 100, 0),
            (
                format!("{rules}SecRuleUpdateTargetById 1-2 TX:a|REQUEST_HEADERS:b\n"),
                2,
                100,
                4,
            ),
            (
                format!(
                    "{rules}SecRuleUpdateTargetById 1-3 TX:a|REQUEST_HEADERS:b\nSecRuleUpdateTargetById 1 1-3 TX:c|REQUEST_HEADERS:d|ARGS_GET:e\n"
                ),
                3,
                2,
                6,
            ),
            (
                format!(
                    "{rules}SecRuleUpdateTargetById 1-2 1 TX:a|REQUEST_HEADERS:b\nSecRuleRemoveById 2\nSecRuleUpdateTargetById 1 3 TX:c|REQUEST_HEADERS:d|ARGS_GET:e\n"
                ),
                2,
                100,
                10,
            ),
        ];
    for (text, per_directive, cumulative, expected_bound) in cases {
        let limits = ValidationLimits {
            max_distinct_matches_per_directive: per_directive,
            max_distinct_matches: cumulative,
            ..ValidationLimits::default()
        };
        let (nodes, targets, evidence) = with_full_draft(&text, |draft| {
            let units = draft
                .final_rules
                .iter()
                .filter(|rule| !matches!(rule.role, FinalRuleRole::ChainChild { .. }))
                .count();
            (
                draft.final_rules.len(),
                draft.targets.len(),
                validation_exception_shape(draft, units, limits).unwrap(),
            )
        })
        .unwrap();
        assert_eq!(evidence.3, expected_bound, "{text}");
        let working = validation_working_bytes(
            3,
            nodes,
            targets,
            3_usize.min(per_directive),
            expected_bound,
            false,
            limits,
        )
        .unwrap();
        if cumulative >= 3 || expected_bound == 0 {
            assert!(with_validated(
                &text,
                crs_validation_context(),
                ValidationLimits {
                    max_working_bytes: working,
                    ..limits
                },
                |_| (),
            )
            .is_ok());
        }
        assert!(matches!(
            with_validated(
                &text,
                crs_validation_context(),
                ValidationLimits {
                    max_working_bytes: working - 1,
                    ..limits
                },
                |_| (),
            ),
            Err(SemanticValidationError::WorkingByteLimit { .. })
        ));
    }
}

#[test]
fn cumulative_raw_and_distinct_limits_anchor_the_first_excess_selector() {
    let text = concat!(
        "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@eq 1\" \"id:2,phase:2,pass\"\n",
        "SecRuleUpdateTargetById 1-2 2 TX:first\n",
        "SecRuleUpdateTargetById 1 1-2 TX:secret_second\n",
    );
    let exact = ValidationLimits {
        max_raw_matches_per_directive: 3,
        max_distinct_matches_per_directive: 2,
        max_raw_matches: 6,
        max_distinct_matches: 4,
        max_selector_work: 22,
        ..ValidationLimits::default()
    };
    let overlays = with_validated(text, crs_validation_context(), exact, |validated| {
        (
            validated
                .applied_target_updates()
                .map(|update| update.affected_units().map(|unit| unit.id()).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            validated
                .rule_units()
                .map(|unit| unit.root().final_targets().len())
                .collect::<Vec<_>>(),
        )
    })
    .unwrap();
    assert_eq!(overlays, (vec![vec![1, 2], vec![1, 2]], vec![3, 3]));

    let selector_offset = text.find("1-2 TX:secret_second").unwrap();
    for (limits, expected) in [
        (
            ValidationLimits {
                max_raw_matches: 5,
                ..exact
            },
            "raw",
        ),
        (
            ValidationLimits {
                max_distinct_matches: 3,
                ..exact
            },
            "distinct",
        ),
    ] {
        let error = with_validated(text, crs_validation_context(), limits, |_| ()).unwrap_err();
        let position = match (&error, expected) {
            (SemanticValidationError::RawMatchLimit { position, .. }, "raw")
            | (SemanticValidationError::DistinctMatchLimit { position, .. }, "distinct") => *position,
            _ => panic!("unexpected {error:?}"),
        };
        assert_eq!((position.line, position.byte_offset), (4, selector_offset));
        let rendered = format!("{error:?} {error}").to_ascii_lowercase();
        assert!(!rendered.contains("secret"));
    }
}

#[test]
fn generated_selector_unions_and_final_overlays_match_the_reference_model() {
    let mut seed = 0x05e1_ec70_1d15_ca11_u64;
    let next = |state: &mut u64| {
        *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        *state
    };
    for case in 0..64_u64 {
        let unit_count = 1 + (next(&mut seed) % 5) as usize;
        let mut source = String::new();
        let mut active = vec![true; unit_count];
        let mut overlays = vec![vec![(CollectionKind::Args, String::new())]; unit_count];
        let mut expected_updates = Vec::new();
        let mut expected_removals = Vec::new();
        for id in 1..=unit_count {
            source.push_str(&format!("SecRule ARGS \"@eq 1\" \"id:{id},phase:2,pass\"\n"));
        }
        let update_count = 1 + (next(&mut seed) % 3) as usize;
        for update in 0..update_count {
            let left = 1 + (next(&mut seed) % unit_count as u64) as usize;
            let right = 1 + (next(&mut seed) % unit_count as u64) as usize;
            let (start, end) = (left.min(right), left.max(right));
            let shape = next(&mut seed) % 4;
            let (selectors, affected) = match shape {
                0 => (left.to_string(), vec![left]),
                1 => ((format!("{start}-{end}")), (start..=end).collect::<Vec<_>>()),
                2 => (format!("{start}-{end} {end}"), (start..=end).collect::<Vec<_>>()),
                _ => {
                    let mut affected = vec![end];
                    affected.extend((start..end).filter(|id| *id != end));
                    (format!("{end} {start}-{end}"), affected)
                }
            };
            let first = format!("u{case}_{update}");
            let second = format!("h{case}_{update}");
            let two_atoms = next(&mut seed) & 1 != 0;
            let targets = if two_atoms {
                format!("TX:{first}|REQUEST_HEADERS:{second}")
            } else {
                format!("TX:{first}")
            };
            source.push_str(&format!("SecRuleUpdateTargetById {selectors} {targets}\n"));
            for &id in &affected {
                overlays[id - 1].push((CollectionKind::Tx, first.clone()));
                if two_atoms {
                    overlays[id - 1].push((CollectionKind::RequestHeaders, second.clone()));
                }
            }
            expected_updates.push(affected);
        }
        if next(&mut seed) & 1 != 0 {
            let start = 1 + (next(&mut seed) % unit_count as u64) as usize;
            let end = 1 + (next(&mut seed) % unit_count as u64) as usize;
            let (start, end) = (start.min(end), start.max(end));
            let affected = (start..=end).collect::<Vec<_>>();
            source.push_str(&format!("SecRuleRemoveById {start}-{end} {end}\n"));
            for &id in &affected {
                active[id - 1] = false;
            }
            expected_removals.push(affected);
        }
        assert!(source.len() <= 4 * 1024);
        let actual = with_validated(
            &source,
            crs_validation_context(),
            ValidationLimits::default(),
            |validated| {
                let updates = validated
                    .applied_target_updates()
                    .map(|value| {
                        value
                            .affected_units()
                            .map(|unit| unit.id() as usize)
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                let removals = validated
                    .applied_rule_removals()
                    .map(|value| {
                        value
                            .affected_units()
                            .map(|unit| unit.id() as usize)
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                let units = validated
                    .rule_units()
                    .map(|unit| {
                        (
                            unit.is_active(),
                            unit.root()
                                .final_targets()
                                .map(|target| {
                                    (
                                        target.collection(),
                                        target
                                            .selector()
                                            .map(|selector| selector.text().to_owned())
                                            .unwrap_or_default(),
                                    )
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>();
                (updates, removals, units)
            },
        )
        .unwrap_or_else(|error| panic!("case {case}: {error:?}\n{source}"));
        assert_eq!(actual.0, expected_updates, "case {case}");
        assert_eq!(actual.1, expected_removals, "case {case}");
        assert_eq!(
            actual.2,
            active.into_iter().zip(overlays).collect::<Vec<_>>(),
            "case {case}"
        );
    }
}

#[test]
fn validation_temporary_and_retained_allocation_failures_are_typed_and_atomic() {
    let text = concat!(
        "SecRule ARGS:/first/ \"@rx value\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@eq 1\" \"id:2,phase:2,pass\"\n",
        "SecRuleUpdateTargetById 1-2 ARGS:/added/\n",
        "SecRuleRemoveById 2\n",
    );
    let operations = [
        "rule ID index",
        "active rule state",
        "resolved rule states",
        "target overlay heads",
        "target overlay tails",
        "final target counts",
        "target overlays",
        "selector generations",
        "affected rule candidates",
        "target requirement generations",
        "target regex candidates",
        "validated rule nodes",
        "validated rule units",
        "validated rule ID index",
        "final target references",
        "applied target updates",
        "applied rule removals",
        "applied exception units",
        "validated requirements",
    ];
    for operation in operations {
        VALIDATION_ALLOCATION_FAILURE.with(|failure| failure.set(Some(operation)));
        let result = with_validated(text, crs_validation_context(), ValidationLimits::default(), |_| ());
        VALIDATION_ALLOCATION_FAILURE.with(|failure| failure.set(None));
        assert_eq!(result, Err(SemanticValidationError::Allocation { operation }));
        let error = result.unwrap_err();
        let rendered = format!("{error:?} {error}").to_ascii_lowercase();
        assert!(!rendered.contains("first"));
    }
    let evidence = with_validated(
        text,
        crs_validation_context(),
        ValidationLimits::default(),
        |validated| {
            (
                validated.rule_units().map(|unit| unit.is_active()).collect::<Vec<_>>(),
                validated.applied_target_updates().len(),
                validated.applied_rule_removals().len(),
                validated.requirements().len(),
            )
        },
    )
    .unwrap();
    assert_eq!(evidence, (vec![true, false], 1, 1, 3));
}

#[test]
fn validation_errors_are_content_free_through_the_complete_source_chain() {
    let error = with_validated(
        concat!(
            "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
            "SecRuleUpdateTargetById 1 !TX:secret_selector\n",
        ),
        crs_validation_context(),
        ValidationLimits::default(),
        |_| (),
    )
    .unwrap_err();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(cause) = current {
        let rendered = format!("{cause:?} {cause}").to_ascii_lowercase();
        assert!(!rendered.contains("secret"));
        current = cause.source();
    }
}

#[test]
fn pinned_crs_validation_pre_capability_evidence_and_final_gate() {
    let Some(root) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") else {
        return;
    };
    let root = PathBuf::from(root);
    let mut paths = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "conf"))
        .collect::<Vec<_>>();
    paths.sort();
    let sources = paths
        .iter()
        .map(|path| {
            let source_id = path.strip_prefix(&root).unwrap().to_string_lossy();
            scan_source(&source_id, &fs::read(path).unwrap(), SourceLimits::default()).unwrap()
        })
        .collect::<Vec<_>>();
    let make_draft = || {
        let documents = sources
            .iter()
            .map(|source| parse_syntax(source, SyntaxLimits::default()).unwrap())
            .collect::<Vec<_>>();
        analyze_semantic_draft(documents, SemanticLimits::default()).unwrap()
    };

    let final_error =
        validate_semantic_draft(make_draft(), crs_validation_context(), ValidationLimits::default()).unwrap_err();
    assert!(
        matches!(
            final_error,
            SemanticValidationError::UnavailableCapability {
                capability: UnavailableCapability::Ctl,
                ..
            }
        ),
        "{final_error:?}"
    );

    let mut draft = make_draft();
    assert_eq!(draft.unavailable.len(), 79);
    draft.unavailable.clear();
    let validated = validate_semantic_draft(draft, crs_validation_context(), ValidationLimits::default()).unwrap();
    let unit_ids = validated.rule_units().map(|unit| unit.id()).collect::<Vec<_>>();
    assert_eq!(
        (
            unit_ids.len(),
            unit_ids.iter().copied().min(),
            unit_ids.iter().copied().max(),
            validated.active_rule_units().len(),
            validated.applied_target_updates().len(),
            validated.applied_rule_removals().len(),
            validated
                .applied_target_updates()
                .map(|update| update.affected_units().len())
                .sum::<usize>(),
            validated
                .rule_nodes()
                .map(|node| node.final_targets().len())
                .sum::<usize>(),
        ),
        (627, Some(901_001), Some(980_170), 627, 55, 0, 55, 1_790)
    );
    let mut requirement_counts = [0_usize; 8];
    for requirement in validated.requirements() {
        requirement_counts[match requirement.kind() {
            CompileRequirement::RegexCompilation => 0,
            CompileRequirement::TargetSelectorRegexCompilation => 1,
            CompileRequirement::PhraseMatcherCompilation => 2,
            CompileRequirement::ExternalPhraseAsset => 3,
            CompileRequirement::RuntimeMacro => 4,
            CompileRequirement::BlockDefaultDisruptive => 5,
            CompileRequirement::TransactionSetVar => 6,
            CompileRequirement::DefaultPhase => 7,
        }] += 1;
    }
    assert_eq!(requirement_counts, [314, 37, 33, 21, 996, 0, 678, 0]);
    let mut update_collections = BTreeMap::new();
    let mut update_selectors = [0_usize; 3];
    let mut max_atom_bytes = 0_usize;
    let mut total_atom_bytes = 0_usize;
    for update in validated.applied_target_updates() {
        for target in update.source_directive().targets() {
            *update_collections.entry(target.collection()).or_insert(0_usize) += 1;
            update_selectors[match target.selector_kind() {
                SelectorKind::Literal => 0,
                SelectorKind::Regex(_) => 1,
                SelectorKind::Path => 2,
                SelectorKind::None => unreachable!("pinned updates have selectors"),
            }] += 1;
            let bytes = target.span().end.byte_offset - target.span().start.byte_offset;
            max_atom_bytes = max_atom_bytes.max(bytes);
            total_atom_bytes += bytes;
        }
    }
    assert_eq!(
        update_collections,
        BTreeMap::from([(CollectionKind::ArgsNames, 1), (CollectionKind::RequestCookies, 54)])
    );
    assert_eq!(
        (update_selectors, max_atom_bytes, total_atom_bytes),
        ([27, 28, 0], 33, 1_416)
    );
    assert_eq!(
        (validated.validation_charged_bytes(), validated.retained_charged_bytes()),
        (108_024, 9_724_171)
    );
    assert_eq!(
        (
            size_of::<ResolvedRuleStateRecord>(),
            size_of::<ValidatedRuleNodeRecord>(),
            size_of::<ValidatedRuleUnitRecord>(),
            size_of::<RuleIdIndexRecord>(),
            size_of::<TargetOverlayEntry>(),
            size_of::<AppliedExceptionRecord>(),
            size_of::<ValidatedRequirementReference>(),
        ),
        (24, 48, 24, 16, 16, 24, 16)
    );
    eprintln!(
        "validated_charge={} retained={} record_sizes={}/{}/{}/{}/{}/{}/{} requirements={:?}",
        validated.validation_charged_bytes(),
        validated.retained_charged_bytes(),
        size_of::<ResolvedRuleStateRecord>(),
        size_of::<ValidatedRuleNodeRecord>(),
        size_of::<ValidatedRuleUnitRecord>(),
        size_of::<RuleIdIndexRecord>(),
        size_of::<TargetOverlayEntry>(),
        size_of::<AppliedExceptionRecord>(),
        size_of::<ValidatedRequirementReference>(),
        requirement_counts,
    );
}

#[test]
fn pinned_crs_semantic_classification_gate() {
    let Some(root) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") else {
        return;
    };
    let root = PathBuf::from(root);
    let mut paths = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "conf"))
        .collect::<Vec<PathBuf>>();
    paths.sort();
    let sources = paths
        .iter()
        .map(|path| {
            let source_id = path
                .strip_prefix(&root)
                .expect("enumerated CRS path remains under configured root")
                .to_string_lossy();
            scan_source(&source_id, &fs::read(path).unwrap(), SourceLimits::default()).unwrap()
        })
        .collect::<Vec<_>>();
    let documents = sources
        .iter()
        .map(|source| parse_syntax(source, SyntaxLimits::default()).unwrap())
        .collect::<Vec<_>>();
    let draft = analyze_semantic_draft(documents, SemanticLimits::default()).unwrap();
    let mut collections = BTreeMap::new();
    let mut operators = BTreeMap::new();
    let mut actions = BTreeMap::new();
    let mut transforms = BTreeMap::new();
    let mut scalars = BTreeMap::new();
    let mut ids = Vec::new();
    let mut phases = [0usize; 5];
    let mut severities = [0usize; 8];
    let mut statuses = BTreeMap::new();
    let mut skips = BTreeMap::new();
    let mut macro_sources = BTreeMap::new();
    let mut macro_fields = BTreeMap::new();
    let mut macro_templates = 0usize;
    let mut max_expansions = 0usize;
    let mut max_segments = 0usize;
    let mut normalized_macro_bodies = std::collections::BTreeSet::new();
    let mut max_body_bytes = 0usize;
    let mut operand_classes = BTreeMap::new();
    let mut phrase_total = 0usize;
    let mut max_phrases_per_operator = 0usize;
    let mut asset_total = 0usize;
    let mut unique_assets = std::collections::BTreeSet::new();
    let mut max_asset_bytes = 0usize;
    let mut network_total = 0usize;
    let mut max_networks_per_operator = 0usize;
    let mut byte_member_total = 0usize;
    let mut max_byte_members = 0usize;
    let mut max_regex_bytes = 0usize;
    let mut integer_constants = BTreeMap::<&str, Vec<i64>>::new();
    let mut action_operand_classes = BTreeMap::new();
    let mut setvar_operations = [0usize; 4];
    let mut max_setvar_whole = 0usize;
    let mut max_setvar_key = 0usize;
    let mut max_setvar_value = 0usize;
    let mut macro_setvar_keys = 0usize;
    let mut macro_setvar_values = 0usize;
    let mut macro_setvar_union = 0usize;
    let mut initcols = Vec::new();
    let mut ctl_kinds = BTreeMap::new();
    let mut ctl_target_collections = BTreeMap::new();
    for target in draft.targets() {
        *collections.entry(target.collection().as_str()).or_insert(0usize) += 1;
    }
    for operator in draft.operators() {
        *operators.entry(operator.kind().as_str()).or_insert(0usize) += 1;
        let class = match operator.operand() {
            OperatorOperandView::NoArgument => "no_argument",
            OperatorOperandView::Regex(text) => {
                max_regex_bytes = max_regex_bytes.max(text.text().len());
                "regex"
            }
            OperatorOperandView::StringTemplate(_) => "string_template",
            OperatorOperandView::IntegerTemplate { constant, .. } => {
                if let Some(value) = constant {
                    integer_constants
                        .entry(operator.kind().as_str())
                        .or_default()
                        .push(value);
                }
                "integer_template"
            }
            OperatorOperandView::PhraseList(phrases) => {
                phrase_total += phrases.phrases().len();
                max_phrases_per_operator = max_phrases_per_operator.max(phrases.phrases().len());
                "phrase_list"
            }
            OperatorOperandView::ExternalAssets(assets) => {
                asset_total += assets.assets().len();
                for asset in assets.assets() {
                    max_asset_bytes = max_asset_bytes.max(asset.handle().len());
                    unique_assets.insert(asset.handle().to_owned());
                }
                "external_assets"
            }
            OperatorOperandView::IpNetworks(networks) => {
                network_total += networks.networks().len();
                max_networks_per_operator = max_networks_per_operator.max(networks.networks().len());
                "ip_networks"
            }
            OperatorOperandView::ByteSet(bytes) => {
                byte_member_total += bytes.member_count();
                max_byte_members = max_byte_members.max(bytes.member_count());
                "byte_set"
            }
        };
        *operand_classes.entry(class).or_insert(0usize) += 1;
    }
    for action in draft.actions() {
        match action.kind() {
            ActionEnvelopeKind::Action(kind) => *actions.entry(kind.as_str()).or_insert(0usize) += 1,
            ActionEnvelopeKind::Transformation => {
                *transforms
                    .entry(action.transformation().unwrap().as_str())
                    .or_insert(0usize) += 1
            }
        }
        let key = match action.local_value() {
            LocalActionValue::Id(value) => {
                ids.push(value);
                Some("id")
            }
            LocalActionValue::Phase(value) => {
                phases[value as usize - 1] += 1;
                Some("phase")
            }
            LocalActionValue::Severity(value) => {
                severities[value as usize] += 1;
                Some("severity")
            }
            LocalActionValue::Status(value) => {
                *statuses.entry(value).or_insert(0usize) += 1;
                Some("status")
            }
            LocalActionValue::Skip(value) => {
                *skips.entry(value).or_insert(0usize) += 1;
                Some("skip")
            }
            _ => None,
        };
        if let Some(key) = key {
            *scalars.entry(key).or_insert(0usize) += 1;
        }
        let class = match action.operand() {
            ActionOperandView::NoArgument => "no_argument",
            ActionOperandView::Local(_) => "scalar",
            ActionOperandView::Transformation(_) => "transformation",
            ActionOperandView::Template(_) => "template",
            ActionOperandView::Literal(_) => "literal",
            ActionOperandView::SetVar(setvar) => {
                setvar_operations[match setvar.operation() {
                    SetVarOperation::Assign => 0,
                    SetVarOperation::Add => 1,
                    SetVarOperation::Subtract => 2,
                    SetVarOperation::Delete => 3,
                }] += 1;
                let whole = action.template().expect("setvar template");
                let key = setvar.key();
                max_setvar_whole = max_setvar_whole.max(whole.text().len());
                max_setvar_key = max_setvar_key.max(key.text().len());
                if key.expansion_count() > 0 {
                    macro_setvar_keys += 1;
                }
                let value_has_macro = setvar.value().is_some_and(|value| {
                    max_setvar_value = max_setvar_value.max(value.text().len());
                    value.expansion_count() > 0
                });
                macro_setvar_values += usize::from(value_has_macro);
                macro_setvar_union += usize::from(key.expansion_count() > 0 || value_has_macro);
                "setvar"
            }
            ActionOperandView::InitCol(initcol) => {
                initcols.push((initcol.collection(), initcol.key().text().to_owned()));
                "initcol"
            }
            ActionOperandView::Ctl(ctl) => {
                let key = match ctl {
                    CtlView::AuditEngine(_) => "auditEngine",
                    CtlView::ForceRequestBodyVariable(_) => "forceRequestBodyVariable",
                    CtlView::RequestBodyProcessor(_) => "requestBodyProcessor",
                    CtlView::RuleRemoveByTag(_) => "ruleRemoveByTag",
                    CtlView::RuleRemoveTargetByTag { target, .. } => {
                        *ctl_target_collections
                            .entry(target.collection().as_str())
                            .or_insert(0usize) += 1;
                        "ruleRemoveTargetByTag"
                    }
                };
                *ctl_kinds.entry(key).or_insert(0usize) += 1;
                "ctl"
            }
        };
        *action_operand_classes.entry(class).or_insert(0usize) += 1;
    }
    for expansion in &draft.expansions {
        *macro_sources.entry(format!("{:?}", expansion.source)).or_insert(0usize) += 1;
        normalized_macro_bodies.insert(expansion.body.text().to_ascii_lowercase());
        max_body_bytes = max_body_bytes.max(expansion.body.text().len());
    }
    for operator in draft.operators() {
        if let Some(template) = operator.template() {
            max_expansions = max_expansions.max(template.expansion_count());
            max_segments = max_segments.max(template.segments().len());
            if template.expansion_count() > 0 {
                macro_templates += 1;
                *macro_fields
                    .entry(format!("operator:{}", operator.kind().as_str()))
                    .or_insert(0usize) += 1;
            }
        }
    }
    for action in draft.actions() {
        if let Some(template) = action.template() {
            max_expansions = max_expansions.max(template.expansion_count());
            max_segments = max_segments.max(template.segments().len());
            if template.expansion_count() > 0 {
                macro_templates += 1;
                let ActionEnvelopeKind::Action(kind) = action.kind() else {
                    unreachable!()
                };
                *macro_fields
                    .entry(format!("action:{}", kind.as_str()))
                    .or_insert(0usize) += 1;
            }
        }
    }
    assert_eq!(
        (collections.len(), operators.len(), actions.len(), transforms.len()),
        (33, 19, 23, 20)
    );
    assert_eq!(
        (draft.target_count(), draft.operator_count(), draft.action_count()),
        (1_790, 693, 8_975)
    );
    assert_eq!(transforms.values().sum::<usize>(), 832);
    assert_eq!(
        scalars,
        BTreeMap::from([
            ("id", 627),
            ("phase", 627),
            ("severity", 342),
            ("skip", 1),
            ("status", 2)
        ])
    );
    ids.sort_unstable();
    assert_eq!(
        (ids.len(), ids.first(), ids.last()),
        (627, Some(&901_001), Some(&980_170))
    );
    ids.dedup();
    assert_eq!(ids.len(), 627);
    assert_eq!(phases, [173, 289, 43, 109, 13]);
    assert_eq!(severities, [0, 0, 301, 16, 20, 5, 0, 0]);
    assert_eq!(statuses, BTreeMap::from([(500, 2)]));
    assert_eq!(skips, BTreeMap::from([(1, 1)]));
    macro_rules! assert_map {
            ($actual:expr, [$($name:literal => $count:literal),+ $(,)?]) => {
                assert_eq!($actual, BTreeMap::from([$(($name, $count)),+]));
            };
        }
    assert_map!(collections, [
        "ARGS"=>189, "ARGS_COMBINED_SIZE"=>1, "ARGS_GET"=>2, "ARGS_GET_NAMES"=>1,
        "ARGS_NAMES"=>186, "FILES"=>9, "FILES_COMBINED_SIZE"=>1, "FILES_NAMES"=>4,
        "MATCHED_VAR"=>3, "MATCHED_VARS"=>12, "MULTIPART_PART_HEADERS"=>5, "QUERY_STRING"=>1,
        "REMOTE_ADDR"=>2, "REQBODY_PROCESSOR"=>5, "REQUEST_BASENAME"=>5, "REQUEST_BODY"=>17,
        "REQUEST_BODY_LENGTH"=>1, "REQUEST_COOKIES"=>219, "REQUEST_COOKIES_NAMES"=>163,
        "REQUEST_FILENAME"=>61, "REQUEST_HEADERS"=>164, "REQUEST_HEADERS_NAMES"=>3,
        "REQUEST_LINE"=>7, "REQUEST_METHOD"=>8, "REQUEST_PROTOCOL"=>2, "REQUEST_URI"=>3,
        "REQUEST_URI_RAW"=>7, "RESPONSE_BODY"=>59, "RESPONSE_HEADERS"=>7, "RESPONSE_STATUS"=>2,
        "TX"=>292, "UNIQUE_ID"=>1, "XML"=>348
    ]);
    assert_map!(operators, [
        "beginsWith"=>1, "contains"=>4, "detectSQLi"=>2, "detectXSS"=>2, "endsWith"=>6,
        "eq"=>55, "ge"=>42, "gt"=>15, "ipMatch"=>2, "lt"=>190, "pm"=>12,
        "pmFromFile"=>21, "rx"=>314, "streq"=>9, "unconditionalMatch"=>1,
        "validateByteRange"=>6, "validateUrlEncoding"=>1, "validateUtf8Encoding"=>1, "within"=>9
    ]);
    assert_map!(operand_classes, [
        "no_argument"=>7, "regex"=>314, "string_template"=>29,
        "integer_template"=>302, "phrase_list"=>12, "external_assets"=>21,
        "ip_networks"=>2, "byte_set"=>6
    ]);
    assert_eq!(
        (
            phrase_total,
            draft.phrase_bytes(),
            max_phrases_per_operator,
            asset_total,
            unique_assets.len(),
            max_asset_bytes,
            network_total,
            max_networks_per_operator,
            byte_member_total,
            max_byte_members,
            max_regex_bytes,
        ),
        (56, 395, 11, 21, 19, 32, 4, 2, 29, 8, 12_106)
    );
    assert_map!(actions, [
        "auditlog"=>1, "block"=>341, "capture"=>275, "chain"=>73, "ctl"=>18, "deny"=>6,
        "id"=>627, "initcol"=>2, "log"=>8, "logdata"=>327, "msg"=>351, "multiMatch"=>8,
        "noauditlog"=>5, "nolog"=>278, "pass"=>280, "phase"=>627, "setvar"=>678,
        "severity"=>342, "skip"=>1, "skipAfter"=>206, "status"=>2, "tag"=>3060, "ver"=>627
    ]);
    assert_map!(action_operand_classes, [
        "no_argument"=>1275, "scalar"=>1599, "transformation"=>832,
        "template"=>678, "literal"=>3893, "setvar"=>678, "initcol"=>2, "ctl"=>18
    ]);
    assert_eq!(setvar_operations, [73, 605, 0, 0]);
    assert_eq!(
        (
            max_setvar_whole,
            max_setvar_key,
            max_setvar_value,
            macro_setvar_keys,
            macro_setvar_values,
            macro_setvar_union,
        ),
        (672, 71, 647, 5, 628, 629)
    );
    assert_eq!(
        initcols,
        [
            (InitColCollection::Global, "global".to_owned()),
            (InitColCollection::Ip, "%{remote_addr}_%{MATCHED_VAR}".to_owned()),
        ]
    );
    assert_map!(ctl_kinds, [
        "auditEngine"=>2, "forceRequestBodyVariable"=>2, "requestBodyProcessor"=>1,
        "ruleRemoveByTag"=>3, "ruleRemoveTargetByTag"=>10
    ]);
    assert_map!(ctl_target_collections, ["REQUEST_FILENAME"=>1, "XML"=>9]);
    assert_map!(transforms, [
        "base64Decode"=>3, "cmdLine"=>13, "compressWhitespace"=>5, "cssDecode"=>23,
        "escapeSeqDecode"=>7, "hexEncode"=>2, "htmlEntityDecode"=>35, "jsDecode"=>37,
        "length"=>3, "lowercase"=>46, "none"=>427, "normalizePath"=>8,
        "normalizePathWin"=>4, "removeCommentsChar"=>1, "removeNulls"=>27,
        "removeWhitespace"=>11, "replaceComments"=>9, "sha1"=>2, "urlDecodeUni"=>137,
        "utf8toUnicode"=>32
    ]);
    let mut fold_roles = [0_usize; 3];
    let mut disruptive = [0_usize; 3];
    let mut reset_lists = 0_usize;
    let mut effective = BTreeMap::<String, usize>::new();
    let mut max_effective = 0_usize;
    let mut max_actions = [0_usize; 3];
    let mut child_presence = [0_usize; 3];
    for fold in draft.folded_rules() {
        let class = match (fold.envelope(), fold.role()) {
            (LocalRuleEnvelope::Rule, Some(ProvisionalRuleRole::Root)) => 0,
            (LocalRuleEnvelope::Rule, Some(ProvisionalRuleRole::Child { .. })) => 1,
            (LocalRuleEnvelope::Action, None) => 2,
            _ => panic!("invalid provisional fold role"),
        };
        fold_roles[class] += 1;
        max_actions[class] = max_actions[class].max(fold.actions().len());
        if class == 1 {
            child_presence[match fold.action_presence() {
                FoldActionPresence::Absent => 0,
                FoldActionPresence::PresentEmpty => 1,
                FoldActionPresence::PresentNonempty => 2,
            }] += 1;
        }
        if fold.reset() {
            reset_lists += 1;
        }
        max_effective = max_effective.max(fold.effective_transformations().len());
        for action in fold.effective_transformations() {
            *effective
                .entry(action.transformation().unwrap().as_str().to_owned())
                .or_default() += 1;
        }
        if let Some(value) = fold.disruptive() {
            disruptive[match value {
                DisruptiveKind::Block => 0,
                DisruptiveKind::Deny => 1,
                DisruptiveKind::Pass => 2,
            }] += 1;
        }
    }
    assert_eq!(fold_roles, [620, 73, 7]);
    assert_eq!(disruptive, [341, 6, 280]);
    assert_eq!(max_actions, [29, 5, 28]);
    assert_eq!(child_presence, [2, 0, 71]);
    assert_eq!(reset_lists, 427);
    assert_eq!(effective.values().sum::<usize>(), 405);
    assert_eq!(effective.get("none"), None);
    assert_eq!(effective.len(), 19);
    assert_eq!(max_effective, 7);
    for (kind, count) in &effective {
        assert_eq!(transforms.get(kind.as_str()), Some(count));
    }
    let mut final_roles = [0_usize; 4];
    let mut inherited_phases = [0_usize; 5];
    for rule in draft.final_rules() {
        final_roles[match rule.role() {
            FinalRuleRole::StandaloneRule => 0,
            FinalRuleRole::ChainRoot { .. } => 1,
            FinalRuleRole::ChainChild { .. } => 2,
            FinalRuleRole::Action => 3,
        }] += 1;
        if let FinalPhaseState::Inherited { phase, .. } = rule.phase() {
            inherited_phases[phase as usize - 1] += 1;
        }
    }
    assert_eq!(final_roles, [565, 55, 73, 7]);
    assert_eq!(inherited_phases, [42, 29, 1, 1, 0]);
    let mut chain_lengths = BTreeMap::new();
    let mut starter_phases = [0_usize; 5];
    let mut terminal_presence = [0_usize; 3];
    for chain in draft.chains() {
        *chain_lengths.entry(chain.members().len()).or_default() += 1;
        if let FinalPhaseState::Explicit { phase, .. } = chain.phase() {
            starter_phases[phase as usize - 1] += 1;
        }
        terminal_presence[match chain.members().last().unwrap().rule().fold().action_presence() {
            FoldActionPresence::Absent => 0,
            FoldActionPresence::PresentEmpty => 1,
            FoldActionPresence::PresentNonempty => 2,
        }] += 1;
    }
    assert_eq!(chain_lengths, BTreeMap::from([(2, 41), (3, 11), (4, 2), (5, 1)]));
    assert_eq!(starter_phases, [28, 25, 1, 1, 0]);
    assert_eq!(terminal_presence, [2, 0, 53]);
    let marker_names = draft.markers().map(|marker| marker.name()).collect::<BTreeSet<_>>();
    assert_eq!(
        (
            marker_names.len(),
            marker_names.iter().map(|name| name.len()).max(),
            marker_names.iter().map(|name| name.len()).sum::<usize>()
        ),
        (30, Some(51), 955)
    );
    let resolutions = draft.resolved_skip_afters().collect::<Vec<_>>();
    let referenced = resolutions
        .iter()
        .map(|resolution| resolution.target().name())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        (
            resolutions.len(),
            referenced.len(),
            resolutions.iter().map(|value| value.target().name().len()).max(),
            resolutions
                .iter()
                .map(|value| value.target().name().len())
                .sum::<usize>(),
            marker_names.difference(&referenced).count()
        ),
        (206, 28, Some(51), 7_041, 2)
    );
    assert_eq!(
        (draft.requirements().len(), draft.unavailable_capabilities().len()),
        (2_383, 79)
    );
    let mut requirement_counts = [0usize; 8];
    for record in draft.requirements() {
        requirement_counts[match record.requirement() {
            CompileRequirement::RegexCompilation => 0,
            CompileRequirement::TargetSelectorRegexCompilation => 1,
            CompileRequirement::PhraseMatcherCompilation => 2,
            CompileRequirement::ExternalPhraseAsset => 3,
            CompileRequirement::RuntimeMacro => 4,
            CompileRequirement::BlockDefaultDisruptive => 5,
            CompileRequirement::TransactionSetVar => 6,
            CompileRequirement::DefaultPhase => 7,
        }] += 1;
    }
    let mut unavailable_counts = [0usize; 3];
    for record in draft.unavailable_capabilities() {
        unavailable_counts[match record.capability() {
            UnavailableCapability::FullResponseBody => 0,
            UnavailableCapability::Ctl => 1,
            UnavailableCapability::InitCol => 2,
        }] += 1;
    }
    assert_eq!(requirement_counts, [314, 0, 33, 21, 996, 341, 678, 0]);
    assert_eq!(unavailable_counts, [59, 18, 2]);
    assert_eq!(
        (
            draft.template_count(),
            draft.template_segment_count(),
            draft.expansion_count()
        ),
        (1_689, 3_665, 1_515)
    );
    assert_eq!(
        (
            macro_templates,
            normalized_macro_bodies.len(),
            max_expansions,
            max_segments,
            max_body_bytes
        ),
        (996, 66, 23, 47, 39)
    );
    assert_eq!(
        macro_sources,
        BTreeMap::from([
            ("Args".to_owned(), 4),
            ("MatchedVar".to_owned(), 265),
            ("MatchedVarName".to_owned(), 293),
            ("RemoteAddr".to_owned(), 1),
            ("RequestHeaders".to_owned(), 3),
            ("RequestLine".to_owned(), 1),
            ("Tx".to_owned(), 948),
        ])
    );
    assert_eq!(
        macro_fields,
        BTreeMap::from([
            ("action:initcol".to_owned(), 1),
            ("action:logdata".to_owned(), 327),
            ("action:msg".to_owned(), 9),
            ("action:setvar".to_owned(), 629),
            ("operator:endsWith".to_owned(), 2),
            ("operator:ge".to_owned(), 8),
            ("operator:gt".to_owned(), 6),
            ("operator:lt".to_owned(), 3),
            ("operator:streq".to_owned(), 3),
            ("operator:within".to_owned(), 8),
        ])
    );
    assert_eq!(
        (draft.scanned_operand_bytes(), draft.decoded_operand_bytes()),
        (345_591, 123_498)
    );
    assert_eq!(
        (
            size_of::<TemplateRecord<'_>>(),
            size_of::<TemplateSegmentRecord>(),
            size_of::<ExpansionRecord<'_>>()
        ),
        (104, 80, 248)
    );
    assert_eq!(
        (
            size_of::<OperatorOperandRecord>(),
            size_of::<StaticOperandRecord<'_>>(),
            size_of::<ListItemRecord>(),
            size_of::<NetworkRecord>()
        ),
        (56, 80, 64, 72)
    );
    assert_eq!(
        (
            size_of::<SyntaxDocument<'_>>(),
            size_of::<DirectiveSemanticRecord>(),
            size_of::<SemanticTarget<'_>>(),
            size_of::<SemanticOperatorRecord<'_>>(),
            size_of::<SemanticActionRecord<'_>>(),
            size_of::<RequirementRecord>(),
            size_of::<CapabilityRecord>()
        ),
        (336, 80, 328, 128, 128, 40, 40)
    );
    assert_eq!(
        (
            size_of::<ActionOperandRecord<'_>>(),
            size_of::<TemplateSliceRecord>(),
            size_of::<SetVarRecord>(),
            size_of::<InitColRecord>(),
            size_of::<CtlRecord<'_>>(),
            size_of::<CtlTargetRecord<'_>>(),
        ),
        (88, 96, 224, 104, 96, 296)
    );
    assert_eq!((size_of::<LocalRuleFoldRecord>(), size_of::<usize>()), (264, 8));
    assert_eq!(
        (
            size_of::<FinalRuleRecord>(),
            size_of::<ChainRecord>(),
            size_of::<MarkerRecord<'_>>(),
            size_of::<ResolvedSkipAfterRecord>()
        ),
        (112, 56, 80, 24)
    );
    assert_eq!(
        (draft.semantic_charged_bytes(), draft.retained_charged_bytes()),
        (4_260_170, 9_616_147)
    );
    for values in integer_constants.values_mut() {
        values.sort_unstable();
    }
    assert_eq!(
        integer_constants
            .iter()
            .map(|(kind, values)| (*kind, (values.len(), values[0], values[values.len() - 1])))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            ("eq", (55, 0, 100)),
            ("ge", (34, 1, 5)),
            ("gt", (9, 0, 100)),
            ("lt", (187, 1, 4)),
        ])
    );
    eprintln!(
            "operand_classes={operand_classes:?} integer_ranges={integer_constants:?} phrases={phrase_total}/{} max_phrases={max_phrases_per_operator} assets={asset_total}/{} max_asset={max_asset_bytes} networks={network_total}/{max_networks_per_operator} byte_members={byte_member_total}/{max_byte_members} regex_max={max_regex_bytes} requirements={} unavailable={} templates={}/{}/{} macro_templates={macro_templates} distinct_bodies={} maxima={max_expansions}/{max_segments}/{max_body_bytes} scanned={} decoded={} operand_sizes={}/{}/{}/{} charge={} retained={}",
            draft.phrase_bytes(),
            unique_assets.len(),
            draft.requirements().len(),
            draft.unavailable_capabilities().len(),
            draft.template_count(),
            draft.template_segment_count(),
            draft.expansion_count(),
            normalized_macro_bodies.len(),
            draft.scanned_operand_bytes(),
            draft.decoded_operand_bytes(),
            size_of::<OperatorOperandRecord>(),
            size_of::<StaticOperandRecord<'_>>(),
            size_of::<ListItemRecord>(),
            size_of::<NetworkRecord>(),
            draft.semantic_charged_bytes(),
            draft.retained_charged_bytes()
        );
}
