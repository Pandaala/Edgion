//! Compiled local rule model and its key-generic evaluation.
//!
//! The rule set is compiled once per plugin snapshot and never per request.
//! Evaluation is generic over the span key so the same rule model can later be
//! driven from the Work Package 17 response hook, whose key is a different,
//! non-convertible type. Nothing here is wired to the response side.

use edgion_resources::resources::edgion_plugins::{
    compile_guard_pattern, AiGuardAction, AiGuardMatcher, AiGuardRole, AiGuardRule,
};

/// One rule with its pattern already compiled under the shared DFA bounds.
pub struct CompiledRule {
    regex: regex::Regex,
    action: AiGuardAction,
    roles: Option<Vec<AiGuardRole>>,
    replacement: Option<String>,
}

/// The immutable compiled rule set owned by a plugin snapshot.
pub struct CompiledRules {
    rules: Vec<CompiledRule>,
}

/// A span as the rule engine sees it: a key, an optional role scope, prior
/// context, and the current text.
///
/// Generic over the key so the response-side hook can pass its own bucket key
/// without a second rule language.
pub struct RuleSpan<'a, K> {
    pub key: K,
    pub role: Option<AiGuardRole>,
    pub current: &'a str,
}

/// The internal decision vocabulary, shared with the external evaluator and
/// shaped so the response hook needs no second vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardDecision<K> {
    Pass,
    Replace(Vec<(K, String)>),
    Reject,
}

/// One rule pass: the decision plus how many spans a staged `allow` rule
/// matched, so staging a rule produces a real signal instead of silence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOutcome<K> {
    pub decision: GuardDecision<K>,
    pub flagged: usize,
}

impl CompiledRules {
    /// Compile a rule list. Errors match the admission-time messages exactly,
    /// so the controller and the gateway can never disagree about validity.
    pub fn compile(rules: &[AiGuardRule]) -> Result<Self, String> {
        let mut compiled = Vec::with_capacity(rules.len());
        for rule in rules {
            let pattern = match &rule.matcher {
                AiGuardMatcher::Regex { pattern } => pattern.clone(),
                AiGuardMatcher::Pii { detector } => detector.pattern().to_string(),
            };
            compiled.push(CompiledRule {
                regex: compile_guard_pattern(&pattern)?,
                action: rule.action,
                roles: rule.roles.clone(),
                replacement: rule.replacement.clone(),
            });
        }
        Ok(Self { rules: compiled })
    }

    /// Evaluate every rule in declared order over the sorted span snapshot.
    ///
    /// A `Reject` wins immediately and suppresses any replacement, so no
    /// masked text is computed for a request that is about to be refused.
    /// Otherwise each masking rule rewrites the accumulated text of the spans
    /// it matches, and the aggregate replacement budget is checked as it goes.
    pub fn evaluate<K: Copy + Ord>(
        &self,
        spans: &[RuleSpan<'_, K>],
        max_replacement_bytes: usize,
    ) -> Result<RuleOutcome<K>, ()> {
        if self.rules.is_empty() || spans.is_empty() {
            return Ok(RuleOutcome {
                decision: GuardDecision::Pass,
                flagged: 0,
            });
        }

        let mut masked: Vec<Option<String>> = vec![None; spans.len()];
        let mut any_mask = false;
        let mut flagged = 0_usize;

        for rule in &self.rules {
            for (index, span) in spans.iter().enumerate() {
                if let Some(roles) = rule.roles.as_ref() {
                    match span.role {
                        Some(role) if roles.contains(&role) => {}
                        // An unknown or out-of-scope role never matches a
                        // role-restricted rule.
                        _ => continue,
                    }
                }
                let current: &str = masked[index].as_deref().unwrap_or(span.current);
                if !rule.regex.is_match(current) {
                    continue;
                }
                match rule.action {
                    AiGuardAction::Reject => {
                        return Ok(RuleOutcome {
                            decision: GuardDecision::Reject,
                            flagged,
                        })
                    }
                    AiGuardAction::Allow => flagged = flagged.saturating_add(1),
                    AiGuardAction::Mask => {
                        let replacement = rule.replacement.as_deref().unwrap_or_default();
                        let rewritten = rule.regex.replace_all(current, replacement).into_owned();
                        // Bound each rewrite as it happens: masking rules chain,
                        // so a replacement longer than its match compounds
                        // across rules before any aggregate check would fire.
                        if rewritten.len() > max_replacement_bytes {
                            return Err(());
                        }
                        masked[index] = Some(rewritten);
                        any_mask = true;
                    }
                }
            }
        }

        if !any_mask {
            return Ok(RuleOutcome {
                decision: GuardDecision::Pass,
                flagged,
            });
        }

        let mut total = 0_usize;
        let mut replacements = Vec::new();
        for (index, span) in spans.iter().enumerate() {
            let Some(text) = masked[index].take() else {
                continue;
            };
            if text == span.current {
                continue;
            }
            total = total.checked_add(text.len()).ok_or(())?;
            if total > max_replacement_bytes {
                return Err(());
            }
            replacements.push((span.key, text));
        }
        if replacements.is_empty() {
            return Ok(RuleOutcome {
                decision: GuardDecision::Pass,
                flagged,
            });
        }
        replacements.sort_by_key(|(key, _)| *key);
        Ok(RuleOutcome {
            decision: GuardDecision::Replace(replacements),
            flagged,
        })
    }
}

#[cfg(test)]
mod tests {
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
        let rules = CompiledRules::compile(&[rule("a", AiGuardAction::Mask, Some("bb"))]).expect("compiles");
        let input = spans(&["a"]);
        // "a" masks to "bb": exactly two bytes.
        assert!(decide(&rules, &input, 2).is_ok());
        assert!(decide(&rules, &input, 1).is_err());
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
}
