//! Request-local arbitration and bounded client framing for AI stream terminals.

use bytes::Bytes;
use pingora_proxy::ResponseBodySink;

use crate::ctx::ResponseStreamKind;

const MAX_TERMINAL_EVENT_BYTES: usize = 4 * 1024;
const GUARDRAIL_REJECTED_EVENT: &[u8] = b"event: error\ndata: {\"error\":{\"message\":\"The response was blocked by a content policy.\",\"type\":\"content_filter_error\",\"code\":\"content_policy_violation\"}}\n\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiClientStreamDialect {
    OpenAiCompatibleSse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseProcessorSlot {
    Ai,
    Semantic,
    Ordinary(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseTerminalCause {
    TranslationFailure,
    QuotaExceeded,
    QuotaUnavailable,
    GuardrailRejected,
    GuardrailDependency,
    ProcessorFailure,
    ProviderCompleted,
    LegacyProcessor,
}

impl ResponseTerminalCause {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::TranslationFailure => "translation-failure",
            Self::QuotaExceeded => "quota-exceeded",
            Self::QuotaUnavailable => "quota-unavailable",
            Self::GuardrailRejected => "guardrail-rejected",
            Self::GuardrailDependency => "guardrail-dependency",
            Self::ProcessorFailure => "processor-failure",
            Self::ProviderCompleted => "provider-completed",
            Self::LegacyProcessor => "legacy-processor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalEmission {
    Pending,
    Rendered,
    NoEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseClose {
    GatewayTerminate,
    NaturalEos,
    UpstreamAbort,
    DownstreamCancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ResponseTerminalState {
    #[default]
    Open,
    Claimed {
        winner: ResponseProcessorSlot,
        cause: ResponseTerminalCause,
        emission: TerminalEmission,
    },
    Closed {
        winner: Option<ResponseProcessorSlot>,
        cause: Option<ResponseTerminalCause>,
        emission: Option<TerminalEmission>,
        close: ResponseClose,
    },
}

impl ResponseTerminalState {
    pub(crate) fn try_claim(&mut self, winner: ResponseProcessorSlot, cause: ResponseTerminalCause) -> bool {
        match *self {
            Self::Open => {
                *self = Self::Claimed {
                    winner,
                    cause,
                    emission: TerminalEmission::Pending,
                };
                true
            }
            Self::Claimed {
                winner: current,
                cause: current_cause,
                ..
            } => current == winner && current_cause == cause,
            Self::Closed { .. } => false,
        }
    }

    pub(crate) fn winner(&self) -> Option<ResponseProcessorSlot> {
        match *self {
            Self::Claimed { winner, .. } => Some(winner),
            Self::Closed { winner, .. } => winner,
            Self::Open => None,
        }
    }

    pub(crate) fn winner_cause(&self) -> Option<ResponseTerminalCause> {
        match *self {
            Self::Claimed { cause, .. } => Some(cause),
            Self::Closed { cause, .. } => cause,
            Self::Open => None,
        }
    }

    pub(crate) fn mark_emission(&mut self, emission: TerminalEmission) {
        if let Self::Claimed { emission: current, .. } = self {
            *current = emission;
        }
    }

    pub(crate) fn close_gateway(&mut self) {
        if let Self::Claimed {
            winner,
            cause,
            emission,
        } = *self
        {
            *self = Self::Closed {
                winner: Some(winner),
                cause: Some(cause),
                emission: Some(if emission == TerminalEmission::Pending {
                    TerminalEmission::NoEvent
                } else {
                    emission
                }),
                close: ResponseClose::GatewayTerminate,
            };
        }
    }

    pub(crate) fn settle_pending_without_event(&mut self) {
        if let Self::Claimed { emission, .. } = self {
            if *emission == TerminalEmission::Pending {
                *emission = TerminalEmission::NoEvent;
            }
        }
    }

    pub(crate) fn close_open(&mut self, close: ResponseClose) {
        if matches!(self, Self::Open) {
            *self = Self::Closed {
                winner: None,
                cause: None,
                emission: None,
                close,
            };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FinalResponseEligibility {
    pub(crate) method_allows_body: bool,
    pub(crate) status_allows_body: bool,
    pub(crate) stream_kind: ResponseStreamKind,
}

impl FinalResponseEligibility {
    pub(crate) fn allows_sse(self) -> bool {
        self.method_allows_body && self.status_allows_body && self.stream_kind == ResponseStreamKind::Sse
    }
}

pub(crate) fn emit_guardrail_rejection(
    dialect: Option<AiClientStreamDialect>,
    eligibility: Option<FinalResponseEligibility>,
    sink: &mut ResponseBodySink,
) -> TerminalEmission {
    if dialect != Some(AiClientStreamDialect::OpenAiCompatibleSse)
        || !eligibility.is_some_and(FinalResponseEligibility::allows_sse)
    {
        return TerminalEmission::NoEvent;
    }
    let Some(event) = bounded_terminal_event(GUARDRAIL_REJECTED_EVENT) else {
        return TerminalEmission::NoEvent;
    };
    if sink.push(event).is_ok() {
        TerminalEmission::Rendered
    } else {
        TerminalEmission::NoEvent
    }
}

fn bounded_terminal_event(event: &'static [u8]) -> Option<Bytes> {
    (event.len() <= MAX_TERMINAL_EVENT_BYTES).then(|| Bytes::from_static(event))
}

#[cfg(test)]
mod tests {
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
}
