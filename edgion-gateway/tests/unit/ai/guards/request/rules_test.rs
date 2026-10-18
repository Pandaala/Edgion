use super::*;
use edgion_resources::resources::edgion_plugins::{AiGuardMatcher, AiGuardPiiDetector};

fn rule(pattern: &str, action: AiGuardAction, replacement: Option<&str>) -> AiGuardRule {
    AiGuardRule {
        name: "r".to_string(),
        matcher: AiGuardMatcher::Regex {
            pattern: pattern.to_string(),
        },
        action,
        roles: None,
        replacement: replacement.map(str::to_string),
    }
}

/// Evaluate and keep only the decision, for tests that do not assert the
/// staged-rule count.
fn decide(rules: &CompiledRules, spans: &[RuleSpan<'_, u32>], cap: usize) -> Result<GuardDecision<u32>, ()> {
    rules.evaluate(spans, cap).map(|outcome| outcome.decision)
}

fn spans<'a>(texts: &'a [&'a str]) -> Vec<RuleSpan<'a, u32>> {
    texts
        .iter()
        .enumerate()
        .map(|(index, text)| RuleSpan {
            key: index as u32,
            role: None,
            current: text,
        })
        .collect()
}

#[test]
fn reject_wins_immediately_and_computes_no_replacement() {
    let rules = CompiledRules::compile(&[
        rule("secret", AiGuardAction::Mask, Some("[X]")),
        rule("forbidden", AiGuardAction::Reject, None),
    ])
    .expect("compiles");
    let input = spans(&["a secret here", "forbidden"]);
    assert_eq!(decide(&rules, &input, 4096), Ok(GuardDecision::Reject));
}

#[test]
fn mask_rewrites_only_matching_spans() {
    let rules = CompiledRules::compile(&[rule("[0-9]{3}-[0-9]{2}", AiGuardAction::Mask, Some("[REDACTED]"))])
        .expect("compiles");
    let input = spans(&["ssn 123-45 here", "clean"]);
    let decision = decide(&rules, &input, 4096).expect("evaluates");
    match decision {
        GuardDecision::Replace(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].0, 0);
            assert_eq!(items[0].1, "ssn [REDACTED] here");
        }
        other => panic!("expected replace, got {other:?}"),
    }
}

#[test]
fn later_rules_observe_earlier_masks() {
    let rules = CompiledRules::compile(&[
        rule("alpha", AiGuardAction::Mask, Some("beta")),
        rule("beta", AiGuardAction::Mask, Some("gamma")),
    ])
    .expect("compiles");
    let input = spans(&["alpha"]);
    match decide(&rules, &input, 4096).expect("evaluates") {
        GuardDecision::Replace(items) => assert_eq!(items[0].1, "gamma"),
        other => panic!("expected replace, got {other:?}"),
    }
}

#[test]
fn role_restricted_rules_skip_other_roles() {
    let mut restricted = rule("x", AiGuardAction::Reject, None);
    restricted.roles = Some(vec![AiGuardRole::System]);
    let rules = CompiledRules::compile(&[restricted]).expect("compiles");

    let user = vec![RuleSpan {
        key: 0_u32,
        role: Some(AiGuardRole::User),
        current: "x",
    }];
    assert_eq!(decide(&rules, &user, 4096), Ok(GuardDecision::Pass));

    let system = vec![RuleSpan {
        key: 0_u32,
        role: Some(AiGuardRole::System),
        current: "x",
    }];
    assert_eq!(decide(&rules, &system, 4096), Ok(GuardDecision::Reject));
}

#[test]
fn replacement_budget_is_inclusive_and_rejects_plus_one() {
    let rules = CompiledRules::compile(&[rule("(a)", AiGuardAction::Mask, Some("$1$1"))]).expect("compiles");
    let input = spans(&["a"]);
    // "a" expands to "aa": exactly two bytes.
    assert!(decide(&rules, &input, 2).is_ok());
    assert!(decide(&rules, &input, 1).is_err());
}

#[test]
fn mask_requires_a_replacement() {
    let error = CompiledRules::compile(&[rule("a", AiGuardAction::Mask, None)])
        .err()
        .expect("missing replacement is invalid");
    assert_eq!(error, "replacement is required for action mask");
}

#[test]
fn repeated_capture_amplification_stops_at_the_budget() {
    let rules = CompiledRules::compile(&[rule("(a)", AiGuardAction::Mask, Some("$1$1$1$1"))]).expect("compiles");
    assert!(decide(&rules, &spans(&["aaaaaaaa"]), 31).is_err());
    assert!(decide(&rules, &spans(&["aaaaaaaa"]), 32).is_ok());
}

#[test]
fn aggregate_budget_covers_all_live_span_replacements() {
    let rules = CompiledRules::compile(&[rule("a", AiGuardAction::Mask, Some("bb"))]).expect("compiles");
    assert!(decide(&rules, &spans(&["a", "a"]), 3).is_err());
    assert!(decide(&rules, &spans(&["a", "a"]), 4).is_ok());
}

#[test]
fn chained_shrink_releases_budget_for_another_span() {
    let rules = CompiledRules::compile(&[
        rule("^a$", AiGuardAction::Mask, Some("bbbb")),
        rule("^bbbb$", AiGuardAction::Mask, Some("c")),
        rule("^x$", AiGuardAction::Mask, Some("yyy")),
    ])
    .expect("compiles");

    assert_eq!(
        decide(&rules, &spans(&["a", "x"]), 4),
        Ok(GuardDecision::Replace(vec![
            (0, "c".to_string()),
            (1, "yyy".to_string()),
        ]))
    );
}

