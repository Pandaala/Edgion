use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::mem::size_of;
use std::path::{Path, PathBuf};

use super::{
    parse_action_syntax, ActionEntry, ActionEntryKind, ActionListKind, ActionListRecord, ActionMapping,
    ActionMappingView, ActionSyntaxDocument, ActionSyntaxError, ActionSyntaxLimits, ActionValueSpelling,
    EmptyActionKind,
};
use crate::seclang::operator::{parse_operator_syntax, OperatorSyntaxLimits};
use crate::seclang::target::{parse_target_syntax, TargetSyntaxLimits};
use crate::seclang::{
    parse_directives, scan_source, tokenize_document, DirectiveLimits, SourceLimits, TokenizerLimits,
};

fn with_actions(source: &[u8], inspect: impl FnOnce(&ActionSyntaxDocument<'_>)) {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
    drop(tokenized);
    let actions = parse_action_syntax(operators, ActionSyntaxLimits::default(), usize::MAX).unwrap();
    inspect(&actions);
}

fn action_error(source: &[u8], limits: ActionSyntaxLimits) -> ActionSyntaxError {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
    match parse_action_syntax(operators, limits, usize::MAX).unwrap_err() {
        super::ActionBuildError::Syntax(error) => error,
        super::ActionBuildError::RetainedByteLimit => panic!("unexpected retained limit"),
    }
}

fn with_operator_document(source: &[u8], inspect: impl FnOnce(crate::seclang::operator::OperatorSyntaxDocument<'_>)) {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    inspect(parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap());
}

#[test]
fn distinguishes_absent_empty_rule_and_action_lists() {
    with_actions(
        b"SecRule ARGS \"@rx x\"\nSecRule ARGS \"@rx x\" \"\"\nSecAction \"deny\"",
        |document| {
            let mappings = document.iter().collect::<Vec<_>>();
            assert!(matches!(mappings[0], ActionMappingView::None));
            let ActionMappingView::List(empty) = mappings[1] else {
                panic!("expected explicit empty list")
            };
            assert_eq!(empty.kind(), ActionListKind::Rule);
            assert!(empty.entries().is_empty());
            let ActionMappingView::List(action) = mappings[2] else {
                panic!("expected SecAction list")
            };
            assert_eq!(action.kind(), ActionListKind::Action);
            assert_eq!(action.entries()[0].name().text(), "deny");
            assert_eq!(document.list_count(), 2);
            assert_eq!(document.entry_count(), 1);
        },
    );
}

#[test]
fn preserves_value_presence_spelling_case_duplicates_and_first_colon() {
    with_actions(
        br#"SecAction "Deny,id:1,tag:,msg:'a,b:c',empty:'',setvar:tx.a:1,T:none,t:lowercase,deny""#,
        |document| {
            let entries = document.lists().next().unwrap().entries();
            assert_eq!(entries.len(), 9);
            assert!(entries[0].value().is_none());
            assert_eq!(entries[2].value().unwrap().body().text(), "");
            assert_eq!(entries[2].value().unwrap().spelling(), ActionValueSpelling::Unquoted);
            assert_eq!(entries[3].value().unwrap().body().text(), "a,b:c");
            assert_eq!(
                entries[3].value().unwrap().spelling(),
                ActionValueSpelling::SingleQuoted
            );
            assert_eq!(entries[4].value().unwrap().body().text(), "");
            assert_eq!(entries[5].value().unwrap().body().text(), "tx.a:1");
            assert_eq!(entries[6].name().text(), "T");
            assert_eq!(entries[6].kind(), ActionEntryKind::Transformation);
            assert_eq!(entries[7].kind(), ActionEntryKind::Transformation);
            assert_eq!(entries[8].name().text(), "deny");
        },
    );
}

#[test]
fn trims_only_ascii_space_and_tab_and_keeps_macros_and_escapes() {
    with_actions(
        "SecAction \"  msg:  '%{tx.a}\\\"x'  , tag:\u{a0}x\u{a0} ,deny  \"".as_bytes(),
        |document| {
            let entries = document.lists().next().unwrap().entries();
            assert_eq!(entries[0].full().text(), "msg:  '%{tx.a}\\\"x'");
            assert_eq!(entries[0].value().unwrap().body().text(), "%{tx.a}\\\"x");
            assert_eq!(entries[1].value().unwrap().body().text(), "\u{a0}x\u{a0}");
            assert_eq!(entries[2].name().text(), "deny");
        },
    );
}

#[test]
fn delimiter_backslash_parity_is_exact_for_one_through_five() {
    for slashes in 1..=5 {
        let run = "\\".repeat(slashes);
        let source = format!(r#"SecAction "msg:a{run},b,deny""#);
        with_actions(source.as_bytes(), |document| {
            let entries = document.lists().next().unwrap().entries();
            if slashes % 2 == 1 {
                assert_eq!(entries.len(), 2, "slashes={slashes}");
                assert_eq!(entries[0].value().unwrap().body().text(), format!("a{run},b"));
            } else {
                assert_eq!(entries.len(), 3, "slashes={slashes}");
            }
        });

        let source = format!(r#"SecAction "name{run}:value,deny""#);
        with_actions(source.as_bytes(), |document| {
            let first = document.lists().next().unwrap().entries()[0];
            assert_eq!(first.value().is_some(), slashes % 2 == 0, "slashes={slashes}");
        });

        let source = format!(r#"SecAction "msg:'a{run}' ,deny""#);
        if slashes % 2 == 0 {
            with_actions(source.as_bytes(), |_| {});
        } else {
            assert!(matches!(
                action_error(source.as_bytes(), ActionSyntaxLimits::default()),
                ActionSyntaxError::UnclosedQuote { .. }
            ));
        }
    }
}

#[test]
fn unquoted_quote_is_ordinary_and_quoted_suffix_fails_closed() {
    with_actions(br#"SecAction "msg:a'b,deny""#, |document| {
        let entries = document.lists().next().unwrap().entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].value().unwrap().body().text(), "a'b");
    });
    assert!(matches!(
        action_error(br#"SecAction "msg:'abc""#, ActionSyntaxLimits::default()),
        ActionSyntaxError::UnclosedQuote { .. }
    ));
    assert!(matches!(
        action_error(br#"SecAction "msg:'abc'x""#, ActionSyntaxLimits::default()),
        ActionSyntaxError::QuotedValueSuffix { .. }
    ));
}

#[test]
fn rejects_leading_trailing_consecutive_empty_entries_and_empty_names() {
    for source in [
        br#"SecAction ",deny""#.as_slice(),
        br#"SecAction "deny,""#,
        br#"SecAction "deny,,log""#,
    ] {
        assert!(matches!(
            action_error(source, ActionSyntaxLimits::default()),
            ActionSyntaxError::Empty {
                kind: EmptyActionKind::Entry,
                ..
            }
        ));
    }
    assert!(matches!(
        action_error(br#"SecAction " :value""#, ActionSyntaxLimits::default()),
        ActionSyntaxError::Empty {
            kind: EmptyActionKind::Name,
            ..
        }
    ));
}

#[test]
fn retains_continuation_positions_ranges_and_source_after_tokenizer_drop() {
    let source = scan_source(
        "actions.conf",
        b"SecAction \\\n              \"msg:'caf\xc3\xa9',deny\"",
        SourceLimits::default(),
    )
    .unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
    drop(tokenized);
    let document = parse_action_syntax(operators, ActionSyntaxLimits::default(), usize::MAX).unwrap();
    let list = document.lists().next().unwrap();
    assert_eq!(list.logical_line_index(), 0);
    assert_eq!(list.span().start.line, 2);
    assert_eq!(list.logical_range(), 11..27);
    assert_eq!(list.entries()[0].value().unwrap().body().text(), "caf\u{e9}");
    assert_eq!(document.source().source_id(), "actions.conf");

    let source = scan_source(
        "joined-action.conf",
        b"SecAction \"msg:'caf\\\n\xc3\xa9',deny\"",
        SourceLimits::default(),
    )
    .unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
    let document = parse_action_syntax(operators, ActionSyntaxLimits::default(), usize::MAX).unwrap();
    let value = document.lists().next().unwrap().entries()[0].value().unwrap().body();
    assert_eq!(value.text(), "caf\u{e9}");
    assert_eq!(value.span().start.line, 1);
    assert_eq!(value.span().end.line, 2);
}

#[test]
fn enforces_exact_list_scanned_entry_and_owned_limits_in_precedence_order() {
    let base = ActionSyntaxLimits::default();
    assert!(matches!(
        action_error(br#"SecAction "deny""#, ActionSyntaxLimits { max_lists: 0, ..base }),
        ActionSyntaxError::ListLimit { .. }
    ));
    assert!(matches!(
        action_error(
            br#"SecAction "deny""#,
            ActionSyntaxLimits {
                max_scanned_bytes: 3,
                max_entries_per_list: 0,
                ..base
            }
        ),
        ActionSyntaxError::ScannedByteLimit { .. }
    ));
    assert!(matches!(
        action_error(
            br#"SecAction "deny,""#,
            ActionSyntaxLimits {
                max_entries_per_list: 1,
                max_entries: 1,
                ..base
            }
        ),
        ActionSyntaxError::ListEntryLimit { .. }
    ));
    assert!(matches!(
        action_error(
            br#"SecAction "deny,log""#,
            ActionSyntaxLimits {
                max_entries_per_list: 2,
                max_entries: 1,
                ..base
            }
        ),
        ActionSyntaxError::DocumentEntryLimit { .. }
    ));
    assert!(matches!(
        action_error(
            br#"SecAction "deny""#,
            ActionSyntaxLimits {
                max_owned_bytes: 0,
                ..base
            }
        ),
        ActionSyntaxError::MappingOwnedByteLimit { .. }
    ));
}

#[test]
fn accepts_every_exact_limit_and_rejects_each_first_excess_charge() {
    let expected_owned = size_of::<ActionMapping>() + size_of::<ActionEntry>() + size_of::<ActionListRecord>();
    with_operator_document(br#"SecAction "deny""#, |operators| {
        let inherited = operators.retained_charged_bytes().unwrap();
        let document = parse_action_syntax(
            operators,
            ActionSyntaxLimits {
                max_lists: 1,
                max_entries_per_list: 1,
                max_entries: 1,
                max_scanned_bytes: 4,
                max_owned_bytes: expected_owned,
            },
            inherited + expected_owned,
        )
        .unwrap();
        assert_eq!(document.charged_bytes(), expected_owned);
    });

    with_operator_document(br#"SecAction "deny""#, |operators| {
        let inherited = operators.retained_charged_bytes().unwrap();
        assert!(matches!(
            parse_action_syntax(
                operators,
                ActionSyntaxLimits::default(),
                inherited + size_of::<ActionMapping>() - 1,
            ),
            Err(super::ActionBuildError::RetainedByteLimit)
        ));
    });
    assert!(matches!(
        action_error(
            br#"SecAction "deny""#,
            ActionSyntaxLimits {
                max_owned_bytes: expected_owned - 1,
                ..ActionSyntaxLimits::default()
            }
        ),
        ActionSyntaxError::OwnedByteLimit { .. }
    ));
    assert!(matches!(
        super::ensure_cumulative(usize::MAX, 1, usize::MAX),
        Err(super::ActionBuildError::RetainedByteLimit)
    ));
}

#[test]
fn charged_sizes_are_exact_and_errors_do_not_echo_content() {
    with_actions(br#"SecAction "deny,msg:'secret-marker'""#, |document| {
        assert_eq!(
            document.charged_bytes(),
            size_of::<ActionMapping>() + size_of::<ActionListRecord>() + 2 * size_of::<ActionEntry>()
        );
        assert!(document.retained_charged_bytes() >= document.charged_bytes());
    });
    let error = action_error(br#"SecAction "secret-marker,""#, ActionSyntaxLimits::default());
    assert!(!error.to_string().contains("secret-marker"));
    assert!(!format!("{error:?}").contains("secret-marker"));
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
fn pinned_crs_action_inventory_when_configured() {
    let Some(root) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") else {
        return;
    };
    let files = rule_files(Path::new(&root));
    let mut directives_total = 0;
    let mut lists_total = 0;
    let mut entries_total = 0;
    let mut transformations = 0;
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
    let mut empty_entries = 0;
    let mut max_entries = 0;
    let mut max_body = 0;
    let mut max_body_entry = String::new();
    let mut max_body_envelope = 0;
    let mut chains = 0;
    let mut charged_total = 0;
    let mut retained_total = 0;
    for path in &files {
        let bytes = fs::read(path).unwrap();
        let source_id = path
            .file_name()
            .expect("pinned CRS rule path must have a file name")
            .to_string_lossy();
        let source = scan_source(&source_id, &bytes, SourceLimits::default()).unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        directives_total += directives.len();
        let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
        let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
        let actions = parse_action_syntax(operators, ActionSyntaxLimits::default(), usize::MAX).unwrap();
        charged_total += actions.charged_bytes();
        retained_total += actions.retained_charged_bytes();
        lists_total += actions.list_count();
        for list in actions.lists() {
            max_entries = max_entries.max(list.entries().len());
            for entry in list.entries() {
                entries_total += 1;
                *action_names.entry(entry.name().text().to_owned()).or_default() += 1;
                if entry.name().text().eq_ignore_ascii_case("chain") {
                    chains += 1;
                }
                if entry.full().text().is_empty() {
                    empty_entries += 1;
                }
                if entry.kind() == ActionEntryKind::Transformation {
                    transformations += 1;
                    if let Some(value) = entry.value() {
                        *transform_names.entry(value.body().text().to_owned()).or_default() += 1;
                    }
                }
                match entry.value() {
                    None => valueless += 1,
                    Some(value) => {
                        match value.spelling() {
                            ActionValueSpelling::SingleQuoted => single_quoted += 1,
                            ActionValueSpelling::Unquoted => unquoted += 1,
                        }
                        let body = value.body().text();
                        macro_values += usize::from(body.contains("%{"));
                        macro_occurrences += body.match_indices("%{").count();
                        escaped_single_values += usize::from(body.contains("\\'"));
                        escaped_single_occurrences += body.match_indices("\\'").count();
                        escaped_double_values += usize::from(body.contains("\\\""));
                        escaped_double_occurrences += body.match_indices("\\\"").count();
                        if body.len() > max_body {
                            max_body = body.len();
                            max_body_entry = entry.name().text().to_owned();
                            max_body_envelope = value.envelope().text().len();
                        }
                    }
                }
            }
        }
    }
    eprintln!("action_names={action_names:?}");
    eprintln!("transform_names={transform_names:?}");
    eprintln!("max_body_entry={max_body_entry} envelope={max_body_envelope}");
    eprintln!(
            "macro_values={macro_values} macro_occurrences={macro_occurrences} escaped_single_values={escaped_single_values} escaped_single_occurrences={escaped_single_occurrences} escaped_double_values={escaped_double_values} escaped_double_occurrences={escaped_double_occurrences}"
        );
    eprintln!(
            "sizes mapping={} list={} entry={} lists_total={lists_total} charged_total={charged_total} retained_total={retained_total}",
            size_of::<ActionMapping>(),
            size_of::<ActionListRecord>(),
            size_of::<ActionEntry>()
        );
    assert_eq!(files.len(), 27);
    assert_eq!(directives_total, 786);
    assert_eq!(entries_total, 8_975);
    assert_eq!(max_entries, 29);
    assert_eq!(transformations, 832);
    assert_eq!(lists_total, 698);
    assert_eq!(
        action_names,
        BTreeMap::from([
            ("auditlog".to_owned(), 1),
            ("block".to_owned(), 341),
            ("capture".to_owned(), 275),
            ("chain".to_owned(), 73),
            ("ctl".to_owned(), 18),
            ("deny".to_owned(), 6),
            ("id".to_owned(), 627),
            ("initcol".to_owned(), 2),
            ("log".to_owned(), 8),
            ("logdata".to_owned(), 327),
            ("msg".to_owned(), 351),
            ("multiMatch".to_owned(), 8),
            ("noauditlog".to_owned(), 5),
            ("nolog".to_owned(), 278),
            ("pass".to_owned(), 280),
            ("phase".to_owned(), 627),
            ("setvar".to_owned(), 678),
            ("severity".to_owned(), 342),
            ("skip".to_owned(), 1),
            ("skipAfter".to_owned(), 206),
            ("status".to_owned(), 2),
            ("t".to_owned(), 832),
            ("tag".to_owned(), 3_060),
            ("ver".to_owned(), 627),
        ])
    );
    assert_eq!(
        transform_names,
        BTreeMap::from([
            ("base64Decode".to_owned(), 3),
            ("cmdLine".to_owned(), 13),
            ("compressWhitespace".to_owned(), 5),
            ("cssDecode".to_owned(), 23),
            ("escapeSeqDecode".to_owned(), 7),
            ("hexEncode".to_owned(), 2),
            ("htmlEntityDecode".to_owned(), 35),
            ("jsDecode".to_owned(), 37),
            ("length".to_owned(), 3),
            ("lowercase".to_owned(), 46),
            ("none".to_owned(), 427),
            ("normalizePath".to_owned(), 8),
            ("normalizePathWin".to_owned(), 4),
            ("removeCommentsChar".to_owned(), 1),
            ("removeNulls".to_owned(), 27),
            ("removeWhitespace".to_owned(), 11),
            ("replaceComments".to_owned(), 9),
            ("sha1".to_owned(), 2),
            ("urlDecodeUni".to_owned(), 137),
            ("utf8toUnicode".to_owned(), 32),
        ])
    );
    assert_eq!((single_quoted, unquoted, valueless), (5_385, 2_315, 1_275));
    assert_eq!((macro_values, macro_occurrences), (966, 1_485));
    assert_eq!((escaped_single_values, escaped_single_occurrences), (2, 4));
    assert_eq!((escaped_double_values, escaped_double_occurrences), (1, 2));
    assert_eq!((empty_entries, max_body, max_body_envelope, chains), (0, 876, 878, 73));
    assert_eq!(
        (
            size_of::<ActionMapping>(),
            size_of::<ActionListRecord>(),
            size_of::<ActionEntry>()
        ),
        (16, 104, 336)
    );
    assert_eq!(charged_total, 3_100_768);
    assert_eq!(retained_total, 5_355_977);
}

impl<'source> ActionSyntaxDocument<'source> {
    #[cfg(test)]
    pub(super) fn iter(&self) -> impl Iterator<Item = ActionMappingView<'_, 'source>> {
        self.mappings.iter().map(|mapping| match *mapping {
            ActionMapping::None => ActionMappingView::None,
            ActionMapping::List(index) => {
                let record = self
                    .lists
                    .get(index)
                    .expect("private action mapping must remain valid in an immutable document");
                let entries = checked_slice(
                    &self.entries,
                    record.entry_start,
                    record.entry_count,
                    "private action entry range must remain valid",
                );
                ActionMappingView::List(ActionListView { record, entries })
            }
        })
    }
    #[cfg(test)]
    pub(in crate::seclang) fn operators_mut_for_test(&mut self) -> &mut OperatorSyntaxDocument<'source> {
        &mut self.operators
    }
    #[cfg(test)]
    pub(in crate::seclang) fn swap_mappings_for_test(&mut self, left: usize, right: usize) {
        self.mappings.swap(left, right);
    }
    #[cfg(test)]
    pub(in crate::seclang) fn set_first_list_kind_for_test(&mut self, kind: ActionListKind) {
        if let Some(list) = self.lists.first_mut() {
            list.kind = kind;
        }
    }
    #[cfg(test)]
    pub(in crate::seclang) fn set_first_mapping_none_for_test(&mut self) {
        if let Some(mapping) = self.mappings.first_mut() {
            *mapping = ActionMapping::None;
        }
    }
    #[cfg(test)]
    pub(super) fn lists(&self) -> impl Iterator<Item = ActionListView<'_, 'source>> {
        self.lists.iter().map(|record| ActionListView {
            entries: checked_slice(
                &self.entries,
                record.entry_start,
                record.entry_count,
                "private action entry range must remain valid",
            ),
            record,
        })
    }
}
