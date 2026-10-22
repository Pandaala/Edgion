//! Incremental, bounded Chat JSON usage selection. Values stay provisional until EOS.
use crate::ai::ReportedAiUsage;
use edgion_waf::seclang::{
    JsonDiscardedStringSummary, JsonStreamError, JsonStreamInput, JsonStreamLimits, JsonStreamSelective,
    JsonStreamSelectiveLimits, JsonStreamSelectiveSink, JsonStreamSinkError, JsonStreamToken,
};

pub(crate) const MAX_USAGE_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ResponseOutcome {
    #[default]
    Unknown,
    Completed,
    Incomplete,
    Failed,
    Cancelled,
}
impl ResponseOutcome {
    fn from_status(status: &str) -> Self {
        match status {
            "completed" => Self::Completed,
            "incomplete" => Self::Incomplete,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ResponsesEventKind {
    #[default]
    Other,
    Completed,
    Incomplete,
    Failed,
    Error,
}
impl ResponsesEventKind {
    pub(crate) fn outcome(self) -> Option<ResponseOutcome> {
        match self {
            Self::Completed => Some(ResponseOutcome::Completed),
            Self::Incomplete => Some(ResponseOutcome::Incomplete),
            Self::Failed | Self::Error => Some(ResponseOutcome::Failed),
            Self::Other => None,
        }
    }
}

fn limits() -> JsonStreamSelectiveLimits {
    JsonStreamSelectiveLimits {
        lexical: JsonStreamLimits {
            max_source_bytes: MAX_USAGE_SOURCE_BYTES,
            max_raw_token_bytes: 1538,
            max_decoded_bytes: MAX_USAGE_SOURCE_BYTES,
            max_events: 131_072,
            max_depth: MAX_DEPTH,
            max_retained_bytes: 8192,
            max_work: 256 * 1024 * 1024,
        },
        max_key_bytes: 256,
        max_number_bytes: 64,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JsonUsageOutcome {
    Reported(ReportedAiUsage),
    Missing,
    Unavailable(JsonStreamError),
}

pub(crate) struct JsonUsageObserver {
    parser: JsonStreamSelective,
    selector: Selector,
    outcome: Option<JsonUsageOutcome>,
}
impl JsonUsageObserver {
    pub(crate) fn new() -> Self {
        Self {
            parser: JsonStreamSelective::new(limits()),
            selector: Selector::default(),
            outcome: None,
        }
    }
    pub(crate) fn responses(event: bool) -> Self {
        let mut observer = Self::new();
        observer.selector.responses = true;
        observer.selector.event = event;
        observer
    }
    pub(crate) fn response_outcome(&self) -> ResponseOutcome {
        self.selector.status
    }
    pub(crate) fn event_kind(&self) -> ResponsesEventKind {
        self.selector.event_kind
    }
    pub(crate) fn sequence_number(&self) -> Option<u64> {
        self.selector.sequence
    }
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        if self.outcome.is_none() {
            if let Err(error) = self.parser.feed(JsonStreamInput::Data(bytes), &mut self.selector) {
                self.outcome = Some(JsonUsageOutcome::Unavailable(error));
            }
        }
    }
    pub(crate) fn finish(&mut self) -> JsonUsageOutcome {
        if let Some(outcome) = self.outcome {
            return outcome;
        }
        let outcome = match self.parser.feed(JsonStreamInput::Complete, &mut self.selector) {
            Err(error) => JsonUsageOutcome::Unavailable(error),
            Ok(()) => self
                .selector
                .usage()
                .map_or(JsonUsageOutcome::Missing, JsonUsageOutcome::Reported),
        };
        self.outcome = Some(outcome);
        outcome
    }
    pub(crate) fn failed(&self) -> bool {
        matches!(self.outcome, Some(JsonUsageOutcome::Unavailable(_)))
    }
}

#[derive(Clone, Copy, Default)]
enum Role {
    Root,
    Response,
    Usage,
    Details,
    #[default]
    Other,
}
#[derive(Clone, Copy, Default)]
enum Key {
    Usage,
    Response,
    Status,
    Type,
    Sequence,
    Details,
    Input,
    Output,
    Cached,
    Read,
    Write,
    #[default]
    Other,
}
#[derive(Clone, Copy, Default)]
struct Frame {
    role: Role,
    pending: Key,
}
#[derive(Default)]
struct Candidate {
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
    read: Option<u64>,
    write: Option<u64>,
}
struct Selector {
    frames: [Frame; MAX_DEPTH + 1],
    depth: usize,
    candidate: Candidate,
    responses: bool,
    event: bool,
    status: ResponseOutcome,
    event_kind: ResponsesEventKind,
    sequence: Option<u64>,
}
impl Default for Selector {
    fn default() -> Self {
        Self {
            frames: [Frame::default(); MAX_DEPTH + 1],
            depth: 0,
            candidate: Candidate::default(),
            responses: false,
            event: false,
            status: ResponseOutcome::Unknown,
            event_kind: ResponsesEventKind::Other,
            sequence: None,
        }
    }
}
impl Selector {
    fn target(&mut self) -> Key {
        let Some(frame) = self.depth.checked_sub(1).map(|index| &mut self.frames[index]) else {
            return Key::Other;
        };
        let key = std::mem::take(&mut frame.pending);
        match key {
            Key::Usage => self.candidate = Candidate::default(),
            Key::Response => {
                self.candidate = Candidate::default();
                self.status = ResponseOutcome::Unknown;
            }
            Key::Status => self.status = ResponseOutcome::Unknown,
            Key::Type => self.event_kind = ResponsesEventKind::Other,
            Key::Sequence => self.sequence = None,
            Key::Details | Key::Cached => self.candidate.cached = None,
            Key::Input => self.candidate.input = None,
            Key::Output => self.candidate.output = None,
            Key::Read => self.candidate.read = None,
            Key::Write => self.candidate.write = None,
            Key::Other => {}
        }
        key
    }
    fn usage(&self) -> Option<ReportedAiUsage> {
        let input_tokens = self.candidate.input?;
        let output_tokens = self.candidate.output?;
        if self.responses
            && (input_tokens.checked_add(output_tokens).is_none()
                || self.candidate.cached.is_some_and(|cached| cached > input_tokens))
        {
            return None;
        }
        Some(ReportedAiUsage {
            input_tokens,
            output_tokens,
            cache_read_input_tokens: self.candidate.cached.or(self.candidate.read).unwrap_or(0),
            cache_write_input_tokens: self.candidate.write.unwrap_or(0),
        })
    }
}
impl JsonStreamSelectiveSink for Selector {
    fn on_token(&mut self, token: JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError> {
        match token {
            JsonStreamToken::Key(key) => {
                let frame = self
                    .frames
                    .get_mut(self.depth.checked_sub(1).ok_or(JsonStreamSinkError)?)
                    .ok_or(JsonStreamSinkError)?;
                frame.pending = match (frame.role, key) {
                    (Role::Root, "response") if self.responses && self.event => Key::Response,
                    (Role::Root, "type") if self.responses && self.event => Key::Type,
                    (Role::Root, "sequence_number") if self.responses && self.event => Key::Sequence,
                    (Role::Root, "status") if self.responses && !self.event => Key::Status,
                    (Role::Response, "status") => Key::Status,
                    (Role::Root, "usage") if !self.event => Key::Usage,
                    (Role::Response, "usage") => Key::Usage,
                    (Role::Usage, "input_tokens") if self.responses => Key::Input,
                    (Role::Usage, "output_tokens") if self.responses => Key::Output,
                    (Role::Usage, "input_tokens_details") if self.responses => Key::Details,
                    (Role::Usage, "prompt_tokens") if !self.responses => Key::Input,
                    (Role::Usage, "completion_tokens") if !self.responses => Key::Output,
                    (Role::Usage, "prompt_tokens_details") if !self.responses => Key::Details,
                    (Role::Usage, "cache_read_input_tokens") => Key::Read,
                    (Role::Usage, "cache_creation_input_tokens") => Key::Write,
                    (Role::Details, "cached_tokens") => Key::Cached,
                    _ => Key::Other,
                };
            }
            JsonStreamToken::ObjectStart | JsonStreamToken::ArrayStart => {
                let object = matches!(token, JsonStreamToken::ObjectStart);
                let role = if self.depth == 0 {
                    if !object {
                        return Err(JsonStreamSinkError);
                    }
                    Role::Root
                } else {
                    match (self.target(), object) {
                        (Key::Response, true) => Role::Response,
                        (Key::Usage, true) => Role::Usage,
                        (Key::Details, true) => Role::Details,
                        _ => Role::Other,
                    }
                };
                let frame = self.frames.get_mut(self.depth).ok_or(JsonStreamSinkError)?;
                *frame = Frame {
                    role,
                    pending: Key::Other,
                };
                self.depth += 1;
            }
            JsonStreamToken::ObjectEnd | JsonStreamToken::ArrayEnd => {
                self.depth = self.depth.checked_sub(1).ok_or(JsonStreamSinkError)?
            }
            JsonStreamToken::Boolean(_) | JsonStreamToken::Null => {
                if self.depth == 0 {
                    return Err(JsonStreamSinkError);
                }
                self.target();
            }
            JsonStreamToken::String(_) | JsonStreamToken::Number { .. } => return Err(JsonStreamSinkError),
        }
        Ok(())
    }
    fn on_number(&mut self, raw: &str) -> Result<(), JsonStreamSinkError> {
        if self.depth == 0 {
            return Err(JsonStreamSinkError);
        }
        let destination = match self.target() {
            Key::Sequence => &mut self.sequence,
            Key::Input => &mut self.candidate.input,
            Key::Output => &mut self.candidate.output,
            Key::Cached => &mut self.candidate.cached,
            Key::Read => &mut self.candidate.read,
            Key::Write => &mut self.candidate.write,
            _ => return Ok(()),
        };
        *destination = raw
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_digit)
            .then(|| raw.parse::<u64>().ok())
            .flatten();
        Ok(())
    }
    fn string_capture_limit(&mut self) -> Option<usize> {
        self.depth
            .checked_sub(1)
            .and_then(|index| self.frames.get(index))
            .filter(|frame| matches!(frame.pending, Key::Status | Key::Type))
            .map(|_| 64)
    }
    fn on_captured_string(&mut self, value: &str) -> Result<(), JsonStreamSinkError> {
        match self.target() {
            Key::Status => self.status = ResponseOutcome::from_status(value),
            Key::Type => {
                self.event_kind = match value {
                    "response.completed" => ResponsesEventKind::Completed,
                    "response.incomplete" => ResponsesEventKind::Incomplete,
                    "response.failed" => ResponsesEventKind::Failed,
                    "error" => ResponsesEventKind::Error,
                    _ => ResponsesEventKind::Other,
                }
            }
            _ => return Err(JsonStreamSinkError),
        }
        Ok(())
    }
    fn on_discarded_string(&mut self, _summary: JsonDiscardedStringSummary) -> Result<(), JsonStreamSinkError> {
        if self.depth == 0 {
            return Err(JsonStreamSinkError);
        }
        self.target();
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/protocols/openai/json_usage_test.rs"]
mod tests;
