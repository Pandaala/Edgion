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
use edgion_resources::EdgionStatus;

use super::evaluator::CompiledEvaluator;
use super::rules::{CompiledRules, GuardDecision, RuleSpan};
use super::spans::{select_spans, GuardSpan, GuardSpanKey, SpanError, SpanLimits};
use crate::ai::error::{content_filter_response, openai_error_response, AiProxyError};
use crate::plugins::runtime::traits::session::BodyReadError;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

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
    fn build(config: &AiGuardConfig, identity: String) -> Result<Self, String> {
        if let Some(error) = config.validation_error() {
            return Err(error);
        }
        let evaluator = match config.evaluator.as_ref() {
            Some(evaluator) => Some(Arc::new(CompiledEvaluator::build(
                evaluator,
                config.max_replacement_bytes,
            )?)),
            None => None,
        };
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
        Self::build(config, entry_scope.to_string())
            .ok()
            .map(|plugin| Box::new(plugin) as Box<dyn RequestFilter>)
    }

    /// Render a refusal in the operator-selected mode.
    fn refuse(&self, session: &mut dyn PluginSession, model: &str, streaming: bool) -> PluginRunningResult {
        match self.rejection_mode {
            AiGuardRejectionMode::Error => openai_error_response(session, AiProxyError::ContentFiltered),
            AiGuardRejectionMode::ContentFilter => content_filter_response(session, model, streaming),
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
        status: EdgionStatus,
    ) -> PluginRunningResult {
        log.push_err(kind);
        session.add_error(status);
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
        model: &str,
        streaming: bool,
    ) -> PluginRunningResult {
        log.push_err(kind);
        // Latch even here: a backendRef-scoped guard is re-entered on every
        // upstream attempt, and repeating a failed evaluator call would
        // multiply off-box egress under retry.
        session.record_ai_guard_decision(self.identity.clone(), None);
        if self.fail_open {
            PluginRunningResult::GoodNext
        } else {
            self.refuse(session, model, streaming)
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
            return Self::configuration_fault(session, log, "guard after ai proxy", EdgionStatus::BodyMutationConflict);
        }

        let body = match session.read_request_body(None).await {
            Ok(body) => body,
            Err(BodyReadError::BufferLimitExceeded { .. }) => {
                log.push_err("body too large");
                return openai_error_response(session, AiProxyError::RequestTooLarge);
            }
            Err(_) => {
                log.push_err("body unavailable");
                return self.refuse(session, "", false);
            }
        };
        let facts = request_facts(&body);

        let Some(generation) = session.ctx().request_body.generation() else {
            return Self::configuration_fault(session, log, "body not buffered", EdgionStatus::BodyRefusedStage);
        };

        // Both span caps default to the resolved request-body cap, so the guard
        // can never reject a long-context request that AiProxy would accept.
        let body_cap = session.ctx().request_body_config.max_body_size_bytes as usize;
        let limits = SpanLimits {
            max_spans: self.max_spans,
            max_span_bytes: self.max_span_bytes.unwrap_or(body_cap),
            max_total_span_bytes: self.max_total_span_bytes.unwrap_or(body_cap),
        };
        let spans = match select_spans(&body, limits) {
            Ok(spans) => spans,
            Err(SpanError::Unparseable) => {
                return self.dependency_failure(session, log, "request unguardable", &facts.model, facts.streaming)
            }
            Err(SpanError::Bounds) => {
                return self.dependency_failure(session, log, "span bounds", &facts.model, facts.streaming)
            }
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
            return self.dependency_failure(session, log, "replacement bounds", &facts.model, facts.streaming);
        };
        let flagged = outcome.flagged;
        let mut decision = outcome.decision;

        // The evaluator sees post-local-mask text, so locally masked content
        // never leaves the box. A local reject short-circuits it entirely.
        if let Some(evaluator) = self.evaluator.as_ref() {
            if !matches!(decision, GuardDecision::Reject) {
                let merged = apply_local(&spans, &decision);
                match evaluator.decide(session, &merged).await {
                    Ok(GuardDecision::Pass) => {}
                    Ok(GuardDecision::Reject) => decision = GuardDecision::Reject,
                    Ok(GuardDecision::Replace(items)) => {
                        decision = merge(decision, items);
                    }
                    Err(kind) => {
                        return self.dependency_failure(session, log, kind, &facts.model, facts.streaming);
                    }
                }
            }
        }

        let outcome = match decision {
            GuardDecision::Reject => Outcome::Reject,
            GuardDecision::Pass => Outcome::Pass,
            GuardDecision::Replace(items) => {
                let mut edits: Vec<(std::ops::Range<u64>, Bytes)> = Vec::with_capacity(items.len());
                for (key, text) in &items {
                    let Some(span) = spans.iter().find(|span| span.key == *key) else {
                        // An evaluator naming a span that was never sent is a
                        // dependency protocol error, so it follows the failure
                        // policy rather than being reported as a gateway fault.
                        return self.dependency_failure(
                            session,
                            log,
                            "evaluator bad key",
                            &facts.model,
                            facts.streaming,
                        );
                    };
                    // Whole quoted tokens in, whole quoted tokens out: the mask
                    // never addresses an interior byte, so a code point cannot
                    // be split and the body stays valid JSON.
                    let Ok(encoded) = serde_json::to_string(text) else {
                        return self.dependency_failure(
                            session,
                            log,
                            "replacement encode",
                            &facts.model,
                            facts.streaming,
                        );
                    };
                    edits.push((span.token.clone(), Bytes::from(encoded)));
                }
                edits.sort_by_key(|(range, _)| range.start);
                if edits.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
                    return Self::configuration_fault(session, log, "span overlap", EdgionStatus::BodyMutationConflict);
                }
                match session.replace_request_body_spans(generation, &edits).await {
                    Ok(_) => Outcome::Masked(edits.len()),
                    Err(BodyReadError::GenerationConflict) => {
                        return Self::configuration_fault(
                            session,
                            log,
                            "body changed under guard",
                            EdgionStatus::BodyMutationConflict,
                        )
                    }
                    Err(BodyReadError::BufferLimitExceeded { .. }) => {
                        log.push_err("mask exceeds body cap");
                        return openai_error_response(session, AiProxyError::RequestTooLarge);
                    }
                    Err(_) => {
                        return self.dependency_failure(session, log, "mask failed", &facts.model, facts.streaming)
                    }
                }
            }
        };

        // Bind the settled generation so AiProxy can detect a later mutation.
        let settled = session.ctx().request_body.generation();
        {
            let identity = self.identity.clone();
            session.record_ai_guard_decision(identity, settled);
        }

        match outcome {
            Outcome::Reject => {
                log.push("Deny content filter");
                self.refuse(session, &facts.model, facts.streaming)
            }
            Outcome::Masked(count) => {
                log.push_fmt(
                    "OK masked {}",
                    crate::plugins::runtime::log::la![count.to_string().as_str()],
                );
                PluginRunningResult::GoodNext
            }
            Outcome::Pass if flagged > 0 => {
                log.push_fmt(
                    "OK flagged {}",
                    crate::plugins::runtime::log::la![flagged.to_string().as_str()],
                );
                PluginRunningResult::GoodNext
            }
            Outcome::Pass => {
                log.push("OK");
                PluginRunningResult::GoodNext
            }
        }
    }
}

