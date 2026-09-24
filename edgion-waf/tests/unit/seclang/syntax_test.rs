use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{parse_syntax, validate, SyntaxDocument, SyntaxError, SyntaxInvariantKind, SyntaxLimits};
use crate::seclang::action::{parse_action_syntax, ActionEntryKind, ActionListKind, ActionSyntaxLimits};
use crate::seclang::operator::{
    operator_record_size_for_test, parse_operator_syntax, OperatorNameView, OperatorSyntaxLimits,
};
use crate::seclang::target::{
    parse_target_syntax, DirectiveTargetView, SelectorKind, TargetModifier, TargetSyntaxLimits,
};
use crate::seclang::{
    parse_directives, scan_source, tokenize_document, DirectiveKind, DirectiveLimits, SourceLimits, TokenizerLimits,
};

fn with_syntax(source: &[u8], inspect: impl FnOnce(&SyntaxDocument<'_>)) {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    inspect(&syntax);
}

#[test]
fn aligns_all_envelopes_and_preserves_absent_and_empty_actions() {
    with_syntax(b"", |document| {
        assert!(document.is_empty());
        assert!(document.get(0).is_none());
        assert_eq!(document.iter().len(), 0);
    });
    with_syntax(
            b"SecRule ARGS \"@rx x\"\nSecRule ARGS \"@rx x\" \"\"\nSecAction \"deny\"\nSecRuleUpdateTargetById 1 ARGS\nSecRuleRemoveById 1 2-3\nSecMarker END\nSecComponentSignature \"fixture\"",
            |document| {
                assert_eq!(document.len(), 7);
                assert_eq!(document.iter().len(), 7);
                assert_eq!(document.iter().map(|view| view.directive().directive().kind()).collect::<Vec<_>>(), [
                    DirectiveKind::SecRule,
                    DirectiveKind::SecRule,
                    DirectiveKind::SecAction,
                    DirectiveKind::SecRuleUpdateTargetById,
                    DirectiveKind::SecRuleRemoveById,
                    DirectiveKind::SecMarker,
                    DirectiveKind::SecComponentSignature,
                ]);
                let first = document.get(0).unwrap();
                assert!(matches!(first.targets(), DirectiveTargetView::Rule { .. }));
                assert!(first.operator().is_some());
                assert!(first.actions().is_none());
                assert_eq!(document.get(1).unwrap().actions().unwrap().entries().len(), 0);
                assert_eq!(document.get(2).unwrap().actions().unwrap().kind(), ActionListKind::Action);
                assert!(matches!(document.get(3).unwrap().targets(), DirectiveTargetView::Update { .. }));
                assert!(matches!(
                    document.get(4).unwrap().targets(),
                    DirectiveTargetView::Remove { ids } if ids.len() == 2
                ));
                assert!(document.get(5).unwrap().operator().is_none());
                assert!(document.get(7).is_none());
                assert_eq!(document.source().source_id(), "test");
            },
        );
}

#[test]
fn operator_coordinates_distinguish_implicit_and_explicit_source_text() {
    with_syntax(b"SecRule ARGS \"plain\"\nSecRule ARGS \"!@rx value\"", |document| {
        let implicit = document.get(0).unwrap().operator().unwrap();
        assert!(matches!(implicit.name(), OperatorNameView::ImplicitRx));
        assert!(implicit.explicit_spelling().is_none());
        assert_eq!(implicit.argument().raw(), "plain");
        assert_eq!(implicit.argument_logical_range(), 14..19);
        let explicit = document.get(1).unwrap().operator().unwrap();
        let OperatorNameView::Explicit(name) = explicit.name() else {
            panic!("expected explicit operator name")
        };
        assert_eq!(name.raw(), "rx");
        assert_eq!(explicit.explicit_spelling().unwrap().raw(), "!@rx value");
        assert_eq!(explicit.name_logical_range(), Some(16..18));
        assert_eq!(explicit.argument().raw(), "value");
        assert_eq!(explicit.argument_logical_range(), 19..24);
        assert_eq!(explicit.name_span().unwrap().start.line, 2);
    });
    with_syntax(b"SecRule ARGS \"@r\\\nx value\"", |document| {
        let operator = document.get(0).unwrap().operator().unwrap();
        assert_eq!(operator.name_span().unwrap().start.line, 1);
        assert_eq!(operator.name_span().unwrap().end.line, 2);
        assert_eq!(operator.argument_span().start.line, 2);
        assert_eq!(operator.argument_span().end.line, 2);
    });
}

#[test]
fn stage_precedence_retained_limit_and_diagnostics_are_typed_and_redacted() {
    let cases = [
        (b"SecAction \"unterminated".as_slice(), "Tokenizer"),
        (b"Unknown secret-marker".as_slice(), "Directive"),
        (b"SecRule ARGS||BODY \"x\"".as_slice(), "Target"),
        (b"SecAction \"msg:'secret-marker\"".as_slice(), "Action"),
    ];
    for (bytes, stage) in cases {
        let source = scan_source("test", bytes, SourceLimits::default()).unwrap();
        let error = parse_syntax(&source, SyntaxLimits::default()).unwrap_err();
        let debug = format!("{error:?}");
        assert!(debug.starts_with(stage), "expected {stage}, got {debug}");
        assert!(std::error::Error::source(&error).is_some());
        assert_error_chain_redacted(&error, "secret-marker");
    }
    let source = scan_source("test", b"SecRule ARGS \"x\"", SourceLimits::default()).unwrap();
    let error = parse_syntax(
        &source,
        SyntaxLimits {
            operators: OperatorSyntaxLimits {
                max_rule_operators: 0,
                ..OperatorSyntaxLimits::default()
            },
            ..SyntaxLimits::default()
        },
    )
    .unwrap_err();
    assert!(matches!(error, SyntaxError::Operator(_)));
    assert!(std::error::Error::source(&error).is_some());
    assert_error_chain_redacted(&error, "secret-marker");
    let source = scan_source("test", b"SecMarker END", SourceLimits::default()).unwrap();
    assert!(matches!(
        parse_syntax(
            &source,
            SyntaxLimits {
                max_retained_bytes: 0,
                ..SyntaxLimits::default()
            }
        ),
        Err(SyntaxError::RetainedByteLimit { limit: 0 })
    ));
}

fn raw_actions<'source>(
    source: &'source crate::seclang::SourceDocument,
) -> crate::seclang::action::ActionSyntaxDocument<'source> {
    let tokenized = tokenize_document(source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    drop(tokenized);
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
    parse_action_syntax(operators, ActionSyntaxLimits::default(), usize::MAX).unwrap()
}

