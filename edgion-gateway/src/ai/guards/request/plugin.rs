//! Request-stage AI prompt guard.
//!
//! Ordering is load-bearing. The provider request body is derived from the
//! canonical body `AiProxy` captures, so a mask published after `AiProxy` has
//! run never reaches the provider. The guard therefore refuses to run late,
//! and records the body generation it settled on so `AiProxy` can detect any
//! mutation that happened in between.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{AiGuardConfig, AiGuardRejectionMode};
use edgion_resources::EdgionErrorCode;

use super::evaluator::CompiledEvaluator;
use super::rules::{CompiledRules, GuardDecision, RuleSpan};
use super::spans::{select_spans, GuardSpan, GuardSpanKey, SpanError, SpanLimits};
use crate::ai::protocols::openai::error::{content_filter_response, openai_error_response, AiProxyError};
use crate::plugins::http::common::outbound::client::PreparedManagedHttp;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::request_body::BodyReadError;

const PLUGIN_NAME: &str = "AiGuard";

/// Canonical chat-completions endpoint suffix, matching `AiProxy`'s own gate.
const CHAT_COMPLETIONS_SUFFIX: &str = "/chat/completions";

pub struct AiGuard {
    identity: String,
    rules: Arc<CompiledRules>,
    evaluator: Option<Arc<CompiledEvaluator>>,
    rejection_mode: AiGuardRejectionMode,
    max_spans: usize,
    max_span_bytes: Option<usize>,
    max_total_span_bytes: Option<usize>,
    max_replacement_bytes: usize,
    fail_open: bool,
}

/// What the guard settled on, before it is rendered to the client.
enum Outcome {
    Pass,
    Masked(usize),
    Reject,
}

impl AiGuard {
    fn build(
        config: &AiGuardConfig,
        identity: String,
        evaluator: Option<Arc<CompiledEvaluator>>,
    ) -> Result<Self, String> {
        if let Some(error) = config.validation_error() {
            return Err(error);
        }
        if config.evaluator.is_some() != evaluator.is_some() {
            return Err("owning AiGuard evaluator required".into());
        }
        Ok(Self {
            identity,
            rules: Arc::new(CompiledRules::compile(&config.rules)?),
            evaluator,
            rejection_mode: config.rejection_mode,
            max_spans: config.max_spans,
            max_span_bytes: config.max_span_bytes,
            max_total_span_bytes: config.max_total_span_bytes,
            max_replacement_bytes: config.max_replacement_bytes,
            fail_open: config.fail_open,
        })
    }

    pub fn create(config: &AiGuardConfig, entry_scope: &str) -> Option<Box<dyn RequestFilter>> {
        Self::build(config, entry_scope.to_string(), None)
            .ok()
            .map(|plugin| Box::new(plugin) as Box<dyn RequestFilter>)
    }

    pub(crate) fn create_owned(input: &PreparedManagedHttp<'_>, entry_scope: &str) -> Option<Box<dyn RequestFilter>> {
        let edgion_resources::resources::edgion_plugins::EdgionPlugin::AiGuard(config) = input.plugin() else {
            return None;
        };
        // Validate before preparing the client or starting a health task.
        if config.validation_error().is_some() {
            return None;
        }
        let evaluator = Arc::new(CompiledEvaluator::build(input).ok()?);
        Self::build(config, entry_scope.to_string(), Some(evaluator))
            .ok()
            .map(|plugin| Box::new(plugin) as Box<dyn RequestFilter>)
    }

    /// Render a refusal in the operator-selected mode.
    fn refuse(&self, session: &mut dyn PluginSession, body: &[u8]) -> PluginRunningResult {
        match self.rejection_mode {
            AiGuardRejectionMode::Error => openai_error_response(session, AiProxyError::ContentFiltered),
            AiGuardRejectionMode::ContentFilter => {
                let facts = request_facts(body);
                content_filter_response(session, &facts.model, facts.streaming)
            }
        }
    }

