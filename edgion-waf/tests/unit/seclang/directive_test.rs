use std::mem::size_of;

use super::{
    parse_directives, Directive, DirectiveArity, DirectiveField, DirectiveLimits, DirectiveParseError, DirectiveRecord,
    QuoteRequirement,
};
use crate::seclang::{scan_source, tokenize_document, QuoteStyle, SourceLimits, TokenizerLimits};

fn with_parsed(source: &[u8], inspect: impl FnOnce(&super::DirectiveDocument<'_>)) {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let parsed = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    inspect(&parsed);
}

fn parse_error(source: &[u8]) -> DirectiveParseError {
    let source = scan_source("test", source, SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    parse_directives(&tokenized, DirectiveLimits::default()).unwrap_err()
}

#[test]
fn parses_all_variants_case_insensitively_in_source_order() {
    with_parsed(
            b"sEcRuLe ARGS \"@rx x\" \"id:1\"\nSECACTION \"id:2\"\nSecMarker START\nSecComponentSignature \"OWASP_CRS/4\"\nsecruleupdatetargetbyid 1 2 \"!ARGS:a\"\nSECRULEREMOVEBYID 3 4-5",
            |parsed| {
                let views = parsed.iter().collect::<Vec<_>>();
                assert_eq!(views.len(), 6);
                assert!(matches!(views[0].directive(), Directive::SecRule { .. }));
                assert!(matches!(views[1].directive(), Directive::SecAction { .. }));
                assert!(matches!(views[2].directive(), Directive::SecMarker { .. }));
                assert!(matches!(views[3].directive(), Directive::SecComponentSignature { .. }));
                assert!(matches!(
                    views[4].directive(),
                    Directive::SecRuleUpdateTargetById { selectors, .. } if selectors.len() == 2
                ));
                assert!(matches!(
                    views[5].directive(),
                    Directive::SecRuleRemoveById { selectors } if selectors.len() == 2
                ));
                assert_eq!(
                    views.iter().map(|view| view.logical_line_index()).collect::<Vec<_>>(),
                    [0, 1, 2, 3, 4, 5]
                );
            },
        );
}

#[test]
fn preserves_rule_action_presence_and_empty_operator_forms() {
    with_parsed(b"SecRule ARGS \"\"\nSecRule ARGS \"@rx x\" \"\"", |parsed| {
        let mut directives = parsed.iter().map(|view| view.directive());
        assert!(matches!(
            directives.next(),
            Some(Directive::SecRule { operator, actions: None, .. }) if operator.value().is_empty()
        ));
        assert!(matches!(
            directives.next(),
            Some(Directive::SecRule { actions: Some(actions), .. }) if actions.value().is_empty()
        ));
    });
}

#[test]
fn retains_unquoted_multiword_marker_as_one_borrowed_tail() {
    with_parsed(b"SecMarker alpha\t beta gamma", |parsed| {
        let view = parsed.iter().next().unwrap();
        assert_eq!(view.span(), parsed.source().lines()[0].span());
        assert!(matches!(
            view.directive(),
            Directive::SecMarker { marker }
                if marker.value() == "alpha\t beta gamma" && marker.quote_style() == QuoteStyle::Unquoted
        ));
    });
}

#[test]
fn retains_continued_utf8_marker_tail_after_an_empty_first_contribution() {
    with_parsed("\\\nSecMarker café alpha".as_bytes(), |parsed| {
        let view = parsed.iter().next().unwrap();
        assert_eq!(view.span().start.line, 1);
        assert_eq!(view.span().end.line, 2);
        assert!(matches!(
            view.directive(),
            Directive::SecMarker { marker }
                if marker.value() == "café alpha"
                    && marker.span().start.line == 2
                    && marker.span().end.line == 2
        ));
    });
}

#[test]
fn accepts_marker_and_update_target_quote_profiles() {
    with_parsed(
            b"SecMarker \"quoted\"\nSecMarker plain\nSecRuleUpdateTargetById 1 ARGS:a\nSecRuleUpdateTargetById 2 \"!ARGS:b\"",
            |parsed| assert_eq!(parsed.len(), 4),
        );
}

#[test]
fn enforces_every_directive_arity_shape() {
    for (source, expected) in [
        (&b"SecRule ARGS"[..], DirectiveArity::Either(3, 4)),
        (&b"SecRule ARGS \"x\" \"a\" extra"[..], DirectiveArity::Either(3, 4)),
        (&b"SecAction"[..], DirectiveArity::Exact(2)),
        (&b"SecAction \"a\" extra"[..], DirectiveArity::Exact(2)),
        (&b"SecMarker"[..], DirectiveArity::AtLeast(2)),
        (&b"SecComponentSignature"[..], DirectiveArity::Exact(2)),
        (&b"SecRuleUpdateTargetById 1"[..], DirectiveArity::AtLeast(3)),
        (&b"SecRuleRemoveById"[..], DirectiveArity::AtLeast(2)),
    ] {
        assert!(matches!(
            parse_error(source),
            DirectiveParseError::Arity { expected: actual, .. } if actual == expected
        ));
    }
}

#[test]
fn rejects_forbidden_quote_styles_after_arity() {
    for (source, field, expected) in [
        (
            &b"SecRule \"ARGS\" \"x\""[..],
            DirectiveField::Targets,
            QuoteRequirement::Unquoted,
        ),
        (
            &b"SecRule ARGS '@rx x'"[..],
            DirectiveField::Operator,
            QuoteRequirement::Double,
        ),
        (
            &b"SecRule ARGS \"x\" actions"[..],
            DirectiveField::Actions,
            QuoteRequirement::Double,
        ),
        (
            &b"SecAction actions"[..],
            DirectiveField::Actions,
            QuoteRequirement::Double,
        ),
        (
            &b"SecComponentSignature signature"[..],
            DirectiveField::Signature,
            QuoteRequirement::Double,
        ),
        (
            &b"SecMarker 'marker'"[..],
            DirectiveField::Marker,
            QuoteRequirement::UnquotedOrDouble,
        ),
        (
            &b"SecMarker \"first\" second"[..],
            DirectiveField::Marker,
            QuoteRequirement::Unquoted,
        ),
        (
            &b"SecRuleUpdateTargetById \"1\" ARGS"[..],
            DirectiveField::UpdateSelector,
            QuoteRequirement::Unquoted,
        ),
        (
            &b"SecRuleRemoveById \"1\""[..],
            DirectiveField::UpdateSelector,
            QuoteRequirement::Unquoted,
        ),
        (
            &b"SecRuleUpdateTargetById 1 'ARGS'"[..],
            DirectiveField::UpdateTargets,
            QuoteRequirement::UnquotedOrDouble,
        ),
    ] {
        assert!(matches!(
            parse_error(source),
            DirectiveParseError::QuoteStyle { field: actual_field, expected: actual_expected, .. }
                if actual_field == field && actual_expected == expected
        ));
    }
}

#[test]
fn rejects_empty_fields_but_keeps_rule_empty_forms() {
    for (source, field) in [
        (&b"SecAction \"\""[..], DirectiveField::Actions),
        (&b"SecMarker \"\""[..], DirectiveField::Marker),
        (&b"SecComponentSignature \"\""[..], DirectiveField::Signature),
        (&b"SecRuleUpdateTargetById 1 \"\""[..], DirectiveField::UpdateTargets),
    ] {
        assert!(matches!(
            parse_error(source),
            DirectiveParseError::EmptyField { field: actual, .. } if actual == field
        ));
    }
}

#[test]
fn rejects_quoted_and_unknown_directives_without_echoing_content() {
    assert!(matches!(
        parse_error(br#""SecRule" ARGS "x""#),
        DirectiveParseError::QuotedDirectiveName { .. }
    ));
    let error = parse_error(b"PrivateDirective secret");
    assert!(matches!(error, DirectiveParseError::UnknownDirective { .. }));
    assert!(!error.to_string().contains("Private"));
    assert!(!format!("{error:?}").contains("Private"));
    assert!(!error.to_string().contains("secret"));

    let allocation = DirectiveParseError::Allocation {
        operation: "directive records",
    };
    assert_eq!(
        allocation.to_string(),
        "cannot allocate bounded SecLang directive records; reduce the source or configured limits"
    );
}

#[test]
fn preserves_continuation_spans_source_identity_and_borrowed_text() {
    let source = scan_source("first.conf", b"SecRule ARGS \\\n \"@rx x\"", SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let parsed = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
    drop(tokenized);
    let view = parsed.iter().next().unwrap();
    assert_eq!(parsed.source().source_id(), "first.conf");
    assert_eq!(view.span().start.line, 1);
    assert_eq!(view.span().end.line, 2);
    assert!(matches!(
        view.directive(),
        Directive::SecRule { targets, .. }
            if std::ptr::eq(targets.value().as_ptr(), parsed.source().lines()[0].text()[8..].as_ptr())
    ));

    let other = scan_source("second.conf", b"SecMarker A", SourceLimits::default()).unwrap();
    let other_tokens = tokenize_document(&other, TokenizerLimits::default()).unwrap();
    let other_parsed = parse_directives(&other_tokens, DirectiveLimits::default()).unwrap();
    assert_ne!(parsed.source().source_id(), other_parsed.source().source_id());
    assert_eq!(
        parsed.iter().next().unwrap().span().start,
        other_parsed.iter().next().unwrap().span().start
    );
}

#[test]
fn enforces_directive_limit_before_reading_excess_line_tokens() {
    let source = scan_source("limits", b"SecMarker A\n\"Private\"", SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let limits = DirectiveLimits {
        max_directives: 1,
        ..DirectiveLimits::default()
    };
    assert!(matches!(
        parse_directives(&tokenized, limits),
        Err(DirectiveParseError::DirectiveLimit { .. })
    ));
}

#[test]
fn enforces_selector_limit_before_selector_validation() {
    let source = scan_source(
        "selectors",
        b"SecRuleUpdateTargetById \"1\" ARGS",
        SourceLimits::default(),
    )
    .unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let limits = DirectiveLimits {
        max_update_selectors: 0,
        ..DirectiveLimits::default()
    };
    assert!(matches!(
        parse_directives(&tokenized, limits),
        Err(DirectiveParseError::UpdateSelectorLimit {
            actual: 1,
            limit: 0,
            ..
        })
    ));
    let source = scan_source("selectors", b"SecRuleRemoveById \"1\"", SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    assert!(matches!(
        parse_directives(&tokenized, limits),
        Err(DirectiveParseError::UpdateSelectorLimit {
            actual: 1,
            limit: 0,
            ..
        })
    ));
}

#[test]
fn enforces_exact_selector_and_owned_metadata_limits() {
    let source = scan_source("metadata", b"SecRuleUpdateTargetById 1 2 ARGS", SourceLimits::default()).unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let selector_size = 2 * size_of::<crate::seclang::Token<'_>>();
    let exact_bytes = selector_size + size_of::<DirectiveRecord<'_>>();
    let exact = DirectiveLimits {
        max_update_selectors: 2,
        max_owned_bytes: exact_bytes,
        ..DirectiveLimits::default()
    };
    let parsed = parse_directives(&tokenized, exact).unwrap();
    assert_eq!(parsed.charged_bytes(), exact_bytes);
    assert!(matches!(
        parsed.iter().next().unwrap().directive(),
        Directive::SecRuleUpdateTargetById { selectors, .. } if selectors.len() == 2
    ));

    let short = DirectiveLimits {
        max_update_selectors: 2,
        max_owned_bytes: exact_bytes - 1,
        ..DirectiveLimits::default()
    };
    assert!(matches!(
        parse_directives(&tokenized, short),
        Err(DirectiveParseError::OwnedByteLimit { .. })
    ));
}

#[test]
fn remove_selectors_are_borrowed_ordered_and_charged_exactly() {
    let source = scan_source(
        "remove",
        b"SecRuleRemoveById 0 9-3 18446744073709551615",
        SourceLimits::default(),
    )
    .unwrap();
    let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
    let exact_bytes = 3 * size_of::<crate::seclang::Token<'_>>() + size_of::<DirectiveRecord<'_>>();
    let limits = DirectiveLimits {
        max_update_selectors: 3,
        max_owned_bytes: exact_bytes,
        ..DirectiveLimits::default()
    };
    let parsed = parse_directives(&tokenized, limits).unwrap();
    assert_eq!(parsed.charged_bytes(), exact_bytes);
    let Directive::SecRuleRemoveById { selectors } = parsed.iter().next().unwrap().directive() else {
        panic!("expected removal envelope")
    };
    assert_eq!(
        selectors.iter().map(|selector| selector.value()).collect::<Vec<_>>(),
        ["0", "9-3", "18446744073709551615"]
    );
    let line = parsed.source().lines()[0].text();
    let line_start = line.as_ptr() as usize;
    let line_end = line_start + line.len();
    assert!(selectors.iter().all(|selector| {
        let pointer = selector.value().as_ptr() as usize;
        (line_start..line_end).contains(&pointer)
    }));
    assert!(matches!(
        parse_directives(
            &tokenized,
            DirectiveLimits {
                max_owned_bytes: exact_bytes - 1,
                ..limits
            }
        ),
        Err(DirectiveParseError::OwnedByteLimit { .. })
    ));
}
