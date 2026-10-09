use super::*;
use crate::ai::GenAiAttemptUsageSource;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::edgion_plugins::AiUsageObservation;
fn frame(kind: &str, data: &str) -> String {
    format!("event: {kind}\ndata: {data}\n\n")
}
fn normal() -> String {
    frame(
        "message_start",
        r#"{"type":"message_start","message":{"type":"message","model":"m","usage":{"input_tokens":5,"output_tokens":0,"cache_creation_input_tokens":3}}}"#,
    ) + &frame(
        "message_delta",
        r#"{"type":"message_delta","usage":{"input_tokens":8,"cache_read_input_tokens":2,"output_tokens":7},"delta":{}}"#,
    ) + &frame(
        "message_delta",
        r#"{"type":"message_delta","usage":{},"delta":{"stop_reason":"tool_use"}}"#,
    ) + &frame("message_stop", r#"{"type":"message_stop"}"#)
}
async fn run(body: &str, sse: bool, known_unknown: bool) -> Arc<AiUsageShared> {
    let shared = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Provider));
    if known_unknown {
        shared.mark_observation_unavailable();
    }
    let quota = Arc::new(QuotaAccumulator::new_for_test(1));
    let mut processor = MessagesProcessor::new(shared.clone(), Some(quota.clone()), sse, false, "m".into());
    let mut session = MockPluginSession::new();
    session.expect_record_error().returning(|_| {});
    let mut sink = ResponseBodySink::new();
    for (i, b) in body.as_bytes().iter().enumerate() {
        let bytes = Bytes::copy_from_slice(std::slice::from_ref(b));
        let mut chunk = Some(bytes.clone());
        processor
            .on_chunk(
                &mut chunk,
                i + 1 == body.len(),
                if sse {
                    ResponseStreamKind::Sse
                } else {
                    ResponseStreamKind::None
                },
                &mut sink,
                &mut session,
            )
            .await;
        assert_eq!(chunk, Some(bytes));
        assert!(!sink.is_terminated());
        assert!(sink.peek_extra().is_empty());
    }
    if !known_unknown && !shared.observation_unavailable() {
        assert_eq!(
            quota.observed_output_for_test(),
            shared.reported_usage().map_or(0, |u| u.output_tokens)
        );
    }
    shared
}
#[tokio::test]
async fn native_sse_late_input_and_cache_are_cumulative_without_character_billing() {
    let shared = run(
        &(": heartbeat\n\n".to_owned() + &normal() + ": final ping\n\n"),
        true,
        false,
    )
    .await;
    assert_eq!(shared.reported_tokens(), Some((13, 7)));
    assert_eq!(shared.output_chars(), 0);
    assert_eq!(
        shared.accounting_snapshot(99999, true).source,
        GenAiAttemptUsageSource::Reported
    );
    assert!(shared.permits_provider_success());
}