    /// A configuration fault, never a policy decision.
    ///
    /// Always a fixed error and always fail-close: rendering an operator
    /// mistake as `contentFilter` would make it indistinguishable, to the
    /// client and to any agent chain, from a genuine content-policy refusal.
    fn configuration_fault(
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        kind: &'static str,
        status: EdgionErrorCode,
    ) -> PluginRunningResult {
        log.push_err(kind);
        session.record_error(status);
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None,
        }
    }

    /// Resolve a non-policy failure through the configured failure policy.
    fn dependency_failure(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        kind: &'static str,
        body: &[u8],
    ) -> PluginRunningResult {
        log.push_err(kind);
        // Latch even here: a backendRef-scoped guard is re-entered on every
        // upstream attempt, and repeating a failed evaluator call would
        // multiply off-box egress under retry.
        session.ai().record_ai_guard_decision(self.identity.clone(), None);
        if self.fail_open {
            PluginRunningResult::GoodNext
        } else {
            self.refuse(session, body)
        }
    }
}

/// Minimal facts the guard needs from the request body, read without keeping
/// the parsed tree alive.
struct RequestFacts {
    model: String,
    streaming: bool,
}

fn request_facts(body: &[u8]) -> RequestFacts {
    // Best-effort: only used to render a dialect-correct refusal. A body that
    // cannot be read here is separately reported as unguardable.
    let value: Option<serde_json::Value> = serde_json::from_slice(body).ok();
    let model = value
        .as_ref()
        .and_then(|value| value.get("model"))
        .and_then(|model| model.as_str())
        .unwrap_or_default()
        .to_string();
    let streaming = value
        .as_ref()
        .and_then(|value| value.get("stream"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    RequestFacts { model, streaming }
}

#[async_trait]
impl RequestFilter for AiGuard {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Record actual invocation before applicability: an earlier scoped guard
        // can precede URLRewrite and must not silently skip native Responses.
        session
            .ctx()
            .ai_request_guard_invoked
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if session.ctx().has_unsupported_native_guard_plan() {
            log.push_err("responses-guard-unsupported");
            return crate::ai::protocols::error::ai_error_response(session, AiProxyError::UnsupportedParameter);
        }
        // Re-entry latch. The backendRef-scoped chain re-runs on every upstream
        // attempt, and a published mask is not rolled back by rewind, so a
        // second pass would mask its own output.
        if session
            .ctx()
            .ai_state()
            .is_some_and(|state| state.guard_decided.iter().any(|id| id == &self.identity))
        {
            return PluginRunningResult::GoodNext;
        }

        // Not applicable: exactly AiProxy's gate. A global- or Gateway-scoped
        // guard must leave ordinary traffic completely untouched rather than
        // fail-closing it.
        if session.get_method() != "POST" || !session.get_path().ends_with(CHAT_COMPLETIONS_SUFFIX) {
            log.push("Skip not applicable");
            return PluginRunningResult::GoodNext;
        }

        // Ordering fault: AiProxy already captured the canonical body, so a
        // mask published now could never reach the provider.
        if session.ctx().has_ai_attempt_plan() {
            return Self::configuration_fault(
                session,
                log,
                "guard after ai proxy",
                EdgionErrorCode::BodyMutationConflict,
            );
        }

        let body = match session.read_request_body(None).await {
            Ok(body) => body,
            Err(BodyReadError::BufferLimitExceeded { .. }) => {
                log.push_err("body too large");
                return openai_error_response(session, AiProxyError::RequestTooLarge);
            }
            Err(_) => {
                log.push_err("body unavailable");
                return self.refuse(session, &[]);
            }
        };

        let Some(generation) = session.ctx().request_body.generation() else {
            return Self::configuration_fault(session, log, "body not buffered", EdgionErrorCode::BodyRefusedStage);
        };

        // Both span caps default to the resolved request-body cap, so the guard
        // can never reject a long-context request that AiProxy would accept.
        let body_cap = usize::try_from(session.ctx().request_body_config.max_body_size_bytes).unwrap_or(usize::MAX);
        let limits = SpanLimits {
            max_spans: self.max_spans,
            max_span_bytes: self.max_span_bytes.unwrap_or(body_cap),
            max_total_span_bytes: self.max_total_span_bytes.unwrap_or(body_cap),
        };
        let spans = match select_spans(&body, limits) {
            Ok(spans) => spans,
            Err(SpanError::Unparseable) => return self.dependency_failure(session, log, "request unguardable", &body),
            Err(SpanError::Bounds) => return self.dependency_failure(session, log, "span bounds", &body),
        };

        let rule_spans: Vec<RuleSpan<'_, GuardSpanKey>> = spans
            .iter()
            .map(|span| RuleSpan {
                key: span.key,
                role: span.role,
                current: span.current.as_str(),
            })
            .collect();

        let Ok(outcome) = self.rules.evaluate(&rule_spans, self.max_replacement_bytes) else {
            return self.dependency_failure(session, log, "replacement bounds", &body);
        };
        let flagged = outcome.flagged;
        let mut decision = outcome.decision;

        // The evaluator sees post-local-mask text, so locally masked content
        // never leaves the box. A local reject short-circuits it entirely.
        if let Some(evaluator) = self.evaluator.as_ref() {
            if !matches!(decision, GuardDecision::Reject) {
                let external = {
                    let merged = apply_local(&spans, &decision);
                    evaluator.decide(session, &merged).await
                };
                match external {
                    Ok(GuardDecision::Pass) => {}
                    Ok(GuardDecision::Reject) => decision = GuardDecision::Reject,
                    Ok(GuardDecision::Replace(items)) => {
                        decision = merge(decision, items);
                    }
                    Err(kind) => {
                        return self.dependency_failure(session, log, kind, &body);
                    }
                }
            }
        }

        self.finish_decision(
            session,
            log,
            FinalDecision {
                body: &body,
                spans: &spans,
                generation,
                decision,
                flagged,
            },
        )
        .await
    }
}

/// Request-local inputs retained until the final decision is settled.
struct FinalDecision<'a> {
    body: &'a [u8],
    spans: &'a [GuardSpan],
    generation: u64,
    decision: GuardDecision<GuardSpanKey>,
    flagged: usize,
}

