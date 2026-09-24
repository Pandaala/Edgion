use super::*;

#[test]
fn terminal_claim_is_first_wins_and_idempotent() {
    let mut state = ResponseTerminalState::Open;
    assert!(state.try_claim(
        ResponseProcessorSlot::Semantic,
        ResponseTerminalCause::GuardrailRejected
    ));
    assert!(state.try_claim(
        ResponseProcessorSlot::Semantic,
        ResponseTerminalCause::GuardrailRejected
    ));
    assert!(!state.try_claim(
        ResponseProcessorSlot::Ordinary(0),
        ResponseTerminalCause::ProcessorFailure
    ));
    state.mark_emission(TerminalEmission::Rendered);
    state.close_gateway();
    assert_eq!(
        state,
        ResponseTerminalState::Closed {
            winner: Some(ResponseProcessorSlot::Semantic),
            cause: Some(ResponseTerminalCause::GuardrailRejected),
            emission: Some(TerminalEmission::Rendered),
            close: ResponseClose::GatewayTerminate,
        }
    );
}

#[test]
fn bodyless_or_non_sse_terminal_is_not_renderable() {
    let bodyless = FinalResponseEligibility {
        method_allows_body: false,
        status_allows_body: true,
        stream_kind: ResponseStreamKind::Sse,
    };
    assert!(!bodyless.allows_sse());
    let non_sse = FinalResponseEligibility {
        method_allows_body: true,
        status_allows_body: true,
        stream_kind: ResponseStreamKind::None,
    };
    assert!(!non_sse.allows_sse());
    let bodyless_status = FinalResponseEligibility {
        method_allows_body: true,
        status_allows_body: false,
        stream_kind: ResponseStreamKind::Sse,
    };
    assert!(!bodyless_status.allows_sse());
}

#[test]
fn guardrail_refusal_is_one_bounded_terminal_sse_record() {
    let eligibility = FinalResponseEligibility {
        method_allows_body: true,
        status_allows_body: true,
        stream_kind: ResponseStreamKind::Sse,
    };
    let mut sink = ResponseBodySink::new();
    assert_eq!(
        emit_guardrail_rejection(
            Some(AiClientStreamDialect::OpenAiCompatibleSse),
            Some(eligibility),
            &mut sink
        ),
        TerminalEmission::Rendered
    );
    assert_eq!(sink.peek_extra(), &[Bytes::from_static(GUARDRAIL_REJECTED_EVENT)]);
    let wire = std::str::from_utf8(&sink.peek_extra()[0]).unwrap();
    assert_eq!(wire.matches("event: error\n").count(), 1);
    assert_eq!(wire.matches("content_policy_violation").count(), 1);
    assert!(!wire.contains("[DONE]"));
}

#[test]
fn bodyless_refusal_never_reserves_sink_output() {
    let eligibility = FinalResponseEligibility {
        method_allows_body: false,
        status_allows_body: true,
        stream_kind: ResponseStreamKind::Sse,
    };
    let mut sink = ResponseBodySink::new();
    assert_eq!(
        emit_guardrail_rejection(
            Some(AiClientStreamDialect::OpenAiCompatibleSse),
            Some(eligibility),
            &mut sink
        ),
        TerminalEmission::NoEvent
    );
    assert!(sink.peek_extra().is_empty());
}

#[test]
fn exhausted_sink_keeps_the_same_terminal_without_a_fallback_event() {
    let eligibility = FinalResponseEligibility {
        method_allows_body: true,
        status_allows_body: true,
        stream_kind: ResponseStreamKind::Sse,
    };
    let mut sink = ResponseBodySink::new();
    sink.push(Bytes::from(vec![0; pingora_proxy::RESPONSE_BODY_EMIT_BUDGET]))
        .expect("the exact sink budget must be accepted");
    assert_eq!(
        emit_guardrail_rejection(
            Some(AiClientStreamDialect::OpenAiCompatibleSse),
            Some(eligibility),
            &mut sink
        ),
        TerminalEmission::NoEvent
    );
    assert_eq!(sink.peek_extra().len(), 1);
}

#[test]
fn terminal_renderer_cap_is_inclusive() {
    let exact = Box::leak(vec![b'x'; MAX_TERMINAL_EVENT_BYTES].into_boxed_slice());
    let plus_one = Box::leak(vec![b'x'; MAX_TERMINAL_EVENT_BYTES + 1].into_boxed_slice());
    assert_eq!(
        bounded_terminal_event(exact).map(|event| event.len()),
        Some(MAX_TERMINAL_EVENT_BYTES)
    );
    assert!(bounded_terminal_event(plus_one).is_none());
}

#[test]
fn natural_eos_cannot_be_reclassified_by_late_logging() {
    let mut state = ResponseTerminalState::Open;
    state.close_open(ResponseClose::NaturalEos);
    let closed = state;
    state.close_open(ResponseClose::DownstreamCancel);
    state.close_open(ResponseClose::UpstreamAbort);
    assert_eq!(state, closed);
}
