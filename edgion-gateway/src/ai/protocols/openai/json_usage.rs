//! Incremental, bounded Chat JSON usage selection. Values stay provisional until EOS.
use crate::ai::ReportedAiUsage;
use edgion_waf::seclang::{
    JsonDiscardedStringSummary, JsonStreamError, JsonStreamInput, JsonStreamLimits, JsonStreamSelective,
    JsonStreamSelectiveLimits, JsonStreamSelectiveSink, JsonStreamSinkError, JsonStreamToken,
};

pub(crate) const MAX_USAGE_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

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
    Usage,
    Details,
    #[default]
    Other,
}
#[derive(Clone, Copy, Default)]
enum Key {
    Usage,
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
}
impl Default for Selector {
    fn default() -> Self {
        Self {
            frames: [Frame::default(); MAX_DEPTH + 1],
            depth: 0,
            candidate: Candidate::default(),
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
        Some(ReportedAiUsage {
            input_tokens: self.candidate.input?,
            output_tokens: self.candidate.output?,
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
                    (Role::Root, "usage") => Key::Usage,
                    (Role::Usage, "prompt_tokens") => Key::Input,
                    (Role::Usage, "completion_tokens") => Key::Output,
                    (Role::Usage, "prompt_tokens_details") => Key::Details,
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
        let target = self.target();
        let value = raw
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| raw.parse::<u64>().ok())
            .flatten();
        match target {
            Key::Input => self.candidate.input = value,
            Key::Output => self.candidate.output = value,
            Key::Cached => self.candidate.cached = value,
            Key::Read => self.candidate.read = value,
            Key::Write => self.candidate.write = value,
            _ => {}
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
