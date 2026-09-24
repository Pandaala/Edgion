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
#[path = "../../tests/unit/ai/terminal_test.rs"]
mod tests;
