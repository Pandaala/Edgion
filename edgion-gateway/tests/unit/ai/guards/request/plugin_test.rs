use super::*;
use pingora_core::protocols::http::RequestBodyBuffer;
use std::sync::Arc;

use super::super::spans::GuardPart;
use crate::ai::runtime::attempt_plan::{AiAttemptPlan, AiPlanError};
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
    AiGuard::build(&config(rules, extra), "test/requestPlugins/#0".to_string(), None).expect("builds")
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
    assert!(session
        .ctx()
        .ai_request_guard_invoked
        .load(std::sync::atomic::Ordering::Relaxed));
    assert!(session.ctx().ai_state.is_none());
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
        session.expect_record_error().returning(|_| ());
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
async fn an_unavailable_body_is_refused_without_latching_even_when_fail_open() {
    for (mode, expected_status) in [("error", 403), ("contentFilter", 200)] {
        let guard = guard(reject_rule(), &format!("rejectionMode: {mode}\nfailOpen: true\n"));
        let mut session = MockPluginSession::new();
        let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_get_path()
            .return_const("/v1/chat/completions".to_string());
        session
            .expect_read_request_body()
            .returning(|_| Err(BodyReadError::Unimplemented));
        session.expect_set_response_header().returning(|_, _| Ok(()));
        ai.expect_record_ai_guard_decision().never();
        session
            .expect_ai()
            .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);

        let mut log = PluginLog::new("AiGuard");
        assert!(matches!(
            guard.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse { status, .. } if status == expected_status
        ));
    }
}