#[test]
fn invariant_validation_rejects_same_length_shift_and_wrong_kind() {
    let source = scan_source(
        "test",
        b"SecRule ARGS \"x\" \"deny\"\nSecAction \"log\"",
        SourceLimits::default(),
    )
    .unwrap();
    let mut shifted = raw_actions(&source);
    shifted.swap_mappings_for_test(0, 1);
    assert!(matches!(
        validate(&shifted),
        Err(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::LogicalLine,
            index: 0
        })
    ));
    let mut wrong_kind = raw_actions(&source);
    wrong_kind.set_first_list_kind_for_test(ActionListKind::Action);
    assert!(matches!(
        validate(&wrong_kind),
        Err(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::NestedKind,
            index: 0
        })
    ));

    let mut missing_action = raw_actions(&source);
    missing_action.set_first_mapping_none_for_test();
    assert!(matches!(
        validate(&missing_action),
        Err(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::ActionPresence,
            index: 0
        })
    ));

    let mut target_wrong_kind = raw_actions(&source);
    target_wrong_kind
        .operators_mut_for_test()
        .targets_mut_for_test()
        .set_first_record_none_for_test();
    assert!(matches!(
        validate(&target_wrong_kind),
        Err(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::NestedKind,
            index: 0
        })
    ));

    let mut target_shifted = raw_actions(&source);
    target_shifted
        .operators_mut_for_test()
        .targets_mut_for_test()
        .swap_records_for_test(0, 1);
    assert!(matches!(
        validate(&target_shifted),
        Err(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::LogicalLine,
            index: 0
        })
    ));

    let mut operator_wrong_kind = raw_actions(&source);
    operator_wrong_kind
        .operators_mut_for_test()
        .set_first_mapping_none_for_test();
    assert!(matches!(
        validate(&operator_wrong_kind),
        Err(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::NestedKind,
            index: 0
        })
    ));

    let mut operator_shifted = raw_actions(&source);
    operator_shifted
        .operators_mut_for_test()
        .shift_first_operator_line_for_test();
    assert!(matches!(
        validate(&operator_shifted),
        Err(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::LogicalLine,
            index: 0
        })
    ));

    let remove_source = scan_source("remove", b"SecRuleRemoveById 1", SourceLimits::default()).unwrap();
    let mut remove_wrong_kind = raw_actions(&remove_source);
    remove_wrong_kind
        .operators_mut_for_test()
        .targets_mut_for_test()
        .set_first_record_none_for_test();
    assert!(matches!(
        validate(&remove_wrong_kind),
        Err(SyntaxError::Invariant {
            kind: SyntaxInvariantKind::NestedKind,
            index: 0
        })
    ));
}

struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }
}

#[test]
fn bounded_generator_streams_ten_thousand_mixed_valid_models() {
    const CASES: usize = 10_000;
    const MAX_TOTAL: usize = 2_500_000;
    let names = ["deny", "log", "id:1", "phase:2", "t:none", "msg:'ok'"];
    let mut random = XorShift64(0x4544_4749_4f4e_5741);
    let mut total = 0;
    for _ in 0..CASES {
        let count = (random.next() as usize % 8) + 1;
        let mut actions = String::new();
        let mut expected = Vec::with_capacity(count);
        for index in 0..count {
            if index != 0 {
                actions.push(',');
            }
            let action = names[random.next() as usize % names.len()];
            actions.push_str(action);
            expected.push(action.split_once(':').map_or(action, |(name, _)| name));
        }
        let rule_has_actions = random.next().is_multiple_of(2);
        let rule_line = if rule_has_actions {
            "SecRule ARGS \"@rx needle\" \"log\"".to_owned()
        } else {
            "SecRule ARGS \"@rx needle\"".to_owned()
        };
        let mut models = [
            (
                rule_line,
                GeneratedModel::Rule {
                    target: "ARGS",
                    operator: "@rx needle",
                    actions: rule_has_actions.then_some("log"),
                },
            ),
            (
                format!("SecAction \"{actions}\""),
                GeneratedModel::Action {
                    actions: actions.clone(),
                    names: expected,
                },
            ),
            (
                "SecRuleUpdateTargetById 1 2 BODY".to_owned(),
                GeneratedModel::Update {
                    target: "BODY",
                    ids: ["1", "2"],
                },
            ),
        ];
        for index in (1..models.len()).rev() {
            let selected = random.next() as usize % (index + 1);
            models.swap(index, selected);
        }
        let input = models
            .iter()
            .map(|(line, _)| line.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(input.len() <= 256);
        total += input.len();
        with_syntax(input.as_bytes(), |document| {
            assert_eq!(document.len(), 3);
            assert_eq!(document.target_atom_count(), 2);
            assert_eq!(document.id_selector_count(), 2);
            assert_eq!(document.operator_count(), 1);
            assert_eq!(document.action_list_count(), 1 + usize::from(rule_has_actions));
            assert_eq!(document.action_entry_count(), count + usize::from(rule_has_actions));
            for (index, (_, model)) in models.iter().enumerate() {
                assert_generated_model(document, index, model);
            }
        });
    }
    assert!(total <= MAX_TOTAL);
}

enum GeneratedModel {
    Rule {
        target: &'static str,
        operator: &'static str,
        actions: Option<&'static str>,
    },
    Action {
        actions: String,
        names: Vec<&'static str>,
    },
    Update {
        target: &'static str,
        ids: [&'static str; 2],
    },
}

fn assert_borrowed_from(text: &str, owner: &str) {
    let text_start = text.as_ptr() as usize;
    let text_end = text_start + text.len();
    let owner_start = owner.as_ptr() as usize;
    let owner_end = owner_start + owner.len();
    assert!(text_start >= owner_start && text_end <= owner_end);
}

fn assert_generated_model(document: &SyntaxDocument<'_>, index: usize, expected: &GeneratedModel) {
    let view = document.get(index).unwrap();
    let line = &document.source().lines()[view.directive().logical_line_index()];
    match expected {
        GeneratedModel::Rule {
            target,
            operator,
            actions,
        } => {
            assert_eq!(view.directive().directive().kind(), DirectiveKind::SecRule);
            let DirectiveTargetView::Rule { atoms } = view.targets() else {
                panic!("expected rule targets")
            };
            assert_eq!(atoms[0].collection().text(), *target);
            assert_borrowed_from(atoms[0].collection().text(), line.text());
            let actual_operator = view.operator().unwrap().content().raw();
            assert_eq!(actual_operator, *operator);
            assert_borrowed_from(actual_operator, line.text());
            assert_eq!(view.actions().map(|list| list.content()), *actions);
            if let Some(list) = view.actions() {
                assert_borrowed_from(list.content(), line.text());
            }
        }
        GeneratedModel::Action { actions, names } => {
            assert_eq!(view.directive().directive().kind(), DirectiveKind::SecAction);
            assert!(matches!(view.targets(), DirectiveTargetView::None));
            assert!(view.operator().is_none());
            let list = view.actions().unwrap();
            assert_eq!(list.content(), actions);
            assert_borrowed_from(list.content(), line.text());
            assert_eq!(
                list.entries()
                    .iter()
                    .map(|entry| entry.name().text())
                    .collect::<Vec<_>>(),
                *names
            );
        }
        GeneratedModel::Update { target, ids } => {
            assert_eq!(
                view.directive().directive().kind(),
                DirectiveKind::SecRuleUpdateTargetById
            );
            let DirectiveTargetView::Update { ids: actual_ids, atoms } = view.targets() else {
                panic!("expected update targets")
            };
            assert_eq!(actual_ids.iter().map(|id| id.raw()).collect::<Vec<_>>(), *ids);
            assert_eq!(atoms[0].collection().text(), *target);
            assert_borrowed_from(atoms[0].collection().text(), line.text());
            assert!(view.operator().is_none());
            assert!(view.actions().is_none());
        }
    }
}

#[test]
fn bounded_mutations_never_panic_or_emit_input_content() {
    const CAP: usize = 256 * 1024;
    const ALPHABET: &[u8] = b"'\"\\:,|!& \t@";
    let seeds: [&[u8]; 6] = [
        b"SecAction \"deny\"",
        b"SecRule ARGS \"@rx x\" \"id:1\"",
        b"SecRule ARGS|BODY \"x\"",
        b"SecAction \"msg:'x,y'\"",
        b"SecRuleUpdateTargetById 1 ARGS",
        b"SecMarker END",
    ];
    let mut total = 0;
    for seed in seeds {
        for boundary in 0..=seed.len() {
            let mut variants = Vec::new();
            let mutation_index = boundary.min(seed.len() - 1);
            let mut deleted = seed.to_vec();
            deleted.remove(mutation_index);
            variants.push(deleted);
            let mut duplicated = seed.to_vec();
            duplicated.insert(boundary, seed[mutation_index]);
            variants.push(duplicated);
            for byte in ALPHABET {
                let mut inserted = seed.to_vec();
                inserted.insert(boundary, *byte);
                variants.push(inserted);
            }
            for mut variant in variants {
                variant.extend_from_slice(b"\nSecMarker MUTATION_END");
                total += variant.len();
                let source = scan_source("mutation", &variant, SourceLimits::default()).unwrap();
                if let Err(error) = parse_syntax(&source, SyntaxLimits::default()) {
                    assert_error_chain_redacted(&error, "MUTATION_END");
                }
            }
        }
    }
    assert!(total <= CAP, "mutation input cap exceeded: {total}");
}

fn assert_error_chain_redacted(error: &(dyn std::error::Error + 'static), needle: &str) {
    let mut current = Some(error);
    while let Some(item) = current {
        assert!(!item.to_string().contains(needle));
        assert!(!format!("{item:?}").contains(needle));
        current = item.source();
    }
}

#[test]
fn captured_coraza_3_7_0_fixtures_freeze_selected_common_and_divergent_forms() {
    let fixtures = [
        CorazaFixture {
            label: "common rule",
            source: b"SecRule ARGS \"@rx x\" \"id:1,deny\"",
            coraza: "accept: rule/operator/two actions",
            edgion: FixtureOutcome::Accept,
        },
        CorazaFixture {
            label: "even delimiter run",
            source: br#"SecAction "msg:a\\,""#,
            coraza: "accept: previous byte hides comma",
            edgion: FixtureOutcome::Reject,
        },
        CorazaFixture {
            label: "odd delimiter run",
            source: br#"SecAction "msg:a\\\,b,deny""#,
            coraza: "accept: previous byte hides comma",
            edgion: FixtureOutcome::ActionEntries(2),
        },
        CorazaFixture {
            label: "mid-value quote placement",
            source: b"SecAction \"msg:a'b,deny\"",
            coraza: "accept: quote toggles and captures comma",
            edgion: FixtureOutcome::ActionEntries(2),
        },
        CorazaFixture {
            label: "unclosed action quote",
            source: b"SecAction \"msg:'unclosed\"",
            coraza: "accept with warning",
            edgion: FixtureOutcome::Reject,
        },
        CorazaFixture {
            label: "quoted action suffix",
            source: b"SecAction \"msg:'closed'x\"",
            coraza: "accept: generic scanner keeps suffix",
            edgion: FixtureOutcome::Reject,
        },
        CorazaFixture {
            label: "Unicode action trim",
            source: "SecAction \"\u{a0},deny\"".as_bytes(),
            coraza: "reject: Unicode trim exposes empty entry",
            edgion: FixtureOutcome::ActionEntries(2),
        },
        CorazaFixture {
            label: "Unicode operator trim",
            source: "SecRule ARGS \"\u{a0}@rx x\u{a0}\"".as_bytes(),
            coraza: "accept: implicit rx with a Unicode-trimmed @rx x argument",
            edgion: FixtureOutcome::OperatorImplicitArgument("\u{a0}@rx x\u{a0}"),
        },
        CorazaFixture {
            label: "repeated target modifier",
            source: b"SecRule !!ARGS \"x\"",
            coraza: "accept: repeated exclusion is tolerated",
            edgion: FixtureOutcome::Reject,
        },
        CorazaFixture {
            label: "mixed target modifier",
            source: b"SecRule !&ARGS \"x\"",
            coraza: "accept: modifier scanner consumes both",
            edgion: FixtureOutcome::Reject,
        },
        CorazaFixture {
            label: "multi-ID update",
            source: b"SecRuleUpdateTargetById 1 2 ARGS",
            coraza: "syntax accepted; execution returns the first single-ID result and never visits later selectors",
            edgion: FixtureOutcome::UpdateIds(2),
        },
        CorazaFixture {
            label: "target regex",
            source: b"SecRule ARGS:/x/ \"x\"",
            coraza: "accept: regex selector",
            edgion: FixtureOutcome::Accept,
        },
    ];
    for fixture in fixtures {
        assert!(!fixture.label.is_empty());
        assert!(!fixture.coraza.is_empty());
        let scanned = scan_source(
            "coraza-3.7.0-27069d06c896be74b77b9a6c0b539a0cbfaca360",
            fixture.source,
            SourceLimits::default(),
        )
        .unwrap();
        let parsed = parse_syntax(&scanned, SyntaxLimits::default());
        match fixture.edgion {
            FixtureOutcome::Accept => assert!(parsed.is_ok(), "{}", fixture.label),
            FixtureOutcome::Reject => assert!(parsed.is_err(), "{}", fixture.label),
            FixtureOutcome::ActionEntries(expected) => {
                assert_eq!(
                    parsed.unwrap().get(0).unwrap().actions().unwrap().entries().len(),
                    expected
                )
            }
            FixtureOutcome::OperatorImplicitArgument(expected) => {
                let parsed = parsed.unwrap();
                let operator = parsed.get(0).unwrap().operator().unwrap();
                assert!(matches!(operator.name(), OperatorNameView::ImplicitRx));
                assert_eq!(operator.argument().raw(), expected);
            }
            FixtureOutcome::UpdateIds(expected) => {
                let parsed = parsed.unwrap();
                let DirectiveTargetView::Update { ids, .. } = parsed.get(0).unwrap().targets() else {
                    panic!("expected update fixture")
                };
                assert_eq!(ids.len(), expected);
            }
        }
    }
}

struct CorazaFixture {
    label: &'static str,
    source: &'static [u8],
    coraza: &'static str,
    edgion: FixtureOutcome,
}

enum FixtureOutcome {
    Accept,
    Reject,
    ActionEntries(usize),
    OperatorImplicitArgument(&'static str),
    UpdateIds(usize),
}

fn rule_files(root: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "conf"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[test]
fn pinned_crs_unified_public_syntax_gate_when_configured() {
    let Some(root) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") else {
        return;
    };
    let root = Path::new(&root);
    let files = rule_files(root);
    let mut directives = 0;
    let mut rule_targets = 0;
    let mut update_targets = 0;
    let mut max_atoms_per_expression = 0;
    let mut target_modifiers = [0_usize; 3];
    let mut selector_kinds = [0_usize; 4];
    let mut collection_names = BTreeMap::<String, usize>::new();
    let mut update_ids = 0;
    let mut operators = 0;
    let mut lists = 0;
    let mut entries = 0;
    let mut transforms = 0;
    let mut operator_names = BTreeMap::<String, usize>::new();
    let mut negated_operator_names = BTreeMap::<String, usize>::new();
    let mut implicit_operators = 0;
    let mut empty_operator_arguments = 0;
    let mut max_operator_raw = 0;
    let mut max_operator_decoded = 0;
    let mut operator_escape_total = 0;
    let mut max_operator_escapes = 0;
    let mut operator_macro_occurrences = 0;
    let mut decoded_operator_total = 0;
    let mut source_charge = 0;
    let mut tokenizer_charge = 0;
    let mut directive_charge = 0;
    let mut target_charge = 0;
    let mut operator_charge = 0;
    let mut action_charge = 0;
    let mut retained_charge = 0;
    let mut action_names = BTreeMap::<String, usize>::new();
    let mut transform_names = BTreeMap::<String, usize>::new();
    let mut single_quoted = 0;
    let mut unquoted = 0;
    let mut valueless = 0;
    let mut macro_values = 0;
    let mut macro_occurrences = 0;
    let mut escaped_single_values = 0;
    let mut escaped_single_occurrences = 0;
    let mut escaped_double_values = 0;
    let mut escaped_double_occurrences = 0;
    let mut max_entries_per_list = 0;
    let mut max_body = 0;
    let mut max_envelope = 0;
    let mut chains = 0;
    for path in &files {
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        let bytes = fs::read(path).unwrap();
        let source = scan_source(&relative, &bytes, SourceLimits::default()).unwrap();
        let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
        directives += syntax.len();
        update_ids += syntax.id_selector_count();
        operators += syntax.operator_count();
        decoded_operator_total += syntax.decoded_operator_bytes();
        lists += syntax.action_list_count();
        entries += syntax.action_entry_count();
        source_charge += syntax.source_charged_bytes();
        tokenizer_charge += syntax.tokenizer_charged_bytes();
        directive_charge += syntax.directive_charged_bytes();
        target_charge += syntax.target_charged_bytes();
        operator_charge += syntax.operator_charged_bytes();
        action_charge += syntax.action_charged_bytes();
        retained_charge += syntax.retained_charged_bytes();
        for directive in syntax.iter() {
            match directive.targets() {
                DirectiveTargetView::Rule { atoms } => {
                    rule_targets += atoms.len();
                    max_atoms_per_expression = max_atoms_per_expression.max(atoms.len());
                    for atom in atoms {
                        *collection_names.entry(atom.collection().text().to_owned()).or_default() += 1;
                        target_modifiers[match atom.modifier() {
                            TargetModifier::None => 0,
                            TargetModifier::Exclude => 1,
                            TargetModifier::Count => 2,
                        }] += 1;
                        selector_kinds[match atom.selector_kind() {
                            SelectorKind::None => 0,
                            SelectorKind::Literal => 1,
                            SelectorKind::Path => 2,
                            SelectorKind::Regex(_) => 3,
                        }] += 1;
                    }
                }
                DirectiveTargetView::Update { atoms, .. } => {
                    update_targets += atoms.len();
                    max_atoms_per_expression = max_atoms_per_expression.max(atoms.len());
                    for atom in atoms {
                        *collection_names.entry(atom.collection().text().to_owned()).or_default() += 1;
                    }
                }
                DirectiveTargetView::None | DirectiveTargetView::Remove { .. } => {}
            }
            if let Some(operator) = directive.operator() {
                let name = match operator.name() {
                    OperatorNameView::ImplicitRx => {
                        implicit_operators += 1;
                        "rx".to_owned()
                    }
                    OperatorNameView::Explicit(name) => String::from_utf8(name.bytes().collect()).unwrap(),
                };
                *operator_names.entry(name.clone()).or_default() += 1;
                if operator.negated() {
                    *negated_operator_names.entry(name).or_default() += 1;
                }
                empty_operator_arguments += usize::from(operator.argument().decoded_len() == 0);
                max_operator_raw = max_operator_raw.max(operator.content().raw().len());
                max_operator_decoded = max_operator_decoded.max(operator.content().decoded_len());
                let escapes = operator.content().raw().len() - operator.content().decoded_len();
                operator_escape_total += escapes;
                max_operator_escapes = max_operator_escapes.max(escapes);
                operator_macro_occurrences += operator.argument().raw().match_indices("%{").count();
            }
            if let Some(actions) = directive.actions() {
                max_entries_per_list = max_entries_per_list.max(actions.entries().len());
                for entry in actions.entries() {
                    *action_names.entry(entry.name().text().to_owned()).or_default() += 1;
                    chains += usize::from(entry.name().text().eq_ignore_ascii_case("chain"));
                    if entry.kind() == ActionEntryKind::Transformation {
                        transforms += 1;
                        if let Some(value) = entry.value() {
                            *transform_names.entry(value.body().text().to_owned()).or_default() += 1;
                        }
                    }
                    match entry.value() {
                        None => valueless += 1,
                        Some(value) => {
                            match value.spelling() {
                                crate::seclang::ActionValueSpelling::SingleQuoted => single_quoted += 1,
                                crate::seclang::ActionValueSpelling::Unquoted => unquoted += 1,
                            }
                            let body = value.body().text();
                            macro_values += usize::from(body.contains("%{"));
                            macro_occurrences += body.match_indices("%{").count();
                            escaped_single_values += usize::from(body.contains("\\'"));
                            escaped_single_occurrences += body.match_indices("\\'").count();
                            escaped_double_values += usize::from(body.contains("\\\""));
                            escaped_double_occurrences += body.match_indices("\\\"").count();
                            max_body = max_body.max(body.len());
                            max_envelope = max_envelope.max(value.envelope().text().len());
                        }
                    }
                }
            }
        }
    }
    eprintln!(
            "files={} directives={directives} rule_targets={rule_targets} update_targets={update_targets} update_ids={update_ids} operators={operators} lists={lists} entries={entries} transforms={transforms} charges={source_charge}/{tokenizer_charge}/{directive_charge}/{target_charge}/{operator_charge}/{action_charge}/{retained_charge} action_names={action_names:?} transform_names={transform_names:?}",
            files.len()
        );
    eprintln!("collection_names={collection_names:?}");
    assert_eq!(
        (
            files.len(),
            directives,
            rule_targets,
            update_targets,
            update_ids,
            operators,
            lists,
            entries,
            transforms
        ),
        (27, 786, 1_735, 55, 55, 693, 698, 8_975, 832)
    );
    assert_eq!(
        (source_charge, directive_charge, target_charge, action_charge),
        (1_043_945, 391_488, 572_408, 3_100_768)
    );
    assert_eq!(operator_charge, 247_368);
    assert_eq!(tokenizer_charge, 452_448);
    assert_eq!(retained_charge, 5_355_977);
    assert_eq!(max_atoms_per_expression, 10);
    assert_eq!(target_modifiers, [1_656, 28, 51]);
    assert_eq!(selector_kinds, [946, 432, 348, 9]);
    assert_eq!(operator_names, expected_operator_names());
    assert_eq!(negated_operator_names, expected_negated_operator_names());
    assert_eq!(implicit_operators, 0);
    assert_eq!(empty_operator_arguments, 7);
    assert_eq!((max_operator_raw, max_operator_decoded), (12_265, 12_110));
    assert_eq!((operator_escape_total, max_operator_escapes), (3_015, 971));
    assert_eq!(operator_macro_occurrences, 30);
    assert_eq!(decoded_operator_total, 198_298);
    assert_eq!(operator_record_size_for_test(), 304);
    assert_eq!(
        retained_charge,
        source_charge + directive_charge + target_charge + operator_charge + action_charge
    );
    assert_eq!(
        (max_entries_per_list, single_quoted, unquoted, valueless, chains),
        (29, 5_385, 2_315, 1_275, 73)
    );
    assert_eq!((macro_values, macro_occurrences), (966, 1_485));
    assert_eq!((escaped_single_values, escaped_single_occurrences), (2, 4));
    assert_eq!((escaped_double_values, escaped_double_occurrences), (1, 2));
    assert_eq!((max_body, max_envelope), (876, 878));
    assert_eq!(action_names, expected_action_names());
    assert_eq!(transform_names, expected_transform_names());
}

fn expected_action_names() -> BTreeMap<String, usize> {
    [
        ("auditlog", 1),
        ("block", 341),
        ("capture", 275),
        ("chain", 73),
        ("ctl", 18),
        ("deny", 6),
        ("id", 627),
        ("initcol", 2),
        ("log", 8),
        ("logdata", 327),
        ("msg", 351),
        ("multiMatch", 8),
        ("noauditlog", 5),
        ("nolog", 278),
        ("pass", 280),
        ("phase", 627),
        ("setvar", 678),
        ("severity", 342),
        ("skip", 1),
        ("skipAfter", 206),
        ("status", 2),
        ("t", 832),
        ("tag", 3_060),
        ("ver", 627),
    ]
    .into_iter()
    .map(|(name, count)| (name.to_owned(), count))
    .collect()
}

fn expected_operator_names() -> BTreeMap<String, usize> {
    [
        ("beginsWith", 1),
        ("contains", 4),
        ("detectSQLi", 2),
        ("detectXSS", 2),
        ("endsWith", 6),
        ("eq", 55),
        ("ge", 42),
        ("gt", 15),
        ("ipMatch", 2),
        ("lt", 190),
        ("pm", 12),
        ("pmFromFile", 21),
        ("rx", 314),
        ("streq", 9),
        ("unconditionalMatch", 1),
        ("validateByteRange", 6),
        ("validateUrlEncoding", 1),
        ("validateUtf8Encoding", 1),
        ("within", 9),
    ]
    .into_iter()
    .map(|(name, count)| (name.to_owned(), count))
    .collect()
}

fn expected_negated_operator_names() -> BTreeMap<String, usize> {
    [
        ("beginsWith", 1),
        ("endsWith", 3),
        ("eq", 4),
        ("lt", 1),
        ("pm", 2),
        ("pmFromFile", 1),
        ("rx", 21),
        ("streq", 2),
        ("validateByteRange", 1),
        ("within", 6),
    ]
    .into_iter()
    .map(|(name, count)| (name.to_owned(), count))
    .collect()
}

fn expected_transform_names() -> BTreeMap<String, usize> {
    [
        ("base64Decode", 3),
        ("cmdLine", 13),
        ("compressWhitespace", 5),
        ("cssDecode", 23),
        ("escapeSeqDecode", 7),
        ("hexEncode", 2),
        ("htmlEntityDecode", 35),
        ("jsDecode", 37),
        ("length", 3),
        ("lowercase", 46),
        ("none", 427),
        ("normalizePath", 8),
        ("normalizePathWin", 4),
        ("removeCommentsChar", 1),
        ("removeNulls", 27),
        ("removeWhitespace", 11),
        ("replaceComments", 9),
        ("sha1", 2),
        ("urlDecodeUni", 137),
        ("utf8toUnicode", 32),
    ]
    .into_iter()
    .map(|(name, count)| (name.to_owned(), count))
    .collect()
}
