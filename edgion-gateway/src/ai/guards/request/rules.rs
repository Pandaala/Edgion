//! Compiled local rule model and its key-generic evaluation.
//!
//! The rule set is compiled once per plugin snapshot and never per request.
//! Evaluation is generic over the span key so the same rule model can later be
//! driven from the Work Package 17 response hook, whose key is a different,
//! non-convertible type. Nothing here is wired to the response side.

use edgion_resources::resources::edgion_plugins::{
    compile_guard_pattern, AiGuardAction, AiGuardMatcher, AiGuardRole, AiGuardRule,
};

use crate::plugins::http::common::CompiledRegexReplacement;

/// One rule with its pattern already compiled under the shared DFA bounds.
pub struct CompiledRule {
    regex: regex::Regex,
    action: AiGuardAction,
    roles: Option<Vec<AiGuardRole>>,
    replacement: Option<CompiledRegexReplacement>,
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
            let regex = compile_guard_pattern(&pattern)?;
            let replacement = match rule.action {
                AiGuardAction::Mask => Some(CompiledRegexReplacement::compile(
                    &regex,
                    rule.replacement
                        .as_deref()
                        .ok_or_else(|| "replacement is required for action mask".to_string())?
                        .to_string(),
                )),
                AiGuardAction::Allow | AiGuardAction::Reject => None,
            };
            compiled.push(CompiledRule {
                regex,
                action: rule.action,
                roles: rule.roles.clone(),
                replacement,
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

        let mut masked: Vec<Option<String>> = Vec::new();
        let mut live_replacement_bytes = 0_usize;
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
                match rule.action {
                    AiGuardAction::Reject => {
                        let current = masked.get(index).and_then(Option::as_deref).unwrap_or(span.current);
                        if rule.regex.is_match(current) {
                            return Ok(RuleOutcome {
                                decision: GuardDecision::Reject,
                                flagged,
                            });
                        }
                    }
                    AiGuardAction::Allow => {
                        let current = masked.get(index).and_then(Option::as_deref).unwrap_or(span.current);
                        if rule.regex.is_match(current) {
                            flagged = flagged.saturating_add(1);
                        }
                    }
                    AiGuardAction::Mask => {
                        if masked.is_empty() {
                            masked = vec![None; spans.len()];
                        }
                        let old_len = masked[index].as_ref().map_or(0, String::len);
                        let other_live = live_replacement_bytes.checked_sub(old_len).ok_or(())?;
                        let candidate_limit = max_replacement_bytes.checked_sub(other_live).ok_or(())?;
                        let rewritten = {
                            let current = masked[index].as_deref().unwrap_or(span.current);
                            rule.replacement
                                .as_ref()
                                .ok_or(())?
                                .replace_all_bounded(&rule.regex, current, candidate_limit)
                                .map_err(|_| ())?
                        };
                        let Some(rewritten) = rewritten else {
                            continue;
                        };
                        let unchanged = masked[index].as_deref().unwrap_or(span.current) == rewritten;
                        if unchanged {
                            continue;
                        }
                        if rewritten == span.current {
                            masked[index] = None;
                            live_replacement_bytes = other_live;
                            continue;
                        }
                        live_replacement_bytes = other_live.checked_add(rewritten.len()).ok_or(())?;
                        masked[index] = Some(rewritten);
                    }
                }
            }
        }

        if masked.iter().all(Option::is_none) {
            return Ok(RuleOutcome {
                decision: GuardDecision::Pass,
                flagged,
            });
        }

        let mut replacements = Vec::new();
        for (index, span) in spans.iter().enumerate() {
            let Some(text) = masked[index].take() else {
                continue;
            };
            replacements.push((span.key, text));
        }
        replacements.sort_by_key(|(key, _)| *key);
        Ok(RuleOutcome {
            decision: GuardDecision::Replace(replacements),
            flagged,
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/guards/request/rules_test.rs"]
mod tests;
