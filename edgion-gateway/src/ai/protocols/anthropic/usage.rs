//! Selective native numeric/terminal metadata over the canonical JSON tokenizer.
use crate::ai::ReportedAiUsage;
use edgion_waf::seclang::{
    JsonDiscardedStringSummary, JsonStreamInput, JsonStreamLimits, JsonStreamSelective, JsonStreamSelectiveLimits,
    JsonStreamSelectiveSink, JsonStreamSinkError, JsonStreamToken,
};
const DEPTH: usize = 64;
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    #[default]
    Other,
    Message,
    Start,
    Delta,
    Stop,
    Error,
}
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Components {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub read: Option<u64>,
    pub write: Option<u64>,
    invalid: u8,
}
impl Components {
    pub(crate) fn valid(&self) -> bool {
        self.invalid == 0
    }
    pub(crate) fn merge(&mut self, patch: Self) {
        fn high(a: &mut Option<u64>, b: Option<u64>) {
            if let Some(b) = b {
                *a = Some(a.map_or(b, |a| a.max(b)));
            }
        }
        high(&mut self.input, patch.input);
        high(&mut self.output, patch.output);
        high(&mut self.read, patch.read);
        high(&mut self.write, patch.write);
        self.invalid |= patch.invalid;
    }
    pub(crate) fn normalized(self) -> Option<ReportedAiUsage> {
        if !self.valid() {
            return None;
        }
        let read = self.read.unwrap_or(0);
        let write = self.write.unwrap_or(0);
        let input = self.input?.checked_add(read)?.checked_add(write)?;
        let output = self.output?;
        input.checked_add(output)?;
        Some(ReportedAiUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: read,
            cache_write_input_tokens: write,
        })
    }
}
#[derive(Default)]
pub(crate) struct Observation {
    pub kind: Kind,
    pub components: Components,
    pub stop: bool,
    pub composite: bool,
    pub invalid: bool,
}
pub(crate) struct Observer {
    parser: JsonStreamSelective,
    selector: Selector,
    failed: bool,
}
impl Observer {
    pub(crate) fn new() -> Self {
        Self::for_envelope(Kind::Message)
    }
    pub(crate) fn for_envelope(envelope: Kind) -> Self {
        Self {
            parser: JsonStreamSelective::new(JsonStreamSelectiveLimits {
                lexical: JsonStreamLimits {
                    max_source_bytes: 64 * 1024 * 1024,
                    max_raw_token_bytes: 3074,
                    max_decoded_bytes: 64 * 1024 * 1024,
                    max_events: 131072,
                    max_depth: DEPTH,
                    max_retained_bytes: 8192,
                    max_work: 256 * 1024 * 1024,
                },
                max_key_bytes: 256,
                max_number_bytes: 64,
            }),
            selector: Selector {
                envelope,
                ..Selector::default()
            },
            failed: false,
        }
    }
    pub(crate) fn feed(&mut self, data: &[u8]) {
        if !self.failed {
            self.failed = self
                .parser
                .feed(JsonStreamInput::Data(data), &mut self.selector)
                .is_err();
        }
    }
    pub(crate) fn finish(&mut self, expected_model: &str) -> Observation {
        if !self.failed {
            self.failed = self.parser.feed(JsonStreamInput::Complete, &mut self.selector).is_err();
        }
        let s = &self.selector;
        Observation {
            kind: s.kind,
            components: s.components,
            stop: s.stop,
            composite: s.composite
                || s.iteration_model
                    .as_ref()
                    .is_some_and(|m| s.model.as_deref().unwrap_or(expected_model) != m),
            invalid: self.failed
                || !s.components.valid()
                || s.stop_invalid
                || (s.envelope == Kind::Start && !s.message_typed),
        }
    }
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }
}
#[derive(Clone, Copy, Default, PartialEq)]
enum Role {
    Root,
    Message,
    Delta,
    Usage,
    Iterations,
    Iteration,
    #[default]
    Other,
}
#[derive(Clone, Copy, Default, PartialEq)]
enum Key {
    Type,
    Message,
    Delta,
    Usage,
    Stop,
    Input,
    Output,
    Read,
    Write,
    Iterations,
    Model,
    #[default]
    Other,
}
#[derive(Clone, Copy, Default)]
struct Frame {
    role: Role,
    key: Key,
    iteration_message: bool,
}
struct Selector {
    frames: [Frame; DEPTH + 1],
    depth: usize,
    kind: Kind,
    envelope: Kind,
    message_typed: bool,
    components: Components,
    stop: bool,
    stop_invalid: bool,
    composite: bool,
    model: Option<String>,
    iteration_model: Option<String>,
    iterations: usize,
    current_iteration_model: Option<String>,
    current_iteration_model_invalid: bool,
}
impl Default for Selector {
    fn default() -> Self {
        Self {
            frames: [Frame::default(); DEPTH + 1],
            depth: 0,
            kind: Kind::Other,
            envelope: Kind::Message,
            message_typed: false,
            components: Components::default(),
            stop: false,
            stop_invalid: false,
            composite: false,
            model: None,
            iteration_model: None,
            iterations: 0,
            current_iteration_model: None,
            current_iteration_model_invalid: false,
        }
    }
}
impl Selector {
    fn pending(&self) -> (Role, Key) {
        self.depth
            .checked_sub(1)
            .map(|i| (self.frames[i].role, self.frames[i].key))
            .unwrap_or((Role::Other, Key::Other))
    }
    fn target(&mut self) -> (Role, Key) {
        let (role, key) = self.pending();
        if self.depth > 0 {
            self.frames[self.depth - 1].key = Key::Other;
        }
        let bit = match key {
            Key::Input => 1,
            Key::Output => 2,
            Key::Read => 4,
            Key::Write => 8,
            _ => 0,
        };
        self.components.invalid &= !bit;
        match key {
            Key::Input => self.components.input = None,
            Key::Output => self.components.output = None,
            Key::Read => self.components.read = None,
            Key::Write => self.components.write = None,
            Key::Usage => {
                self.components = Components::default();
                self.composite = false;
                self.iteration_model = None;
                self.iterations = 0;
            }
            Key::Message => {
                self.message_typed = false;
                self.components = Components::default();
                self.stop = false;
                self.stop_invalid = false;
                self.model = None;
                self.composite = false;
                self.iteration_model = None;
            }
            Key::Delta | Key::Stop => {
                self.stop = false;
                self.stop_invalid = false;
            }
            Key::Iterations => {
                self.composite = false;
                self.iteration_model = None;
                self.iterations = 0;
            }
            Key::Type if role == Role::Root => self.kind = Kind::Other,
            Key::Type if role == Role::Message => self.message_typed = false,
            Key::Type if role == Role::Iteration => self.frames[self.depth - 1].iteration_message = false,
            Key::Model if role == Role::Iteration => {
                self.current_iteration_model = None;
                self.current_iteration_model_invalid = false;
            }
            Key::Model => self.model = None,
            _ => {}
        }
        (role, key)
    }
    fn invalid_scalar(&mut self, role: Role, key: Key) {
        let bit = match key {
            Key::Input => 1,
            Key::Output => 2,
            Key::Read => 4,
            Key::Write => 8,
            _ => 0,
        };
        self.components.invalid |= bit;
        if matches!(key, Key::Stop | Key::Delta) {
            self.stop_invalid = true;
        }
        if key == Key::Usage {
            self.components.invalid |= 16;
        }
        if role == Role::Iteration && key == Key::Model {
            self.current_iteration_model_invalid = true;
        }
        if matches!(key, Key::Iterations) || role == Role::Iterations {
            self.composite = true;
        }
    }
}
impl JsonStreamSelectiveSink for Selector {
    fn on_token(&mut self, t: JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError> {
        match t {
            JsonStreamToken::Key(name) => {
                let f = self
                    .frames
                    .get_mut(self.depth.checked_sub(1).ok_or(JsonStreamSinkError)?)
                    .ok_or(JsonStreamSinkError)?;
                f.key = match (f.role, name) {
                    (Role::Root | Role::Message | Role::Iteration, "type") => Key::Type,
                    (Role::Root, "message") if self.envelope == Kind::Start => Key::Message,
                    (Role::Root, "delta") if self.envelope == Kind::Delta => Key::Delta,
                    (Role::Root, "usage") if matches!(self.envelope, Kind::Message | Kind::Delta) => Key::Usage,
                    (Role::Message, "usage") => Key::Usage,
                    (Role::Root, "stop_reason") if self.envelope == Kind::Message => Key::Stop,
                    (Role::Delta, "stop_reason") => Key::Stop,
                    (Role::Root, "model") if self.envelope == Kind::Message => Key::Model,
                    (Role::Message | Role::Iteration, "model") => Key::Model,
                    (Role::Usage, "input_tokens") => Key::Input,
                    (Role::Usage, "output_tokens") => Key::Output,
                    (Role::Usage, "cache_read_input_tokens") => Key::Read,
                    (Role::Usage, "cache_creation_input_tokens") => Key::Write,
                    (Role::Usage, "iterations") => Key::Iterations,
                    _ => Key::Other,
                };
            }
            JsonStreamToken::ObjectStart | JsonStreamToken::ArrayStart => {
                let object = matches!(t, JsonStreamToken::ObjectStart);
                let role = if self.depth == 0 {
                    if !object {
                        return Err(JsonStreamSinkError);
                    }
                    Role::Root
                } else {
                    let (parent, key) = self.target();
                    if parent == Role::Iterations {
                        self.iterations += 1;
                        if self.iterations > 4096 {
                            return Err(JsonStreamSinkError);
                        }
                        if object {
                            self.current_iteration_model = None;
                            self.current_iteration_model_invalid = false;
                            Role::Iteration
                        } else {
                            self.composite = true;
                            Role::Other
                        }
                    } else {
                        match (key, object) {
                            (Key::Message, true) => Role::Message,
                            (Key::Delta, true) => Role::Delta,
                            (Key::Usage, true) => Role::Usage,
                            (Key::Iterations, false) => Role::Iterations,
                            _ => {
                                self.invalid_scalar(parent, key);
                                Role::Other
                            }
                        }
                    }
                };
                *self.frames.get_mut(self.depth).ok_or(JsonStreamSinkError)? = Frame {
                    role,
                    ..Frame::default()
                };
                self.depth += 1;
            }
            JsonStreamToken::ObjectEnd | JsonStreamToken::ArrayEnd => {
                self.depth = self.depth.checked_sub(1).ok_or(JsonStreamSinkError)?;
                if self.frames[self.depth].role == Role::Iteration {
                    self.composite |=
                        !self.frames[self.depth].iteration_message || self.current_iteration_model_invalid;
                    if let Some(model) = self.current_iteration_model.take() {
                        self.composite |= self.iteration_model.as_ref().is_some_and(|m| m != &model);
                        self.iteration_model = Some(model);
                    }
                }
            }
            JsonStreamToken::Null => {
                let (role, key) = self.target();
                // Only delta input/cache fields are nullable; required counts and
                // selected object parents must not masquerade as omitted patches.
                if matches!(key, Key::Output | Key::Usage | Key::Delta | Key::Message)
                    || (key == Key::Input && self.envelope != Kind::Delta)
                {
                    self.invalid_scalar(role, key);
                }
                if role == Role::Iteration && key == Key::Model {
                    self.current_iteration_model_invalid = true;
                }
                if role == Role::Iterations {
                    self.composite = true;
                }
            }
            JsonStreamToken::Boolean(_) => {
                let (r, k) = self.target();
                self.invalid_scalar(r, k);
            }
            _ => return Err(JsonStreamSinkError),
        }
        Ok(())
    }
    fn on_number(&mut self, raw: &str) -> Result<(), JsonStreamSinkError> {
        let (role, key) = self.target();
        let value = raw
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| raw.parse::<u64>().ok())
            .flatten();
        if value.is_none() {
            self.invalid_scalar(role, key);
        }
        match key {
            Key::Input => self.components.input = value,
            Key::Output => self.components.output = value,
            Key::Read => self.components.read = value,
            Key::Write => self.components.write = value,
            _ => self.invalid_scalar(role, key),
        }
        Ok(())
    }
    fn string_capture_limit(&mut self) -> Option<usize> {
        match self.pending().1 {
            Key::Type | Key::Stop => Some(64),
            Key::Model => Some(512),
            _ => None,
        }
    }
    fn on_captured_string(&mut self, s: &str) -> Result<(), JsonStreamSinkError> {
        let (role, key) = self.target();
        match key {
            Key::Type if role == Role::Root => {
                self.kind = match s {
                    "message" => Kind::Message,
                    "message_start" => Kind::Start,
                    "message_delta" => Kind::Delta,
                    "message_stop" => Kind::Stop,
                    "error" => Kind::Error,
                    _ => Kind::Other,
                };
            }
            Key::Type if role == Role::Message => self.message_typed = s == "message",
            Key::Type if role == Role::Iteration => self.frames[self.depth - 1].iteration_message = s == "message",
            Key::Stop => {
                self.stop = matches!(
                    s,
                    "end_turn"
                        | "max_tokens"
                        | "stop_sequence"
                        | "tool_use"
                        | "pause_turn"
                        | "refusal"
                        | "model_context_window_exceeded"
                );
                if !self.stop {
                    self.stop_invalid = true;
                }
            }
            Key::Model if role == Role::Iteration => {
                self.current_iteration_model = Some(s.to_owned());
            }
            Key::Model => self.model = Some(s.to_owned()),
            _ => {}
        }
        Ok(())
    }
    fn on_discarded_string(&mut self, _: JsonDiscardedStringSummary) -> Result<(), JsonStreamSinkError> {
        let (role, key) = self.target();
        self.invalid_scalar(role, key);
        Ok(())
    }
}
#[cfg(test)]
#[path = "../../../../tests/unit/ai/protocols/anthropic/usage_test.rs"]
mod tests;