#[test]
fn returning_to_original_releases_the_slot_budget() {
    let rules = CompiledRules::compile(&[
        rule("^a$", AiGuardAction::Mask, Some("bbbb")),
        rule("^bbbb$", AiGuardAction::Mask, Some("a")),
        rule("^x$", AiGuardAction::Mask, Some("zzzz")),
    ])
    .expect("compiles");

    assert_eq!(
        decide(&rules, &spans(&["a", "x"]), 4),
        Ok(GuardDecision::Replace(vec![(1, "zzzz".to_string())]))
    );
}

#[test]
fn zero_budget_keeps_an_empty_divergent_replacement() {
    let rules = CompiledRules::compile(&[rule(".+", AiGuardAction::Mask, Some(""))]).expect("compiles");
    assert_eq!(
        decide(&rules, &spans(&["erase me"]), 0),
        Ok(GuardDecision::Replace(vec![(0, String::new())]))
    );
}

#[test]
fn oversized_no_match_is_a_pass() {
    let rules = CompiledRules::compile(&[rule("needle", AiGuardAction::Mask, Some("x"))]).expect("compiles");
    let input = "haystack".repeat(1024);
    assert_eq!(decide(&rules, &spans(&[input.as_str()]), 0), Ok(GuardDecision::Pass));
}

#[test]
fn a_mask_that_changes_nothing_is_a_pass() {
    let rules = CompiledRules::compile(&[rule("a", AiGuardAction::Mask, Some("a"))]).expect("compiles");
    assert_eq!(decide(&rules, &spans(["a"].as_slice()), 4096), Ok(GuardDecision::Pass));
}

#[test]
fn a_staged_allow_rule_is_counted_rather_than_silent() {
    let rules = CompiledRules::compile(&[rule("secret", AiGuardAction::Allow, None)]).expect("compiles");
    let outcome = rules
        .evaluate(&spans(&["a secret here", "clean", "another secret"]), 4096)
        .expect("evaluates");
    // Staging a rule must produce a real signal, not silence.
    assert_eq!(outcome.decision, GuardDecision::Pass);
    assert_eq!(outcome.flagged, 2);
}

#[test]
fn builtin_detector_matches_its_category() {
    let rules = CompiledRules::compile(&[AiGuardRule {
        name: "keys".to_string(),
        matcher: AiGuardMatcher::Pii {
            detector: AiGuardPiiDetector::AwsAccessKeyId,
        },
        action: AiGuardAction::Reject,
        roles: None,
        replacement: None,
    }])
    .expect("compiles");
    assert_eq!(
        decide(&rules, &spans(["key AKIAIOSFODNN7EXAMPLE end"].as_slice()), 4096),
        Ok(GuardDecision::Reject)
    );
    assert_eq!(
        decide(&rules, &spans(["nothing here"].as_slice()), 4096),
        Ok(GuardDecision::Pass)
    );
}

#[test]
fn rules_without_an_applicable_mask_need_no_slot_allocation() {
    let mut excluded_mask = rule("secret", AiGuardAction::Mask, Some("hidden"));
    excluded_mask.roles = Some(vec![AiGuardRole::System]);
    let cases = [
        (vec![rule("secret", AiGuardAction::Allow, None)], GuardDecision::Pass, 2),
        (
            vec![rule("secret", AiGuardAction::Reject, None)],
            GuardDecision::Reject,
            0,
        ),
        (
            vec![rule("absent", AiGuardAction::Reject, None)],
            GuardDecision::Pass,
            0,
        ),
        (
            vec![
                rule("secret", AiGuardAction::Allow, None),
                rule("secret", AiGuardAction::Reject, None),
            ],
            GuardDecision::Reject,
            2,
        ),
        (
            vec![excluded_mask, rule("secret", AiGuardAction::Allow, None)],
            GuardDecision::Pass,
            2,
        ),
        (
            vec![
                rule("secret", AiGuardAction::Reject, None),
                rule("secret", AiGuardAction::Mask, Some("hidden")),
            ],
            GuardDecision::Reject,
            0,
        ),
    ];
    let input = [
        RuleSpan {
            key: 0_u32,
            role: Some(AiGuardRole::User),
            current: "secret",
        },
        RuleSpan {
            key: 1,
            role: None,
            current: "another secret",
        },
        RuleSpan {
            key: 2,
            role: Some(AiGuardRole::User),
            current: "clean",
        },
    ];
    for (source, decision, flagged) in cases {
        let rules = CompiledRules::compile(&source).unwrap();
        let expected = RuleOutcome { decision, flagged };
        // Regex caches may initialize independently of the mask slot storage.
        assert_eq!(rules.evaluate(&input, 0), Ok(expected.clone()));
        let (actual, stats) = crate::test_allocation_counter::count_allocation_stats(|| rules.evaluate(&input, 0));
        assert_eq!(actual, Ok(expected));
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}

#[test]
fn allow_and_reject_observe_masks_after_lazy_slot_initialization() {
    for reject in [false, true] {
        let mut source = vec![
            rule("alpha", AiGuardAction::Allow, None),
            rule("alpha", AiGuardAction::Mask, Some("beta")),
            rule("beta", AiGuardAction::Allow, None),
        ];
        if reject {
            source.push(rule("beta", AiGuardAction::Reject, None));
        }
        let rules = CompiledRules::compile(&source).unwrap();
        let outcome = rules.evaluate(&spans(&["alpha", "clean"]), 4).unwrap();
        assert_eq!(outcome.flagged, 2);
        assert_eq!(
            outcome.decision,
            if reject {
                GuardDecision::Reject
            } else {
                GuardDecision::Replace(vec![(0, "beta".to_string())])
            }
        );
    }
}