impl AiGuard {
    /// Admit the final replacement set before encoding or publishing any mask.
    async fn finish_decision(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        input: FinalDecision<'_>,
    ) -> PluginRunningResult {
        let FinalDecision {
            body,
            spans,
            generation,
            decision,
            flagged,
        } = input;
        let outcome = match decision {
            GuardDecision::Reject => Outcome::Reject,
            GuardDecision::Pass => Outcome::Pass,
            GuardDecision::Replace(items) => {
                if let Err(kind) = validate_replacements(spans, &items, self.max_replacement_bytes) {
                    return self.dependency_failure(session, log, kind, body);
                }
                let mut edits: Vec<(std::ops::Range<u64>, Bytes)> = Vec::with_capacity(items.len());
                for (key, text) in &items {
                    let Some(span) = spans.iter().find(|span| span.key == *key) else {
                        // An evaluator naming a span that was never sent is a
                        // dependency protocol error, so it follows the failure
                        // policy rather than being reported as a gateway fault.
                        return self.dependency_failure(session, log, "evaluator bad key", body);
                    };
                    // Whole quoted tokens in, whole quoted tokens out: the mask
                    // never addresses an interior byte, so a code point cannot
                    // be split and the body stays valid JSON.
                    let Ok(encoded) = serde_json::to_string(text) else {
                        return self.dependency_failure(session, log, "replacement encode", body);
                    };
                    edits.push((span.token.clone(), Bytes::from(encoded)));
                }
                edits.sort_by_key(|(range, _)| range.start);
                if edits.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
                    return Self::configuration_fault(
                        session,
                        log,
                        "span overlap",
                        EdgionErrorCode::BodyMutationConflict,
                    );
                }
                match session.replace_request_body_spans(generation, &edits).await {
                    Ok(_) => Outcome::Masked(edits.len()),
                    Err(BodyReadError::GenerationConflict) => {
                        return Self::configuration_fault(
                            session,
                            log,
                            "body changed under guard",
                            EdgionErrorCode::BodyMutationConflict,
                        )
                    }
                    Err(BodyReadError::BufferLimitExceeded { .. }) => {
                        log.push_err("mask exceeds body cap");
                        return openai_error_response(session, AiProxyError::RequestTooLarge);
                    }
                    Err(_) => return self.dependency_failure(session, log, "mask failed", body),
                }
            }
        };

