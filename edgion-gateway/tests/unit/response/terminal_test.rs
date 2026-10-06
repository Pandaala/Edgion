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
fn natural_eos_cannot_be_reclassified_by_late_logging() {
    let mut state = ResponseTerminalState::Open;
    state.close_open(ResponseClose::NaturalEos);
    let closed = state;
    state.close_open(ResponseClose::DownstreamCancel);
    state.close_open(ResponseClose::UpstreamAbort);
    assert_eq!(state, closed);
}