fn iteration_stream(start_model: &str, iterations: &str) -> String {
    let start = format!(
        r#"{{"type":"message_start","message":{{"type":"message",{start_model},"usage":{{"input_tokens":5,"output_tokens":0,"cache_creation_input_tokens":3}}}}}}"#
    );
    let delta = format!(
        r#"{{"type":"message_delta","usage":{{"input_tokens":8,"cache_read_input_tokens":2,"output_tokens":7,"iterations":{iterations}}},"delta":{{"stop_reason":"end_turn"}}}}"#
    );
    frame("message_start", &start)
        + &frame("message_delta", &delta)
        + &frame("message_stop", r#"{"type":"message_stop"}"#)
}

#[tokio::test]
async fn native_iteration_attribution_uses_the_actual_message_model_in_json_and_sse() {
    for (iterations, reported) in [
        (r#"[{"type":"message","model":"m-v1"}]"#, true),
        (r#"[{"type":"message","model":"m"}]"#, false),
        (r#"[{"type":"message"}]"#, false),
        (r#"[{"type":"message","model":"m-v1"},{"type":"message"}]"#, false),
        (r#"[{"type":"message"},{"type":"message","model":"m-v1"}]"#, false),
    ] {
        let json = format!(
            r#"{{"type":"message","model":"m-v1","stop_reason":"end_turn","usage":{{"input_tokens":8,"cache_creation_input_tokens":3,"cache_read_input_tokens":2,"output_tokens":7,"iterations":{iterations}}}}}"#
        );
        for (body, sse) in [(json, false), (iteration_stream(r#""model":"m-v1""#, iterations), true)] {
            let shared = run(&body, sse, false).await;
            let snapshot = shared.accounting_snapshot(999, true);
            assert_eq!(
                snapshot.source,
                if reported {
                    GenAiAttemptUsageSource::Reported
                } else {
                    GenAiAttemptUsageSource::Unknown
                },
                "{body}"
            );
            assert_eq!(shared.permits_provider_success(), reported);
            if reported {
                assert_eq!(shared.reported_tokens(), Some((13, 7)));
            } else {
                assert!(snapshot.usage.is_none());
            }
        }
    }
}

#[tokio::test]
async fn native_iteration_attribution_rejects_only_invalid_active_streams() {
    for (iterations, terminate) in [
        (r#"[{"type":"message","model":"m-v1"}]"#, false),
        (r#"[{"type":"message","model":"m"}]"#, true),
        (r#"[{"type":"message"}]"#, true),
    ] {
        let shared = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Provider));
        let quota = Arc::new(QuotaAccumulator::new_for_test(100));
        let mut processor = MessagesProcessor::new(shared.clone(), Some(quota), true, true, "m".into());
        let body = iteration_stream(r#""model":"wrong","model":"m-v1""#, iterations);
        let mut session = MockPluginSession::new();
        session.expect_record_error().returning(|_| {});
        session.expect_try_claim_response_terminal().return_const(true);
        session.expect_mark_response_terminal_emission().returning(|_| {});
        let mut sink = ResponseBodySink::new();
        let mut chunk = Some(Bytes::from(body.clone()));
        processor
            .on_chunk(&mut chunk, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;
        assert_eq!(sink.is_terminated(), terminate, "{iterations}");
        if terminate {
            let wire = String::from_utf8(sink.peek_extra()[0].to_vec()).unwrap();
            assert!(wire.contains("event: error\ndata: {"));
            assert!(!wire.contains("[DONE]"));
        } else {
            assert_eq!(chunk, Some(Bytes::from(body)));
            assert!(sink.peek_extra().is_empty());
        }
    }
}

#[tokio::test]
async fn native_start_model_fallback_and_invalid_replacements_remain_conservative() {
    let matching = r#"[{"type":"message","model":"m"}]"#;
    let fallback = iteration_stream(r#""id":"message-without-model""#, matching);
    let shared = run(&fallback, true, false).await;
    assert_eq!(shared.reported_tokens(), Some((13, 7)));
    assert!(shared.permits_provider_success());

    let actual = iteration_stream(r#""model":"m-v1""#, r#"[{"type":"message","model":"m-v1"}]"#);
    let duplicate = frame(
        "message_start",
        r#"{"type":"message_start","message":{"type":"message","model":"m","usage":{"input_tokens":5,"output_tokens":0}}}"#,
    );
    for body in [
        duplicate + &actual,
        frame(
            "message_start",
            r#"{"type":"message_delta","message":{"type":"message","model":"m-v1","usage":{"input_tokens":5,"output_tokens":0}}}"#,
        ) + &actual,
        frame(
            "message_start",
            r#"{"type":"message_start","message":{"type":"message","model":"m-v1","usage":{"input_tokens":-1,"output_tokens":0}}}"#,
        ) + &actual,
    ] {
        let shared = run(&body, true, false).await;
        assert_eq!(
            shared.accounting_snapshot(999, true).source,
            GenAiAttemptUsageSource::Unknown
        );
        assert!(!shared.permits_provider_success());
    }
    let sticky = run(&actual, true, true).await;
    assert_eq!(
        sticky.accounting_snapshot(999, true).source,
        GenAiAttemptUsageSource::Unknown
    );
    assert!(!sticky.permits_provider_success());
}
#[tokio::test]
async fn unknown_usage_is_sticky_and_invalid_streams_do_not_teach_success() {
    for body in [
        normal() + "data: {",
        normal().replace("event: message_stop", "event: mismatch"),
        normal().replace("\"stop_reason\":\"tool_use\"", "\"stop_reason\":\"future\""),
        frame(
            "message_start",
            r#"{"type":"message_start","message":{"usage":{"input_tokens":5,"output_tokens":0}}}"#,
        ) + &frame(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        ) + &frame("message_stop", r#"{"type":"message_stop"}"#),
    ] {
        let shared = run(&body, true, false).await;
        assert_eq!(
            shared.accounting_snapshot(99, true).source,
            GenAiAttemptUsageSource::Unknown
        );
        assert!(!shared.permits_provider_success());
    }
    let shared = run(&normal(), true, true).await;
    assert!(shared.reported_usage().is_none());
    assert!(!shared.permits_provider_success());
}
#[tokio::test]
async fn native_json_stop_reasons_are_success_and_provider_error_is_failure() {
    for reason in ["end_turn", "tool_use", "pause_turn", "max_tokens", "refusal"] {
        let shared = run(
            &format!(r#"{{"type":"message","stop_reason":"{reason}","usage":{{"input_tokens":5,"output_tokens":7}}}}"#),
            false,
            false,
        )
        .await;
        assert_eq!(shared.reported_tokens(), Some((5, 7)));
        assert!(shared.permits_provider_success());
    }
    let shared = run(
        &frame(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"fixed"}}"#,
        ),
        true,
        false,
    )
    .await;
    assert_eq!(shared.response_outcome(), ResponseOutcome::Failed);
    assert!(!shared.permits_provider_success());
}
#[tokio::test]
async fn incomplete_document_and_response_content_never_supply_estimates() {
    let shared = run(
        r#"{"type":"message","content":[{"type":"text","text":"large repeated output"}],"stop_reason":"end_turn"}"#,
        false,
        false,
    )
    .await;
    assert_eq!(
        shared.accounting_snapshot(100, true).source,
        GenAiAttemptUsageSource::Unknown
    );
    assert_eq!(shared.output_chars(), 0);
}

#[tokio::test]
async fn native_reject_owns_one_error_event_while_throttle_does_not_cut() {
    use crate::response::terminal::TerminalEmission;
    for reject in [true, false] {
        let shared = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Provider));
        let quota = Arc::new(QuotaAccumulator::new_for_test(1));
        quota.mark_over_limit();
        let mut processor = MessagesProcessor::new(shared.clone(), Some(quota), true, reject, "m".into());
        let mut session = MockPluginSession::new();
        session.expect_record_error().returning(|_| {});
        if reject {
            session.expect_try_claim_response_terminal().return_const(true);
            session
                .expect_mark_response_terminal_emission()
                .withf(|e| *e == TerminalEmission::Rendered)
                .returning(|_| {});
        }
        let mut chunk = Some(Bytes::from_static(b": heartbeat\n\n"));
        let mut sink = ResponseBodySink::new();
        processor
            .on_chunk(&mut chunk, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;
        assert_eq!(sink.is_terminated(), reject);
        assert_eq!(shared.gateway_terminated(), reject);
        if reject {
            let events = sink.peek_extra();
            assert_eq!(events.len(), 1);
            let event = std::str::from_utf8(&events[0]).unwrap();
            assert!(event.contains("event: error"));
            assert!(event.contains("\"type\":\"rate_limit_error\""));
            assert!(!event.contains("[DONE]"));
        } else {
            assert!(sink.peek_extra().is_empty());
        }
    }
}

#[tokio::test]
async fn provider_error_remains_failed_after_later_normal_terminal() {
    let wire = normal().replace(
        "event: message_stop",
        &(frame("error", r#"{"type":"error","error":{"type":"overloaded_error"}}"#) + "event: message_stop"),
    );
    let shared = run(&wire, true, false).await;
    assert_eq!(shared.response_outcome(), ResponseOutcome::Failed);
    assert_eq!(
        shared.provider_failure(),
        Some(crate::ai::GenAiAttemptFailure::ProviderFailed)
    );
    assert!(shared.observation_unavailable());
}

#[tokio::test]
async fn later_unknown_stop_reason_cannot_reuse_an_earlier_valid_stop() {
    let wire = normal().replace(
        "event: message_stop",
        &(frame(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"future_stop"}}"#,
        ) + "event: message_stop"),
    );
    assert!(run(&wire, true, false).await.observation_unavailable());
    let wire = normal().replace(
        r#""type":"message","model":"m""#,
        r#""type":"future_message","model":"m""#,
    );
    assert!(run(&wire, true, false).await.observation_unavailable());
}

#[tokio::test]
async fn explicit_required_and_structural_null_patches_cannot_reuse_prior_usage() {
    for patch in [
        r#"{"usage":{"output_tokens":null},"delta":{"stop_reason":"end_turn"}}"#,
        r#"{"usage":null,"delta":{"stop_reason":"end_turn"}}"#,
        r#"{"usage":{"output_tokens":7},"delta":null}"#,
    ] {
        let value: serde_json::Value = serde_json::from_str(patch).unwrap();
        let mut value = value.as_object().unwrap().clone();
        value.insert("type".into(), serde_json::json!("message_delta"));
        let wire = normal().replace(
            "event: message_stop",
            &(frame("message_delta", &serde_json::Value::Object(value).to_string()) + "event: message_stop"),
        );
        let shared = run(&wire, true, false).await;
        assert!(shared.observation_unavailable(), "{patch}");
        assert_eq!(
            shared.accounting_snapshot(99, true).source,
            GenAiAttemptUsageSource::Unknown
        );
        assert!(!shared.permits_provider_success());
    }
    let wire = normal().replace(r#""input_tokens":5"#, r#""input_tokens":null"#);
    assert!(run(&wire, true, false).await.observation_unavailable());
}

#[tokio::test]
async fn recognized_sse_envelopes_require_matching_payload_types() {
    for payload in [
        r#"{"type":"message"}"#,
        r#"{"type":"future"}"#,
        r#"{"usage":{"output_tokens":7}}"#,
        r#"{"type":null}"#,
        r#"{"type":"message_stop"}"#,
    ] {
        let wire = normal().replace(
            "event: message_stop",
            &(frame("message_delta", payload) + "event: message_stop"),
        );
        let shared = run(&wire, true, false).await;
        assert!(shared.observation_unavailable());
        assert_eq!(
            shared.accounting_snapshot(99, true).source,
            GenAiAttemptUsageSource::Unknown
        );
        assert!(!shared.permits_provider_success());
    }
}

#[tokio::test]
async fn unknown_sse_events_cannot_supply_or_poison_native_lifecycle_metadata() {
    for payload in [
        r#"{"type":"error"}"#,
        r#"{"type":"message_start","message":{"type":"message","usage":{"input_tokens":999,"output_tokens":999}}}"#,
        r#"{"type":"message_delta","usage":{"input_tokens":999,"output_tokens":999}}"#,
        "opaque native extension",
    ] {
        let wire = normal().replace(
            "event: message_stop",
            &(frame("future_extension", payload) + "event: message_stop"),
        );
        let shared = run(&wire, true, false).await;
        assert_eq!(shared.reported_tokens(), Some((13, 7)));
        assert!(shared.permits_provider_success());
    }
    let wire = normal().replace("event: message_stop", "event: future_extension");
    let shared = run(&wire, true, false).await;
    assert!(shared.observation_unavailable());
    assert!(!shared.permits_provider_success());
}