        // Bind the settled generation so AiProxy can detect a later mutation.
        let settled = session.ctx().request_body.generation();
        {
            let identity = self.identity.clone();
            session.ai().record_ai_guard_decision(identity, settled);
        }

        match outcome {
            Outcome::Reject => {
                log.push("Deny content filter");
                self.refuse(session, body)
            }
            Outcome::Masked(count) => {
                log.push_fmt("OK masked {}", crate::plugins::runtime::log::la![count]);
                PluginRunningResult::GoodNext
            }
            Outcome::Pass if flagged > 0 => {
                log.push_fmt("OK flagged {}", crate::plugins::runtime::log::la![flagged]);
                PluginRunningResult::GoodNext
            }
            Outcome::Pass => {
                log.push("OK");
                PluginRunningResult::GoodNext
            }
        }
    }
}

/// Validate the final decoded text after evaluator overrides have resolved.
fn validate_replacements(
    spans: &[GuardSpan],
    items: &[(GuardSpanKey, String)],
    max_replacement_bytes: usize,
) -> Result<(), &'static str> {
    // Establish the selected-span bound before quadratic key comparisons.
    if items.len() > spans.len() {
        return Err("evaluator bad key");
    }
    for (index, (key, _)) in items.iter().enumerate() {
        if !spans.iter().any(|span| span.key == *key) || items[..index].iter().any(|(previous, _)| previous == key) {
            return Err("evaluator bad key");
        }
    }
    let mut total = 0_usize;
    for (_, text) in items {
        total = total.checked_add(text.len()).ok_or("replacement bounds")?;
        if total > max_replacement_bytes {
            return Err("replacement bounds");
        }
    }
    Ok(())
}

/// Borrow the post-local-mask text of every span for the evaluator.
/// The sole caller supplies spans sorted by select_spans and replacements sorted
/// by CompiledRules::evaluate, so one forward cursor resolves every lookup.
fn apply_local<'a>(spans: &'a [GuardSpan], decision: &'a GuardDecision<GuardSpanKey>) -> Vec<(GuardSpanKey, &'a str)> {
    let replaced = match decision {
        GuardDecision::Replace(items) => items.as_slice(),
        _ => &[],
    };
    let mut replacement_index = 0;
    spans
        .iter()
        .map(|span| {
            while replaced.get(replacement_index).is_some_and(|(key, _)| *key < span.key) {
                replacement_index += 1;
            }
            let text = replaced
                .get(replacement_index)
                .filter(|(key, _)| *key == span.key)
                .map(|(_, text)| text.as_str())
                .unwrap_or(span.current.as_str());
            (span.key, text)
        })
        .collect()
}

/// Evaluator replacements override local ones for the same span key, so each
/// key resolves to exactly one final value.
fn merge(local: GuardDecision<GuardSpanKey>, external: Vec<(GuardSpanKey, String)>) -> GuardDecision<GuardSpanKey> {
    let mut merged = match local {
        GuardDecision::Replace(items) => items,
        _ => Vec::new(),
    };
    for (key, text) in external {
        match merged.iter_mut().find(|(existing, _)| *existing == key) {
            Some(slot) => slot.1 = text,
            None => merged.push((key, text)),
        }
    }
    if merged.is_empty() {
        GuardDecision::Pass
    } else {
        merged.sort_by_key(|(key, _)| *key);
        GuardDecision::Replace(merged)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/guards/request/plugin_test.rs"]
mod tests;