/// Materialize the post-local-mask text of every span, for the evaluator.
fn apply_local(spans: &[GuardSpan], decision: &GuardDecision<GuardSpanKey>) -> Vec<(GuardSpanKey, String)> {
    let replaced = match decision {
        GuardDecision::Replace(items) => items.as_slice(),
        _ => &[],
    };
    spans
        .iter()
        .map(|span| {
            let text = replaced
                .iter()
                .find(|(key, _)| *key == span.key)
                .map(|(_, text)| text.clone())
                .unwrap_or_else(|| span.current.clone());
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
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::ai::attempt_plan::{AiAttemptPlan, AiPlanError};
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::request_body::BodyStorage;
    use crate::EdgionHttpContext;

    fn config(rules: &str, extra: &str) -> AiGuardConfig {
        serde_yaml::from_str(&format!("rules:\n{rules}{extra}")).expect("configuration parses")
    }

    fn reject_rule() -> &'static str {
        "  - name: tripwire\n    match: { type: regex, pattern: \"forbidden\" }\n    action: reject\n"
    }

    fn mask_rule() -> &'static str {
        "  - name: mask\n    match: { type: regex, pattern: \"secret\" }\n    action: mask\n    replacement: \"[X]\"\n"
    }

    fn guard(rules: &str, extra: &str) -> AiGuard {
        AiGuard::build(&config(rules, extra), "test/requestPlugins/#0".to_string()).expect("builds")
    }

    /// A context with a body view installed, so `generation()` is observable.
    fn ctx_with_body() -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        let (_storage, view) = BodyStorage::new(64 * 1024, 1024 * 1024);
        ctx.request_body.install(Arc::clone(&view));
        ctx
    }

    fn body(text: &str) -> Bytes {
        Bytes::from(text.to_string())
    }

    const PROMPT: &str = r#"{"model":"m","messages":[{"role":"user","content":"a forbidden thing"}]}"#;
    const SECRET_PROMPT: &str = r#"{"model":"m","messages":[{"role":"user","content":"my secret value"}]}"#;

    #[tokio::test]
    async fn a_request_that_is_not_a_chat_completion_passes_through_untouched() {
        let guard = guard(reject_rule(), "");
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("GET".to_string());
        session.expect_get_path().return_const("/healthz".to_string());
        // A global-scope guard must not read or touch ordinary traffic.
        session.expect_read_request_body().never();
        session.expect_replace_request_body_spans().never();

        let mut log = PluginLog::new("AiGuard");
        assert_eq!(
            guard.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn a_post_to_another_path_also_passes_through() {
        let guard = guard(reject_rule(), "");
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("POST".to_string());
        session.expect_get_path().return_const("/v1/embeddings".to_string());
        session.expect_read_request_body().never();

        let mut log = PluginLog::new("AiGuard");
        assert_eq!(
            guard.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn running_after_ai_proxy_is_a_fixed_error_in_every_rejection_mode() {
        for mode in ["error", "contentFilter"] {
            let guard = guard(reject_rule(), &format!("rejectionMode: {mode}\nfailOpen: true\n"));
            let mut ctx = ctx_with_body();
            ctx.restore_ai_attempt_plan(AiAttemptPlan::Rejected(AiPlanError::InvalidBackend));

            let mut session = MockPluginSession::new();
            session.expect_ctx().return_const(ctx);
            session.expect_get_method().return_const("POST".to_string());
            session
                .expect_get_path()
                .return_const("/v1/chat/completions".to_string());
            session.expect_add_error().returning(|_| ());
            // An ordering fault must never be softened, and never rendered as
            // a content-policy decision.
            session.expect_read_request_body().never();

            let mut log = PluginLog::new("AiGuard");
            assert_eq!(
                guard.run_request(&mut session, &mut log).await,
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None
                },
                "mode {mode} must fail closed with a fixed error"
            );
        }
    }

    #[tokio::test]
    async fn a_re_entered_guard_short_circuits_without_masking_again() {
        let guard = guard(mask_rule(), "");
        let mut ctx = ctx_with_body();
        ctx.ensure_ai_state()
            .guard_decided
            .push("test/requestPlugins/#0".to_string());

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx);
        // The latch is checked before anything else, so an upstream retry
        // cannot mask the guard's own output a second time.
        session.expect_read_request_body().never();
        session.expect_replace_request_body_spans().never();

        let mut log = PluginLog::new("AiGuard");
        assert_eq!(
            guard.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn a_local_reject_renders_the_fixed_error_envelope() {
        let guard = guard(reject_rule(), "");
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_get_path()
            .return_const("/v1/chat/completions".to_string());
        session.expect_read_request_body().returning(|_| Ok(body(PROMPT)));
        session.expect_set_response_header().returning(|_, _| Ok(()));
        session.expect_record_ai_guard_decision().returning(|_, _| ());

        let mut log = PluginLog::new("AiGuard");
        match guard.run_request(&mut session, &mut log).await {
            PluginRunningResult::ErrResponse { status, body } => {
                assert_eq!(status, 403);
                let body = body.expect("error body");
                assert!(body.contains("content_filter_error"));
                assert!(body.contains("content_policy_violation"));
                // The matched rule name and the prompt itself never leak.
                assert!(!body.contains("tripwire"));
                assert!(!body.contains("forbidden"));
            }
            other => panic!("expected a fixed error reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn content_filter_mode_renders_a_valid_completion_for_both_request_shapes() {
        for (streaming, expected) in [(false, "chat.completion"), (true, "chat.completion.chunk")] {
            let guard = guard(reject_rule(), "rejectionMode: contentFilter\n");
            let prompt = format!(
                r#"{{"model":"gpt-x","stream":{streaming},"messages":[{{"role":"user","content":"a forbidden thing"}}]}}"#
            );
            let mut session = MockPluginSession::new();
            session.expect_ctx().return_const(ctx_with_body());
            session.expect_get_method().return_const("POST".to_string());
            session
                .expect_get_path()
                .return_const("/v1/chat/completions".to_string());
            session.expect_read_request_body().returning(move |_| Ok(body(&prompt)));
            session.expect_set_response_header().returning(|_, _| Ok(()));
            session.expect_record_ai_guard_decision().returning(|_, _| ());

            let mut log = PluginLog::new("AiGuard");
            match guard.run_request(&mut session, &mut log).await {
                PluginRunningResult::ErrResponse { status, body } => {
                    // A degraded completion, not an error status.
                    assert_eq!(status, 200);
                    let body = body.expect("refusal body");
                    assert!(body.contains(expected), "streaming={streaming} body={body}");
                    assert!(body.contains(r#""finish_reason":"content_filter""#));
                    assert!(body.contains("gpt-x"));
                    if streaming {
                        assert!(body.starts_with("data: "));
                        assert!(body.trim_end().ends_with("data: [DONE]"));
                    }
                }
                other => panic!("expected a content-filter refusal, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_mask_publishes_one_batch_of_whole_json_string_tokens() {
        let guard = guard(mask_rule(), "");
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_get_path()
            .return_const("/v1/chat/completions".to_string());
        session
            .expect_read_request_body()
            .returning(|_| Ok(body(SECRET_PROMPT)));
        session
            .expect_replace_request_body_spans()
            .times(1)
            .withf(|generation, edits| {
                // One atomic batch bound to the captured generation.
                *generation == 0 && edits.len() == 1 && {
                    let replacement = std::str::from_utf8(&edits[0].1).expect("utf8");
                    // A complete quoted JSON string token replaces a
                    // complete quoted JSON string token, so no code point
                    // can be split and the body stays valid JSON.
                    replacement.starts_with('"')
                        && replacement.ends_with('"')
                        && replacement.contains("[X]")
                        && !replacement.contains("secret")
                }
            })
            .returning(|_, _| Ok(1));
        session.expect_record_ai_guard_decision().returning(|_, _| ());

        let mut log = PluginLog::new("AiGuard");
        assert_eq!(
            guard.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn a_generation_conflict_is_a_fixed_error_even_when_fail_open() {
        let guard = guard(mask_rule(), "failOpen: true\n");
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_get_path()
            .return_const("/v1/chat/completions".to_string());
        session
            .expect_read_request_body()
            .returning(|_| Ok(body(SECRET_PROMPT)));
        session
            .expect_replace_request_body_spans()
            .returning(|_, _| Err(BodyReadError::GenerationConflict));
        session.expect_add_error().returning(|_| ());

        let mut log = PluginLog::new("AiGuard");
        assert_eq!(
            guard.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            },
            "a body changed under the guard is a fault, never softened"
        );
    }

    #[tokio::test]
    async fn an_unguardable_body_follows_the_failure_policy() {
        let malformed = r#"{"model":"m"}"#;

        let closed = guard(reject_rule(), "");
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_get_path()
            .return_const("/v1/chat/completions".to_string());
        session
            .expect_read_request_body()
            .returning(move |_| Ok(body(malformed)));
        session.expect_set_response_header().returning(|_, _| Ok(()));
        // Even a failure latches, so an upstream retry does not repeat the work.
        session.expect_record_ai_guard_decision().times(1).returning(|_, _| ());
        let mut log = PluginLog::new("AiGuard");
        assert!(matches!(
            closed.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { status: 403, .. }
        ));

        let open = guard(reject_rule(), "failOpen: true\n");
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_get_path()
            .return_const("/v1/chat/completions".to_string());
        session
            .expect_read_request_body()
            .returning(move |_| Ok(body(malformed)));
        session.expect_record_ai_guard_decision().times(1).returning(|_, _| ());
        let mut log = PluginLog::new("AiGuard");
        assert_eq!(
            open.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn a_clean_prompt_passes_without_touching_the_body() {
        let guard = guard(reject_rule(), "");
        let clean = r#"{"model":"m","messages":[{"role":"user","content":"hello"}]}"#;
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_get_path()
            .return_const("/v1/chat/completions".to_string());
        session.expect_read_request_body().returning(move |_| Ok(body(clean)));
        session.expect_replace_request_body_spans().never();
        session.expect_record_ai_guard_decision().returning(|_, _| ());

        let mut log = PluginLog::new("AiGuard");
        assert_eq!(
            guard.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }
}