#[tokio::test]
async fn a_local_reject_renders_the_fixed_error_envelope() {
    let guard = guard(reject_rule(), "");
    let mut session = MockPluginSession::new();
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    session.expect_ctx().return_const(ctx_with_body());
    session.expect_get_method().return_const("POST".to_string());
    session
        .expect_get_path()
        .return_const("/v1/chat/completions".to_string());
    session.expect_read_request_body().returning(|_| Ok(body(PROMPT)));
    session.expect_set_response_header().returning(|_, _| Ok(()));
    ai.expect_record_ai_guard_decision().returning(|_, _| ());
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);

    let mut log = PluginLog::new("AiGuard");
    match guard.run_request(&mut session, &mut log).await {
        PluginRunningResult::ErrResponse { status, body } => {
            assert_eq!(status, 403);
            let body = body.expect("error body");
            let body = std::str::from_utf8(&body).expect("error body is utf-8");
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
        let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_get_path()
            .return_const("/v1/chat/completions".to_string());
        session.expect_read_request_body().returning(move |_| Ok(body(&prompt)));
        session.expect_set_response_header().returning(|_, _| Ok(()));
        ai.expect_record_ai_guard_decision().returning(|_, _| ());
        session
            .expect_ai()
            .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);

        let mut log = PluginLog::new("AiGuard");
        match guard.run_request(&mut session, &mut log).await {
            PluginRunningResult::ErrResponse { status, body } => {
                // A degraded completion, not an error status.
                assert_eq!(status, 200);
                let body = body.expect("refusal body");
                let body = std::str::from_utf8(&body).expect("refusal body is utf-8");
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
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
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
    ai.expect_record_ai_guard_decision().returning(|_, _| ());
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);

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
    session.expect_record_error().returning(|_| ());

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
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
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
    ai.expect_record_ai_guard_decision().times(1).returning(|_, _| ());
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
    let mut log = PluginLog::new("AiGuard");
    assert!(matches!(
        closed.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse { status: 403, .. }
    ));

    let open = guard(reject_rule(), "failOpen: true\n");
    let mut session = MockPluginSession::new();
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    session.expect_ctx().return_const(ctx_with_body());
    session.expect_get_method().return_const("POST".to_string());
    session
        .expect_get_path()
        .return_const("/v1/chat/completions".to_string());
    session
        .expect_read_request_body()
        .returning(move |_| Ok(body(malformed)));
    ai.expect_record_ai_guard_decision().times(1).returning(|_, _| ());
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
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
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    session.expect_ctx().return_const(ctx_with_body());
    session.expect_get_method().return_const("POST".to_string());
    session
        .expect_get_path()
        .return_const("/v1/chat/completions".to_string());
    session.expect_read_request_body().returning(move |_| Ok(body(clean)));
    session.expect_replace_request_body_spans().never();
    ai.expect_record_ai_guard_decision().returning(|_, _| ());
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);

    let mut log = PluginLog::new("AiGuard");
    assert_eq!(
        guard.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[test]
fn evaluator_projection_borrows_ordered_span_and_replacement_text() {
    let spans = vec![
        GuardSpan {
            key: GuardSpanKey {
                message_index: 0,
                part: GuardPart::Content,
            },
            role: None,
            context: String::new(),
            current: "unchanged".to_string(),
            token: 0..11,
        },
        GuardSpan {
            key: GuardSpanKey {
                message_index: 1,
                part: GuardPart::Content,
            },
            role: None,
            context: String::new(),
            current: "original".to_string(),
            token: 12..22,
        },
    ];
    let replacement_key = spans[1].key;
    let decision = GuardDecision::Replace(vec![(replacement_key, "replacement".to_string())]);

    let projected = apply_local(&spans, &decision);

    assert_eq!(
        projected.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
        spans.iter().map(|span| span.key).collect::<Vec<_>>()
    );
    assert_eq!(projected[0].1, "unchanged");
    assert_eq!(projected[0].1.as_ptr(), spans[0].current.as_ptr());
    let GuardDecision::Replace(replacements) = &decision else {
        panic!("decision must contain a replacement");
    };
    assert_eq!(projected[1].1, "replacement");
    assert_eq!(projected[1].1.as_ptr(), replacements[0].1.as_ptr());
}

#[test]
fn detached_ai_guard_cannot_construct_an_external_evaluator() {
    let external: AiGuardConfig =
        serde_json::from_value(serde_json::json!({"evaluator":{"target":{"name":"decision","port":8080},"request":{"path":{"template":"/decision"}}}})).unwrap();
    assert!(AiGuard::create(&external, "tenant/guard/#0").is_none());
    assert!(AiGuard::create(&config(reject_rule(), ""), "tenant/guard/#0").is_some());
}

const TWO_PROMPTS: &str =
    r#"{"model":"m","messages":[{"role":"user","content":"first"},{"role":"user","content":"second"}]}"#;

fn selected_prompts() -> Vec<GuardSpan> {
    select_spans(
        TWO_PROMPTS.as_bytes(),
        SpanLimits {
            max_spans: 512,
            max_span_bytes: 1024,
            max_total_span_bytes: 1024,
        },
    )
    .unwrap()
}

fn replacement_items(decision: &GuardDecision<GuardSpanKey>) -> &[(GuardSpanKey, String)] {
    match decision {
        GuardDecision::Replace(items) => items,
        GuardDecision::Pass => &[],
        GuardDecision::Reject => panic!("expected a replacement or pass"),
    }
}

#[test]
fn composed_disjoint_replacements_share_one_inclusive_budget() {
    let spans = selected_prompts();
    let decision = merge(
        GuardDecision::Replace(vec![(spans[0].key, "aaa".into())]),
        vec![(spans[1].key, "bbb".into())],
    );
    assert_eq!(validate_replacements(&spans, replacement_items(&decision), 6), Ok(()));
    assert_eq!(
        validate_replacements(&spans, replacement_items(&decision), 5),
        Err("replacement bounds")
    );
    // Each producer independently fits this cap, but the composed set does not.
    assert_eq!(
        validate_replacements(&spans, replacement_items(&decision), 3),
        Err("replacement bounds")
    );
}

#[test]
fn same_key_overrides_charge_only_the_surviving_text() {
    let spans = selected_prompts();
    for (external, cap, expected) in [
        ("a", 1, Ok(())),
        ("aaaa", 4, Ok(())),
        ("aaaa", 3, Err("replacement bounds")),
        ("", 0, Ok(())),
    ] {
        let decision = merge(
            GuardDecision::Replace(vec![(spans[0].key, "aaa".into())]),
            vec![(spans[0].key, external.into())],
        );
        let items = replacement_items(&decision);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].1, external);
        assert_eq!(validate_replacements(&spans, items, cap), expected);
    }
    assert_eq!(merge(GuardDecision::Pass, vec![]), GuardDecision::Pass);
    assert_eq!(validate_replacements(&spans, &[], 0), Ok(()));
}

