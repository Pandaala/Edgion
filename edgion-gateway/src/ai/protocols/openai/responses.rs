//! Native Responses event metadata and bounded local stream errors.

use bytes::Bytes;
use serde::Serialize;

use super::error::AiProxyError;
use super::json_usage::{JsonUsageObserver, JsonUsageOutcome, ResponseOutcome, ResponsesEventKind};
use crate::ai::ReportedAiUsage;
use crate::sse::SseEventRef;

pub(crate) struct EventObservation {
    pub outcome: Option<ResponseOutcome>,
    pub usage: Option<ReportedAiUsage>,
    pub sequence: Option<u64>,
    pub unavailable: bool,
}

pub(crate) fn observe_event(event: &SseEventRef<'_>) -> EventObservation {
    // This is one decoder-bounded event, never a response/document aggregate.
    let mut observer = JsonUsageObserver::responses(true);
    let data = event.semantic_data();
    let data_valid = data.is_ok();
    let result = match data {
        Ok(data) => {
            observer.feed(data.as_bytes());
            observer.finish()
        }
        Err(()) => JsonUsageOutcome::Missing,
    };
    let kind = observer.event_kind();
    let outcome = kind.outcome();
    let validated = data_valid && !matches!(result, JsonUsageOutcome::Unavailable(_));
    let matching =
        outcome.is_none() || kind == ResponsesEventKind::Error || outcome == Some(observer.response_outcome());
    EventObservation {
        outcome: (validated && matching).then_some(outcome).flatten(),
        usage: if outcome.is_some() && validated && matching {
            match result {
                JsonUsageOutcome::Reported(usage) => Some(usage),
                _ => None,
            }
        } else {
            None
        },
        sequence: validated.then(|| observer.sequence_number()).flatten(),
        unavailable: !validated || !matching,
    }
}

#[derive(Serialize)]
struct LocalError {
    #[serde(rename = "type")]
    type_: &'static str,
    code: &'static str,
    message: &'static str,
    param: Option<&'static str>,
    sequence_number: u64,
}

pub(crate) fn local_error(error: AiProxyError, last_sequence: Option<u64>) -> Option<Bytes> {
    let sequence_number = match last_sequence {
        Some(value) => value.checked_add(1)?,
        None => 0,
    };
    let (_, message, _, code) = error.fields();
    let event = LocalError {
        type_: "error",
        code,
        message,
        param: None,
        sequence_number,
    };
    let data = serde_json::to_string(&event).ok()?;
    let event = format!("\n\nevent: error\ndata: {data}\n\n");
    (event.len() <= 4096).then(|| Bytes::from(event))
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/protocols/openai/responses_test.rs"]
mod tests;
