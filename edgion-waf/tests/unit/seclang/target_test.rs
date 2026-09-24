use super::*;
use std::fs;
use std::mem::size_of;
use std::path::{Path, PathBuf};

use super::{
    parse_target_syntax, DirectiveTargetView, IdSelectorKind, ModifierErrorKind, RegexStyle, SelectorKind, TargetAtom,
    TargetModifier, TargetRecord, TargetSyntaxError, TargetSyntaxLimits,
};
use crate::seclang::{
    parse_directives, scan_source, tokenize_document, DirectiveLimits, SourceLimits, TokenizerLimits,
};

fn with_syntax(source: &[u8], inspect: impl FnOnce(&super::TargetSyntaxDocument<'_>)) {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let syntax = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    inspect(&syntax);
}

fn syntax_error(source: &[u8], limits: TargetSyntaxLimits) -> TargetSyntaxError {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    parse_target_syntax(&directives, limits).unwrap_err()
}

#[test]
fn parses_modifiers_literal_empty_path_and_regex_selectors() {
    with_syntax(
        br##"SecRule ARGS|!REQUEST_HEADERS:name|&FILES|COL:|XML:/a/b|json:/x|COL:/a|b\/c/|COL:'/d\/e/' """##,
        |syntax| {
            let record = syntax.iter().next().unwrap();
            let DirectiveTargetView::Rule { atoms } = record.targets() else {
                panic!("expected rule targets");
            };
            assert_eq!(atoms.len(), 8);
            assert_eq!(atoms[0].modifier(), TargetModifier::None);
            assert_eq!(atoms[0].selector_kind(), SelectorKind::None);
            assert_eq!(atoms[1].modifier(), TargetModifier::Exclude);
            assert_eq!(atoms[1].selector().unwrap().text(), "name");
            assert_eq!(atoms[2].modifier(), TargetModifier::Count);
            assert_eq!(atoms[3].selector_kind(), SelectorKind::Literal);
            assert_eq!(atoms[3].selector().unwrap().text(), "");
            assert_eq!(atoms[4].selector_kind(), SelectorKind::Path);
            assert_eq!(atoms[4].selector().unwrap().text(), "/a/b");
            assert_eq!(atoms[5].selector_kind(), SelectorKind::Path);
            assert_eq!(atoms[6].selector_kind(), SelectorKind::Regex(RegexStyle::Bare));
            assert_eq!(atoms[6].regex_body().unwrap().text(), "a|b\\/c");
            assert_eq!(atoms[7].selector_kind(), SelectorKind::Regex(RegexStyle::SingleQuoted));
            assert_eq!(atoms[7].regex_body().unwrap().text(), "d\\/e");
        },
    );
}

#[test]
fn regex_closure_uses_backslash_run_parity_and_accepts_empty_body() {
    with_syntax(b"SecRule A://|B:/x\\\\/|C:/x\\/y/ \"\"", |syntax| {
        let DirectiveTargetView::Rule { atoms } = syntax.iter().next().unwrap().targets() else {
            panic!("expected rule targets");
        };
        assert_eq!(atoms[0].regex_body().unwrap().text(), "");
        assert_eq!(atoms[1].regex_body().unwrap().text(), "x\\\\");
        assert_eq!(atoms[2].regex_body().unwrap().text(), "x\\/y");
    });
}

#[test]
fn rejects_empty_atoms_and_modifier_shapes() {
    for source in [&b"SecRule |ARGS \"\""[..], &b"SecRule ARGS| \"\""[..]] {
        assert!(matches!(
            syntax_error(source, TargetSyntaxLimits::default()),
            TargetSyntaxError::EmptyAtom { .. }
        ));
    }
    for (source, kind) in [
        (&b"SecRule !!ARGS \"\""[..], ModifierErrorKind::Repeated),
        (&b"SecRule !&ARGS \"\""[..], ModifierErrorKind::Mixed),
        (&b"SecRule &&ARGS \"\""[..], ModifierErrorKind::Repeated),
    ] {
        assert!(matches!(
            syntax_error(source, TargetSyntaxLimits::default()),
            TargetSyntaxError::InvalidModifier { kind: actual, .. } if actual == kind
        ));
    }
    assert!(matches!(
        syntax_error(b"SecRule ! \"\"", TargetSyntaxLimits::default()),
        TargetSyntaxError::EmptyCollection { .. }
    ));
}

#[test]
fn rejects_unclosed_regex_quote_and_closure_suffix_without_content() {
    for (source, expected) in [
        (&b"SecRule A:/private \"\""[..], "regex"),
        (&b"SecRule A:'/private/ \"\""[..], "quote"),
        (&b"SecRule A:/private/x \"\""[..], "suffix"),
    ] {
        let error = syntax_error(source, TargetSyntaxLimits::default());
        match expected {
            "regex" => assert!(matches!(error, TargetSyntaxError::UnclosedRegex { .. })),
            "quote" => assert!(matches!(error, TargetSyntaxError::UnclosedRegexQuote { .. })),
            _ => assert!(matches!(error, TargetSyntaxError::RegexSuffix { .. })),
        }
        assert!(!error.to_string().contains("private"));
        assert!(!format!("{error:?}").contains("private"));
    }
    assert!(matches!(
        syntax_error(
            br##"SecRuleUpdateTargetById 1 "A:/private\\""##,
            TargetSyntaxLimits::default()
        ),
        TargetSyntaxError::UnclosedRegex { .. }
    ));
}

#[test]
fn parses_update_ids_without_expanding_ranges_and_uses_quoted_content_ranges() {
    with_syntax(
        b"SecRuleUpdateTargetById 0 18446744073709551615 9-3 \"ARGS|XML:/x\"",
        |syntax| {
            assert_eq!(syntax.id_selector_count(), 3);
            let record = syntax.iter().next().unwrap();
            let DirectiveTargetView::Update { ids, atoms } = record.targets() else {
                panic!("expected update targets");
            };
            assert_eq!(ids[0].kind(), IdSelectorKind::Single(0));
            assert_eq!(ids[1].kind(), IdSelectorKind::Single(u64::MAX));
            assert_eq!(ids[2].kind(), IdSelectorKind::Range { start: 9, end: 3 });
            assert_eq!(ids[2].raw(), "9-3");
            assert_eq!(atoms.len(), 2);
            assert_eq!(atoms[0].collection().text(), "ARGS");
            assert_eq!(atoms[0].span().start.column, 53);
        },
    );
}

#[test]
fn parses_remove_ids_in_the_shared_flat_arena_without_targets() {
    let source = scan_source(
        "remove",
        b"SecRuleRemoveById 0 18446744073709551615 9-\\\n3",
        SourceLimits::default(),
    )
    .unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let exact_bytes = size_of::<TargetRecord>() + 3 * size_of::<super::IdSelector<'_>>();
    let syntax = parse_target_syntax(
        &directives,
        TargetSyntaxLimits {
            max_owned_bytes: exact_bytes,
            ..TargetSyntaxLimits::default()
        },
    )
    .unwrap();
    assert_eq!(syntax.charged_bytes(), exact_bytes);
    assert_eq!(syntax.atom_count(), 0);
    let DirectiveTargetView::Remove { ids } = syntax.iter().next().unwrap().targets() else {
        panic!("expected remove selector mapping")
    };
    assert_eq!(
        ids.iter().map(|id| id.kind()).collect::<Vec<_>>(),
        [
            IdSelectorKind::Single(0),
            IdSelectorKind::Single(u64::MAX),
            IdSelectorKind::Range { start: 9, end: 3 },
        ]
    );
    assert_eq!(ids[2].raw(), "9-3");
    assert_eq!((ids[2].span().start.line, ids[2].span().end.line), (1, 2));
    assert!(matches!(
        parse_target_syntax(
            &directives,
            TargetSyntaxLimits {
                max_owned_bytes: exact_bytes - 1,
                ..TargetSyntaxLimits::default()
            }
        ),
        Err(TargetSyntaxError::OwnedByteLimit { .. })
    ));
}

#[test]
fn rejects_every_malformed_id_shape_and_overflow() {
    for selector in ["+1", "-1", "1-", "1-2-3", "1a", "--"] {
        for source in [
            format!("SecRuleUpdateTargetById {selector} ARGS"),
            format!("SecRuleRemoveById {selector}"),
        ] {
            assert!(matches!(
                syntax_error(source.as_bytes(), TargetSyntaxLimits::default()),
                TargetSyntaxError::InvalidIdSelector { .. }
            ));
        }
    }
    assert!(matches!(
        syntax_error(
            b"SecRuleUpdateTargetById 18446744073709551616 ARGS",
            TargetSyntaxLimits::default()
        ),
        TargetSyntaxError::IdSelectorOverflow { .. }
    ));
    assert!(matches!(
        syntax_error(b"SecRuleRemoveById 18446744073709551616", TargetSyntaxLimits::default()),
        TargetSyntaxError::IdSelectorOverflow { .. }
    ));
}

#[test]
fn per_expression_and_per_update_limits_precede_total_limit() {
    let atom_limits = TargetSyntaxLimits {
        max_atoms_per_expression: 1,
        max_nodes: 1,
        ..TargetSyntaxLimits::default()
    };
    assert!(matches!(
        syntax_error(b"SecRule ARGS| \"\"", atom_limits),
        TargetSyntaxError::AtomLimit { limit: 1, .. }
    ));

    let id_limits = TargetSyntaxLimits {
        max_id_selectors_per_update: 1,
        max_nodes: 1,
        ..TargetSyntaxLimits::default()
    };
    assert!(matches!(
        syntax_error(b"SecRuleUpdateTargetById 1 + ARGS", id_limits),
        TargetSyntaxError::IdSelectorLimit { limit: 1, .. }
    ));
    assert!(matches!(
        syntax_error(b"SecRuleRemoveById 1 +", id_limits),
        TargetSyntaxError::IdSelectorLimit { limit: 1, .. }
    ));

    let node_limits = TargetSyntaxLimits {
        max_nodes: 0,
        ..TargetSyntaxLimits::default()
    };
    assert!(matches!(
        syntax_error(b"SecRule |ARGS \"\"", node_limits),
        TargetSyntaxError::NodeLimit { limit: 0, .. }
    ));
    assert!(matches!(
        syntax_error(b"SecRuleUpdateTargetById + ARGS", node_limits),
        TargetSyntaxError::NodeLimit { limit: 0, .. }
    ));
    assert!(matches!(
        syntax_error(b"SecRuleRemoveById +", node_limits),
        TargetSyntaxError::NodeLimit { limit: 0, .. }
    ));
}

#[test]
fn enforces_exact_owned_metadata_and_preserves_safe_flat_views() {
    let source = scan_source(
        "metadata",
        b"SecAction \"id:1\"\nSecRuleUpdateTargetById 1 ARGS|!ARGS:a",
        SourceLimits::default(),
    )
    .unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let exact_bytes =
        2 * size_of::<TargetRecord>() + 2 * size_of::<TargetAtom<'_>>() + size_of::<super::IdSelector<'_>>();
    let exact = TargetSyntaxLimits {
        max_owned_bytes: exact_bytes,
        ..TargetSyntaxLimits::default()
    };
    let syntax = parse_target_syntax(&directives, exact).unwrap();
    assert_eq!(syntax.charged_bytes(), exact_bytes);
    assert_eq!(syntax.source().source_id(), "metadata");
    assert_eq!(syntax.iter().count(), 2);
    assert_eq!(syntax.iter().nth(1).unwrap().logical_line_index(), 1);
    let DirectiveTargetView::Update { atoms, .. } = syntax.iter().nth(1).unwrap().targets() else {
        panic!("expected update targets");
    };
    assert_eq!(atoms[1].collection().logical_range(), 32..36);
    assert_eq!(atoms[1].collection().span(), atoms[1].collection().span());
    let first_range = atoms[0].collection().logical_range();
    assert!(std::ptr::eq(
        atoms[0].collection().text().as_ptr(),
        directives.source().lines()[1].text()[first_range].as_ptr()
    ));

    let short = TargetSyntaxLimits {
        max_owned_bytes: exact_bytes - 1,
        ..TargetSyntaxLimits::default()
    };
    assert!(matches!(
        parse_target_syntax(&directives, short),
        Err(TargetSyntaxError::OwnedByteLimit { .. })
    ));
}

#[test]
fn maps_utf8_and_continuation_join_positions() {
    let error = syntax_error("SecRule café| \"\"".as_bytes(), TargetSyntaxLimits::default());
    assert!(matches!(
        error,
        TargetSyntaxError::EmptyAtom {
            position: crate::seclang::SourcePosition { column: 15, .. }
        }
    ));

    with_syntax(b"SecRuleUpdateTargetById 1 \"ARGS|\\\n XML:/x\"", |syntax| {
        let DirectiveTargetView::Update { atoms, .. } = syntax.iter().next().unwrap().targets() else {
            panic!("expected update targets");
        };
        assert_eq!(atoms[1].span().start.line, 2);
        assert_eq!(atoms[0].span().end.line, 1);
    });
}

#[test]
fn whole_document_failure_publishes_no_partial_arena() {
    let error = syntax_error(
        b"SecRule ARGS \"\"\nSecRule PRIVATE:/unterminated \"\"",
        TargetSyntaxLimits::default(),
    );
    assert!(matches!(error, TargetSyntaxError::UnclosedRegex { .. }));
}

#[test]
fn pinned_crs_target_corpus_gate_when_configured() {
    let Ok(root) = std::env::var("EDGION_WAF_CRS_RULES_DIR") else {
        return;
    };
    let mut files = Vec::new();
    collect_conf_files(Path::new(&root), &mut files);
    files.sort();
    let file_count = files.len();
    let mut rule_atoms = 0_usize;
    let mut update_atoms = 0_usize;
    let mut update_ids = 0_usize;
    let mut max_atoms_per_expression = 0_usize;
    let mut modifiers = [0_usize; 3];
    let mut selector_kinds = [0_usize; 4];
    let mut directive_count = 0_usize;
    let mut directive_charged = 0_usize;
    let mut target_charged = 0_usize;
    for file in files {
        let bytes = fs::read(&file).expect("pinned CRS file must remain readable during the corpus test");
        let source_id = file.to_string_lossy();
        let source = scan_source(&source_id, &bytes, SourceLimits::default()).unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        let syntax = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
        directive_count += directives.len();
        directive_charged += directives.charged_bytes();
        target_charged += syntax.charged_bytes();
        for record in syntax.iter() {
            match record.targets() {
                DirectiveTargetView::None => {}
                DirectiveTargetView::Rule { atoms } => {
                    max_atoms_per_expression = max_atoms_per_expression.max(atoms.len());
                    rule_atoms += atoms.len();
                    for atom in atoms {
                        modifiers[match atom.modifier() {
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
                DirectiveTargetView::Update { ids, atoms } => {
                    max_atoms_per_expression = max_atoms_per_expression.max(atoms.len());
                    update_ids += ids.len();
                    update_atoms += atoms.len();
                }
                DirectiveTargetView::Remove { ids } => update_ids += ids.len(),
            }
        }
    }
    assert_eq!(file_count, 27);
    assert_eq!(directive_count, 786);
    eprintln!(
            "rule_atoms={rule_atoms} update_atoms={update_atoms} update_ids={update_ids} max_atoms_per_expression={max_atoms_per_expression} modifiers={modifiers:?} selectors={selector_kinds:?} directive_charged={directive_charged} target_charged={target_charged} token_size={} directive_record_size={} target_record_size={} target_atom_size={} id_selector_size={}",
            size_of::<crate::seclang::Token<'_>>(),
            (directive_charged - update_ids * size_of::<crate::seclang::Token<'_>>()) / directive_count,
            size_of::<TargetRecord>(),
            size_of::<TargetAtom<'_>>(),
            size_of::<super::IdSelector<'_>>(),
        );
    assert_eq!(rule_atoms, 1_735);
    assert_eq!(max_atoms_per_expression, 10);
    assert_eq!(modifiers, [1_656, 28, 51]);
    assert_eq!(selector_kinds, [946, 432, 348, 9]);
    assert_eq!(update_atoms, 55);
    assert_eq!(update_ids, 55);
}

fn collect_conf_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("configured CRS rules directory must be readable") {
        let entry = entry.expect("CRS directory entry must be readable");
        let path = entry.path();
        if path.is_dir() {
            collect_conf_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "conf") {
            files.push(path);
        }
    }
}

impl<'source> TargetSyntaxDocument<'source> {
    #[cfg(test)]
    pub(crate) fn remove_last_record_for_test(&mut self) {
        self.records.pop();
    }
    #[cfg(test)]
    pub(in crate::seclang) fn swap_records_for_test(&mut self, left: usize, right: usize) {
        self.records.swap(left, right);
    }
    #[cfg(test)]
    pub(in crate::seclang) fn set_first_record_none_for_test(&mut self) {
        if let Some(record) = self.records.first_mut() {
            record.kind = TargetRecordKind::None;
        }
    }
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = TargetRecordView<'_, 'source>> {
        self.records.iter().map(|record| {
            let targets = match record.kind {
                TargetRecordKind::None => DirectiveTargetView::None,
                TargetRecordKind::Rule { atom_start, atom_count } => DirectiveTargetView::Rule {
                    atoms: checked_slice(&self.atoms, atom_start, atom_count, "rule atom range must remain valid"),
                },
                TargetRecordKind::Update {
                    id_start,
                    id_count,
                    atom_start,
                    atom_count,
                } => DirectiveTargetView::Update {
                    ids: checked_slice(
                        &self.id_selectors,
                        id_start,
                        id_count,
                        "update ID range must remain valid",
                    ),
                    atoms: checked_slice(
                        &self.atoms,
                        atom_start,
                        atom_count,
                        "update atom range must remain valid",
                    ),
                },
                TargetRecordKind::Remove { id_start, id_count } => DirectiveTargetView::Remove {
                    ids: checked_slice(
                        &self.id_selectors,
                        id_start,
                        id_count,
                        "remove ID range must remain valid",
                    ),
                },
            };
            TargetRecordView {
                logical_line_index: record.logical_line_index,
                targets,
            }
        })
    }
}