#[test]
fn final_admission_checks_keys_before_counting_decoded_utf8_bytes() {
    let spans = selected_prompts();
    let mut unknown = spans[0].key;
    unknown.message_index = 9;
    assert_eq!(
        validate_replacements(&spans, &[(unknown, "too large".into())], 0),
        Err("evaluator bad key")
    );
    assert_eq!(
        validate_replacements(&spans, &[(spans[0].key, "".into()), (spans[0].key, "".into())], 0),
        Err("evaluator bad key")
    );
    assert_eq!(
        validate_replacements(&spans, &vec![(spans[0].key, String::new()); 3], 0),
        Err("evaluator bad key")
    );
    // Order need not be canonical. UTF-8 bytes count, JSON quoting/escapes do not.
    let items = vec![(spans[1].key, "é€".into()), (spans[0].key, "\n\"".into())];
    assert_eq!(validate_replacements(&spans, &items, 7), Ok(()));
    assert_eq!(validate_replacements(&spans, &items, 6), Err("replacement bounds"));
    assert_eq!(
        validate_replacements(&spans, &[(spans[0].key, String::new())], 0),
        Ok(())
    );
}

#[tokio::test]
async fn a_composed_budget_failure_never_publishes_and_latches_none() {
    let spans = selected_prompts();
    for fail_open in [false, true] {
        for (mode, status) in [("error", 403), ("contentFilter", 200)] {
            let guard = guard(
                reject_rule(),
                &format!("maxReplacementBytes: 3\nfailOpen: {fail_open}\nrejectionMode: {mode}\n"),
            );
            let (mut storage, view) = BodyStorage::new(1024, 4096);
            storage
                .write(&Bytes::from_static(TWO_PROMPTS.as_bytes()))
                .await
                .unwrap();
            storage.finish().await.unwrap();
            let mut ctx = EdgionHttpContext::new();
            ctx.request_body.install(Arc::clone(&view));
            let generation = view.generation();
            let mut session = MockPluginSession::new();
            session.expect_ctx().return_const(ctx);
            session.expect_replace_request_body_spans().never();
            session.expect_set_response_header().returning(|_, _| Ok(()));
            let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
            ai.expect_record_ai_guard_decision()
                .times(1)
                .withf(|id, generation| id == "test/requestPlugins/#0" && generation.is_none())
                .returning(|_, _| ());
            session
                .expect_ai()
                .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
            let decision = merge(
                GuardDecision::Replace(vec![(spans[0].key, "aaa".into())]),
                vec![(spans[1].key, "bbb".into())],
            );
            let result = guard
                .finish_decision(
                    &mut session,
                    &mut PluginLog::new("AiGuard"),
                    FinalDecision {
                        body: TWO_PROMPTS.as_bytes(),
                        spans: &spans,
                        generation,
                        decision,
                        flagged: 0,
                    },
                )
                .await;
            if fail_open {
                assert_eq!(result, PluginRunningResult::GoodNext);
            } else {
                assert!(matches!(result, PluginRunningResult::ErrResponse { status: actual, .. } if actual == status));
            }
            assert_eq!(view.generation(), generation);
            assert!(!view.is_modified());
            assert_eq!(
                view.read_all().await.unwrap(),
                Bytes::from_static(TWO_PROMPTS.as_bytes())
            );
        }
    }
}

