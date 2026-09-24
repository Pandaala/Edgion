use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::mem::size_of;
use std::path::{Path, PathBuf};

use super::{
    parse_operator_syntax, DecodedTextView, OperatorMapping, OperatorMappingView, OperatorRecord, OperatorSpelling,
    OperatorSyntaxError, OperatorSyntaxLimits,
};
use crate::seclang::target::{parse_target_syntax, TargetSyntaxLimits};
use crate::seclang::{
    parse_directives, scan_source, tokenize_document, DirectiveLimits, SourceLimits, TokenizerLimits,
};

fn with_operators(source: &[u8], inspect: impl FnOnce(&super::OperatorSyntaxDocument<'_>)) {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
    inspect(&operators);
}

fn operator_error(source: &[u8], limits: OperatorSyntaxLimits) -> OperatorSyntaxError {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    parse_operator_syntax(directives, targets, limits).unwrap_err()
}

fn decoded(view: DecodedTextView<'_, '_>) -> String {
    String::from_utf8(view.bytes().collect()).unwrap()
}

#[test]
fn decoded_nonempty_argument_ranges_map_escape_and_utf8_boundaries_directionally() {
    with_operators(b"SecRule ARGS \"@eq \xc3\xa9\\\"x\" \"id:1\"\n", |document| {
        let OperatorMappingView::Rule(operator) = document.get(0).unwrap() else {
            panic!("rule")
        };
        let start = operator.argument_logical_range().start;
        assert_eq!(
            operator.map_argument_decoded_nonempty_range(0..2),
            Some(start..start + 2)
        );
        assert_eq!(
            operator.map_argument_decoded_nonempty_range(2..3),
            Some(start + 2..start + 4)
        );
        assert_eq!(
            operator.map_argument_decoded_nonempty_range(3..4),
            Some(start + 4..start + 5)
        );
        assert_eq!(operator.map_argument_decoded_nonempty_range(0..0), None);
        assert_eq!(operator.map_argument_decoded_nonempty_range(1..2), None);
        assert_eq!(operator.map_argument_decoded_nonempty_range(4..5), None);
    });
}

#[test]
fn decoded_range_mapping_uses_identity_or_logarithmic_escape_lookup() {
    with_operators(b"SecRule ARGS \"@eq plain%{TX.key}\" \"id:1\"\n", |document| {
        let operator = document.operators().next().unwrap();
        assert_eq!(operator.argument_mapping_work_for_test(5..14), Some(0));
    });

    let literal_prefix_len = 100_000;
    let escape_count = 4_096;
    let expansion_count = 1_024;
    let mut argument = "x".repeat(literal_prefix_len);
    argument.push_str(&"\\\"".repeat(escape_count));
    argument.push_str(&"%{TX.k}".repeat(expansion_count));
    let source = format!("SecRule ARGS \"@eq {argument}\" \"id:1\"\n");
    with_operators(source.as_bytes(), |document| {
        let operator = document.operators().next().unwrap();
        let first_expansion = literal_prefix_len + escape_count;
        let mut inspected = 0;
        for index in 0..expansion_count {
            let start = first_expansion + index * 7;
            inspected += operator.argument_mapping_work_for_test(start..start + 7).unwrap();
        }
        // Two binary searches per expansion inspect at most 13 entries each
        // for 4,096 escapes. This structural bound catches boundary rescans
        // without relying on machine-dependent timing.
        assert!(inspected <= expansion_count * 26, "inspected={inspected}");
        let final_start = first_expansion + (expansion_count - 1) * 7;
        assert!(operator
            .map_argument_decoded_nonempty_range(final_start..final_start + 7)
            .is_some());
    });
}

fn decoded_name(view: super::OperatorNameView<'_, '_>) -> String {
    match view {
        super::OperatorNameView::ImplicitRx => String::new(),
        super::OperatorNameView::Explicit(value) => decoded(value),
    }
}

#[test]
fn classifies_every_implicit_negated_and_explicit_edge_form() {
    let cases = [
        ("", OperatorSpelling::ImplicitRx, false, "", ""),
        ("abc", OperatorSpelling::ImplicitRx, false, "", "abc"),
        ("!", OperatorSpelling::ImplicitRx, true, "", ""),
        ("!abc", OperatorSpelling::ImplicitRx, true, "", "abc"),
        ("!!abc", OperatorSpelling::ImplicitRx, true, "", "!abc"),
        ("@", OperatorSpelling::Explicit, false, "", ""),
        ("!@", OperatorSpelling::Explicit, false, "!@", ""),
        ("@rx", OperatorSpelling::Explicit, false, "rx", ""),
        ("!@rx", OperatorSpelling::Explicit, true, "rx", ""),
    ];
    for (raw, spelling, negated, name, argument) in cases {
        let source = format!("SecRule ARGS \"{raw}\"");
        with_operators(source.as_bytes(), |document| {
            let operator = document.operators().next().unwrap();
            assert_eq!(operator.spelling(), spelling, "{raw}");
            assert_eq!(operator.negated(), negated, "{raw}");
            assert_eq!(decoded_name(operator.name()), name, "{raw}");
            assert_eq!(decoded(operator.argument()), argument, "{raw}");
        });
    }
}

#[test]
fn splits_only_first_ascii_space_and_trims_ascii_edges() {
    for (raw, name, argument) in [
        ("@rx first second", "rx", "first second"),
        ("  @rx   value  ", "", "@rx   value"),
        ("\t@rx value\t", "", "@rx value"),
        (" !", "", "!"),
        ("!  abc", "", "abc"),
        ("@rx\tvalue", "rx\tvalue", ""),
        ("@rx \t value \t", "rx", "value"),
        ("@MiXeD value", "MiXeD", "value"),
        ("\u{a0}@rx value\u{a0}", "", "\u{a0}@rx value\u{a0}"),
    ] {
        let source = format!("SecRule ARGS \"{raw}\"");
        with_operators(source.as_bytes(), |document| {
            let operator = document.operators().next().unwrap();
            assert_eq!(decoded_name(operator.name()), name, "{raw:?}");
            assert_eq!(decoded(operator.argument()), argument, "{raw:?}");
        });
    }
}

#[test]
fn virtual_decode_matches_coraza_for_odd_runs_and_preserves_other_escapes() {
    for slashes in [1, 3, 5] {
        let run = "\\".repeat(slashes);
        let source = format!(r##"SecRule ARGS "@rx a{run}"b\s\x41""##);
        with_operators(source.as_bytes(), |document| {
            let operator = document.operators().next().unwrap();
            let expected = format!("a{}\"b\\s\\x41", "\\".repeat(slashes - 1));
            let argument = operator.argument();
            assert_eq!(decoded(argument), expected);
            assert_eq!(argument.decoded_len(), expected.len());
            assert!(argument.as_borrowed().is_none());
            assert_eq!(argument.chunks().collect::<String>(), expected);
        });
    }
}

#[test]
fn even_runs_close_the_outer_token_and_keep_the_fast_path() {
    for slashes in [0, 2, 4] {
        let run = "\\".repeat(slashes);
        let source = format!("SecRule ARGS \"@rx value{run}\"");
        with_operators(source.as_bytes(), |document| {
            let argument = document.operators().next().unwrap().argument();
            assert_eq!(argument.as_borrowed(), Some(format!("value{run}").as_str()));
            assert_eq!(argument.bytes().collect::<Vec<_>>(), format!("value{run}").into_bytes());
        });
    }
}

#[test]
fn escaped_quotes_can_remain_in_names_and_subviews_have_independent_fast_paths() {
    with_operators(br##"SecRule ARGS "@na\"me plain""##, |document| {
        let operator = document.operators().next().unwrap();
        assert_eq!(decoded_name(operator.name()), "na\"me");
        assert!(matches!(operator.name(), super::OperatorNameView::Explicit(value) if value.as_borrowed().is_none()));
        assert_eq!(operator.argument().as_borrowed(), Some("plain"));
        assert_eq!(decoded(operator.explicit_spelling().unwrap()), "@na\"me plain");
    });
}

#[test]
fn preserves_macro_text_continuation_spans_ranges_and_source_lifetime() {
    let source = scan_source(
        "operators.conf",
        b"SecRule ARGS \x5c\x0a \"@rx %{tx.name}\"",
        SourceLimits::default(),
    )
    .unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
    drop(tokenized);
    let operator = operators.operators().next().unwrap();
    assert_eq!(decoded(operator.argument()), "%{tx.name}");
    assert_eq!(operator.content_span().start.line, 2);
    assert_eq!(operator.content_logical_range(), 14..28);
    assert_eq!(operator.logical_line_index(), 0);
    assert_eq!(operators.source().source_id(), "operators.conf");
    assert_eq!(operators.targets().record_count(), 1);
}

#[test]
fn mapping_views_preserve_non_rules_and_rule_order() {
    with_operators(b"SecMarker A\nSecRule ARGS \"@rx x\"\nSecAction \"id:1\"", |document| {
        let views = document.iter().collect::<Vec<_>>();
        assert!(matches!(views[0], OperatorMappingView::None));
        assert!(matches!(views[1], OperatorMappingView::Rule(_)));
        assert!(matches!(views[2], OperatorMappingView::None));
        assert_eq!(document.operator_count(), 1);
    });
}

#[test]
fn rejects_source_and_record_count_mismatches_before_allocation() {
    let source_a = scan_source("a", b"SecRule ARGS \"x\"", SourceLimits::default()).unwrap();
    let tokens_a = tokenize_document(&source_a, TokenizerLimits::default()).unwrap();
    let directives_a = parse_directives(&tokens_a, DirectiveLimits::default()).unwrap();
    let targets_a = parse_target_syntax(&directives_a, TargetSyntaxLimits::default()).unwrap();

    let source_b = scan_source("b", b"SecRule ARGS \"x\"", SourceLimits::default()).unwrap();
    let tokens_b = tokenize_document(&source_b, TokenizerLimits::default()).unwrap();
    let directives_b = parse_directives(&tokens_b, DirectiveLimits::default()).unwrap();
    let targets_b = parse_target_syntax(&directives_b, TargetSyntaxLimits::default()).unwrap();
    assert!(matches!(
        parse_operator_syntax(directives_a, targets_b, OperatorSyntaxLimits::default()),
        Err(OperatorSyntaxError::SourceMismatch)
    ));

    let mut shortened = targets_a;
    shortened.remove_last_record_for_test();
    assert!(matches!(
        parse_operator_syntax(directives_b, shortened, OperatorSyntaxLimits::default()),
        Err(OperatorSyntaxError::SourceMismatch)
    ));

    let source_c = scan_source("c", b"SecRule ARGS \"x\"", SourceLimits::default()).unwrap();
    let tokens_c = tokenize_document(&source_c, TokenizerLimits::default()).unwrap();
    let directives_c = parse_directives(&tokens_c, DirectiveLimits::default()).unwrap();
    let mut targets_c = parse_target_syntax(&directives_c, TargetSyntaxLimits::default()).unwrap();
    targets_c.remove_last_record_for_test();
    assert!(matches!(
        parse_operator_syntax(directives_c, targets_c, OperatorSyntaxLimits::default()),
        Err(OperatorSyntaxError::RecordCountMismatch { .. })
    ));
}

#[test]
fn enforces_mapping_operator_escape_decoded_and_owned_limits_in_order() {
    let mapping = OperatorSyntaxLimits {
        max_owned_bytes: 0,
        ..OperatorSyntaxLimits::default()
    };
    assert!(matches!(
        operator_error(b"SecRule ARGS \"x\"", mapping),
        OperatorSyntaxError::MappingOwnedByteLimit { .. }
    ));

    let operator = OperatorSyntaxLimits {
        max_rule_operators: 1,
        ..OperatorSyntaxLimits::default()
    };
    assert!(matches!(
        operator_error(b"SecRule ARGS \"x\"\nSecRule ARGS \"y\"", operator),
        OperatorSyntaxError::OperatorLimit { .. }
    ));

    let source = br##"SecRule ARGS "@rx a\"b""##;
    let per_escape = OperatorSyntaxLimits {
        max_escapes_per_operator: 0,
        max_document_escapes: 0,
        max_decoded_bytes: 0,
        ..OperatorSyntaxLimits::default()
    };
    assert!(matches!(
        operator_error(source, per_escape),
        OperatorSyntaxError::OperatorEscapeLimit { .. }
    ));
    let document_escape = OperatorSyntaxLimits {
        max_escapes_per_operator: 1,
        max_document_escapes: 0,
        max_decoded_bytes: 0,
        ..OperatorSyntaxLimits::default()
    };
    assert!(matches!(
        operator_error(source, document_escape),
        OperatorSyntaxError::DocumentEscapeLimit { .. }
    ));
    let decoded_limit = OperatorSyntaxLimits {
        max_escapes_per_operator: 1,
        max_document_escapes: 1,
        max_decoded_bytes: 6,
        ..OperatorSyntaxLimits::default()
    };
    assert!(matches!(
        operator_error(source, decoded_limit),
        OperatorSyntaxError::DecodedByteLimit { .. }
    ));

    let exact_owned = size_of::<OperatorMapping>() + size_of::<OperatorRecord<'_>>() + size_of::<usize>();
    let exact = OperatorSyntaxLimits {
        max_escapes_per_operator: 1,
        max_document_escapes: 1,
        max_decoded_bytes: 7,
        max_owned_bytes: exact_owned,
        ..OperatorSyntaxLimits::default()
    };
    let source_doc = scan_source("exact", source, SourceLimits::default()).unwrap();
    let tokens = tokenize_document(&source_doc, TokenizerLimits::default()).unwrap();
    let directives = parse_directives(&tokens, DirectiveLimits::default()).unwrap();
    let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
    let parsed = parse_operator_syntax(directives, targets, exact).unwrap();
    assert_eq!(parsed.charged_bytes(), exact_owned);
    assert_eq!(parsed.decoded_bytes(), 7);
    let short = OperatorSyntaxLimits {
        max_owned_bytes: exact_owned - 1,
        ..exact
    };
    assert!(matches!(
        operator_error(source, short),
        OperatorSyntaxError::OwnedByteLimit { .. }
    ));
}

#[test]
fn errors_never_echo_operator_content() {
    let limits = OperatorSyntaxLimits {
        max_escapes_per_operator: 0,
        ..OperatorSyntaxLimits::default()
    };
    let error = operator_error(br##"SecRule ARGS "private\"value""##, limits);
    assert!(!error.to_string().contains("private"));
    assert!(!format!("{error:?}").contains("private"));
}

#[test]
fn pinned_crs_operator_corpus_gate_when_configured() {
    let Ok(root) = std::env::var("EDGION_WAF_CRS_RULES_DIR") else {
        return;
    };
    let mut files = Vec::new();
    collect_conf_files(Path::new(&root), &mut files);
    files.sort();
    let file_count = files.len();
    let mut directive_count = 0_usize;
    let mut operator_count = 0_usize;
    let mut names = BTreeMap::<String, usize>::new();
    let mut negated_names = BTreeMap::<String, usize>::new();
    let mut implicit = 0_usize;
    let mut empty_arguments = 0_usize;
    let mut max_raw = 0_usize;
    let mut max_decoded = 0_usize;
    let mut escape_total = 0_usize;
    let mut max_escapes = 0_usize;
    let mut macro_count = 0_usize;
    let mut decoded_total = 0_usize;
    let mut owned_total = 0_usize;
    for file in files {
        let bytes = fs::read(&file).expect("pinned CRS file must remain readable during the corpus test");
        let source_id = file.to_string_lossy();
        let source = scan_source(&source_id, &bytes, SourceLimits::default()).unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        directive_count += directives.len();
        let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
        let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
        operator_count += operators.operator_count();
        decoded_total += operators.decoded_bytes();
        owned_total += operators.charged_bytes();
        for operator in operators.operators() {
            let name = decoded_name(operator.name());
            *names.entry(name.clone()).or_default() += 1;
            if operator.negated() {
                *negated_names.entry(name).or_default() += 1;
            }
            implicit += usize::from(operator.spelling() == OperatorSpelling::ImplicitRx);
            empty_arguments += usize::from(operator.argument().decoded_len() == 0);
            max_raw = max_raw.max(operator.content().raw().len());
            max_decoded = max_decoded.max(operator.content().decoded_len());
            let escapes = operator.content().raw().len() - operator.content().decoded_len();
            escape_total += escapes;
            max_escapes = max_escapes.max(escapes);
            macro_count += operator.argument().raw().match_indices("%{").count();
        }
    }
    eprintln!(
            "directives={directive_count} operators={operator_count} names={names:?} negated={negated_names:?} implicit={implicit} empty_arguments={empty_arguments} max_raw={max_raw} max_decoded={max_decoded} escapes={escape_total} max_escapes={max_escapes} macros={macro_count} decoded_total={decoded_total} owned_total={owned_total} mapping_size={} operator_record_size={}",
            size_of::<OperatorMapping>(),
            size_of::<OperatorRecord<'_>>()
        );
    assert_eq!(file_count, 27);
    assert_eq!(directive_count, 786);
    assert_eq!(operator_count, 693);
    assert_eq!(implicit, 0);
    assert_eq!(empty_arguments, 7);
    assert_eq!(max_raw, 12_265);
    assert_eq!(max_decoded, 12_110);
    assert_eq!(escape_total, 3_015);
    assert_eq!(max_escapes, 971);
    assert_eq!(macro_count, 30);
    assert_eq!(negated_names, expected_negated_name_counts());
    assert_eq!(decoded_total, 198_298);
    assert_eq!(owned_total, 247_368);
    assert_eq!(size_of::<OperatorRecord>(), 304);
    assert_eq!(names, expected_name_counts());
}

fn expected_name_counts() -> BTreeMap<String, usize> {
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

fn expected_negated_name_counts() -> BTreeMap<String, usize> {
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

impl<'source> OperatorSyntaxDocument<'source> {
    #[cfg(test)]
    pub(super) fn iter(&self) -> impl Iterator<Item = OperatorMappingView<'_, 'source>> {
        self.mappings.iter().map(|mapping| match *mapping {
            OperatorMapping::None => OperatorMappingView::None,
            OperatorMapping::Rule(index) => {
                let record = self
                    .operators
                    .get(index)
                    .expect("private operator mapping must remain valid in an immutable document");
                OperatorMappingView::Rule(operator_view(record, &self.escape_offsets))
            }
        })
    }
    #[cfg(test)]
    pub(in crate::seclang) fn targets_mut_for_test(&mut self) -> &mut TargetSyntaxDocument<'source> {
        &mut self.targets
    }
    #[cfg(test)]
    pub(in crate::seclang) fn set_first_mapping_none_for_test(&mut self) {
        if let Some(mapping) = self.mappings.first_mut() {
            *mapping = OperatorMapping::None;
        }
    }
    #[cfg(test)]
    pub(in crate::seclang) fn shift_first_operator_line_for_test(&mut self) {
        if let Some(operator) = self.operators.first_mut() {
            operator.logical_line_index = operator.logical_line_index.saturating_add(1);
        }
    }
    #[cfg(test)]
    pub(super) fn operators(&self) -> impl Iterator<Item = OperatorView<'_, 'source>> {
        self.operators
            .iter()
            .map(|record| operator_view(record, &self.escape_offsets))
    }
}

impl<'view, 'source> OperatorView<'view, 'source> {
    #[cfg(test)]
    pub(in crate::seclang) fn argument_mapping_work_for_test(&self, range: Range<usize>) -> Option<usize> {
        if range.start >= range.end || range.end > self.argument.decoded_len() {
            return None;
        }
        let (_, start_work) = self.argument.raw_boundary_for_decoded_offset_with_work(range.start)?;
        let (_, end_work) = self.argument.raw_boundary_for_decoded_offset_with_work(range.end)?;
        Some(start_work + end_work)
    }
}
