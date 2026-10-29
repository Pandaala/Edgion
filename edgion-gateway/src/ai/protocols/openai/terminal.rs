//! Bounded OpenAI-compatible stream terminal rendering.

use crate::response::terminal::{FinalResponseEligibility, TerminalEmission};
use bytes::Bytes;
use pingora_proxy::ResponseBodySink;

const MAX_TERMINAL_EVENT_BYTES: usize = 4 * 1024;
const GUARDRAIL_REJECTED_EVENT: &[u8] = b"event: error\ndata: {\"error\":{\"message\":\"The response was blocked by a content policy.\",\"type\":\"content_filter_error\",\"code\":\"content_policy_violation\"}}\n\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiClientStreamDialect {
    OpenAiCompatibleSse,
}

pub(crate) fn emit_guardrail_rejection(
    dialect: AiClientStreamDialect,
    eligibility: FinalResponseEligibility,
    sink: &mut ResponseBodySink,
) -> TerminalEmission {
    if dialect != AiClientStreamDialect::OpenAiCompatibleSse || !eligibility.allows_sse() {
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
#[path = "../../../../tests/unit/ai/protocols/openai/terminal_test.rs"]
mod tests;