#[tokio::test]
async fn an_admitted_composed_decision_publishes_one_generation_bound_batch() {
    let spans = selected_prompts();
    let guard = guard(reject_rule(), "maxReplacementBytes: 4\n");
    let (mut storage, view) = BodyStorage::new(1024, 4096);
    storage
        .write(&Bytes::from_static(TWO_PROMPTS.as_bytes()))
        .await
        .unwrap();
    storage.finish().await.unwrap();
    let generation = view.generation();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body.install(Arc::clone(&view));
    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(ctx);
    let publish_view = Arc::clone(&view);
    session
        .expect_replace_request_body_spans()
        .times(1)
        .returning(move |expected, edits| {
            assert_eq!(expected, generation);
            assert_eq!(edits.len(), 2);
            assert_eq!(edits[0].1, Bytes::from_static(b"\"a\""));
            assert_eq!(edits[1].1, Bytes::from_static(b"\"bbb\""));
            publish_view
                .replace_spans(expected, edits)
                .map_err(|_| BodyReadError::Incomplete)
        });
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    ai.expect_record_ai_guard_decision()
        .times(1)
        .withf(move |_, settled| *settled == Some(generation + 1))
        .returning(|_, _| ());
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
    let decision = merge(
        GuardDecision::Replace(vec![(spans[0].key, "aaa".into())]),
        vec![(spans[0].key, "a".into()), (spans[1].key, "bbb".into())],
    );
    let mut log = PluginLog::new("AiGuard");
    assert_eq!(
        guard
            .finish_decision(
                &mut session,
                &mut log,
                FinalDecision {
                    body: TWO_PROMPTS.as_bytes(),
                    spans: &spans,
                    generation,
                    decision,
                    flagged: 0,
                }
            )
            .await,
        PluginRunningResult::GoodNext
    );
    assert!(log.contains("OK masked 2"));
    let settled: serde_json::Value = serde_json::from_slice(&view.read_all().await.unwrap()).unwrap();
    assert_eq!(settled["messages"][0]["content"], "a");
    assert_eq!(settled["messages"][1]["content"], "bbb");
    assert_eq!(view.generation(), generation + 1);
}

#[tokio::test]
async fn composed_publication_faults_remain_stricter_than_fail_open() {
    let spans = selected_prompts();
    for (failure, status) in [
        (BodyReadError::GenerationConflict, 500),
        (BodyReadError::BufferLimitExceeded { limit: 1 }, 413),
    ] {
        let guard = guard(reject_rule(), "maxReplacementBytes: 6\nfailOpen: true\n");
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(ctx_with_body());
        session.expect_record_error().returning(|_| ());
        session.expect_set_response_header().returning(|_, _| Ok(()));
        session
            .expect_replace_request_body_spans()
            .times(1)
            .return_once(move |_, _| Err(failure));
        let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
        ai.expect_record_ai_guard_decision().never();
        session
            .expect_ai()
            .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
        let decision = merge(
            GuardDecision::Replace(vec![(spans[0].key, "aaa".into())]),
            vec![(spans[1].key, "bbb".into())],
        );
        let result = guard
            .finish_decision(
                &mut session,
                &mut PluginLog::new("AiGuard"),
                FinalDecision {
                    body: TWO_PROMPTS.as_bytes(),
                    spans: &spans,
                    generation: 0,
                    decision,
                    flagged: 0,
                },
            )
            .await;
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: actual, .. } if actual == status));
    }
}

#[tokio::test]
async fn pass_decision_keeps_flag_count_log_text() {
    let guard = guard(mask_rule(), "");
    for flagged in [0usize, 1, 42] {
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_replace_request_body_spans().never();
        let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
        ai.expect_record_ai_guard_decision().times(1).returning(|_, _| ());
        session
            .expect_ai()
            .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
        let mut log = PluginLog::new("AiGuard");
        assert_eq!(
            guard
                .finish_decision(
                    &mut session,
                    &mut log,
                    FinalDecision {
                        body: b"",
                        spans: &[],
                        generation: 0,
                        decision: GuardDecision::Pass,
                        flagged,
                    }
                )
                .await,
            PluginRunningResult::GoodNext
        );
        if flagged == 0 {
            assert!(log.contains("OK"));
            assert!(!log.contains("flagged"));
        } else {
            assert!(log.contains(&format!("OK flagged {flagged}")));
        }
    }
}
