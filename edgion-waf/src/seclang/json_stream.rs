//! Transport-neutral, bounded JSON tokenization. Tokens are provisional until
//! `Complete` succeeds: later syntax errors and duplicate keys can invalidate
//! any application decision based on an earlier token. This module neither
//! publishes WAF variables nor selects an execution lane.
#[cfg(test)]
use crate::seclang::measurements::{self, Event};

use std::fmt;
use std::mem::size_of;

use super::json_scratch::{growth_target, heap_capacity, GrowthError};
use smallvec::SmallVec;
use thiserror::Error;

use super::JsonNumber;

/// Independent lexical budgets, not aliases for complete-body publication limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonStreamLimits {
    pub max_source_bytes: usize,
    /// Maximum source spelling of one token, including string quotes/escapes.
    /// Container punctuation and comma/colon separators each count as one byte.
    pub max_raw_token_bytes: usize,
    /// Sum of decoded key/string bytes and raw number/literal bytes, including duplicates.
    pub max_decoded_bytes: usize,
    /// Key/scalar/container-open/container-close callbacks; separators are not events.
    pub max_events: usize,
    /// Root value is depth zero; keys do not introduce another value depth.
    pub max_depth: usize,
    /// Sum of live token/stack heap capacities, excluding the fixed struct,
    /// allocator overhead, and transient storage during allocator reallocation.
    pub max_retained_bytes: usize,
    /// Source visits, semantic-byte handling, validation scans and retained copies.
    /// Physical copy/growth work depends on transport chunk boundaries.
    pub max_work: usize,
}

impl Default for JsonStreamLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 16 * 1024 * 1024,
            max_raw_token_bytes: 1024 * 1024,
            max_decoded_bytes: 32 * 1024 * 1024,
            max_events: 131_072,
            max_depth: 64,
            max_retained_bytes: 2 * 1024 * 1024,
            max_work: 128 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy)]
pub enum JsonStreamInput<'a> {
    Data(&'a [u8]),
    Complete,
    Abandoned,
}

impl fmt::Debug for JsonStreamInput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data(bytes) => f.debug_tuple("Data").field(&bytes.len()).finish(),
            Self::Complete => f.write_str("Complete"),
            Self::Abandoned => f.write_str("Abandoned"),
        }
    }
}

/// Payloads are borrowed only for the synchronous callback; consumers must not
/// treat them as final document publications. Number spelling is preserved;
/// its value uses checked integers or correctly rounded finite binary64.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JsonStreamToken<'a> {
    ObjectStart,
    ObjectEnd,
    ArrayStart,
    ArrayEnd,
    Key(&'a str),
    String(&'a str),
    Number { raw: &'a str, value: JsonNumber },
    Boolean(bool),
    Null,
}

impl fmt::Debug for JsonStreamToken<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ObjectStart => f.write_str("ObjectStart"),
            Self::ObjectEnd => f.write_str("ObjectEnd"),
            Self::ArrayStart => f.write_str("ArrayStart"),
            Self::ArrayEnd => f.write_str("ArrayEnd"),
            Self::Key(value) => f.debug_tuple("Key").field(&value.len()).finish(),
            Self::String(value) => f.debug_tuple("String").field(&value.len()).finish(),
            Self::Number { raw, .. } => f.debug_tuple("Number").field(&raw.len()).finish(),
            Self::Boolean(_) => f.write_str("Boolean"),
            Self::Null => f.write_str("Null"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonStreamLimit {
    SourceBytes,
    RawTokenBytes,
    DecodedBytes,
    Events,
    Depth,
    RetainedBytes,
    Work,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum JsonStreamError {
    #[error("JSON stream syntax is invalid")]
    Syntax,
    #[error("JSON stream contains invalid UTF-8")]
    Utf8,
    #[error("JSON stream contains an invalid string escape")]
    Escape,
    #[error("JSON stream number is invalid or outside the supported finite range")]
    Number,
    #[error("JSON stream ended before a complete document")]
    Incomplete,
    #[error("JSON stream was abandoned")]
    Abandoned,
    #[error("JSON stream is already complete")]
    AlreadyComplete,
    #[error("JSON stream exceeds {kind:?} limit {limit}")]
    Limit { kind: JsonStreamLimit, limit: usize },
    #[error("cannot allocate bounded JSON stream state")]
    Allocation,
    #[error("JSON stream accounting arithmetic overflowed")]
    Arithmetic,
    #[error("JSON stream token consumer rejected an event")]
    SinkRejected,
}

/// A callback failure carries no caller-controlled diagnostic payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonStreamSinkError;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JsonStreamUsage {
    pub source_bytes: usize,
    pub decoded_bytes: usize,
    pub events: usize,
    pub work: usize,
    pub retained_bytes: usize,
    pub peak_retained_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
enum Terminal {
    Active,
    Complete,
    Failed(JsonStreamError),
}

#[derive(Clone, Copy)]
enum Frame {
    ObjectFirst,
    ObjectKey,
    ObjectColon,
    ObjectValue,
    ObjectComma,
    ArrayFirst,
    ArrayValue,
    ArrayComma,
}

#[derive(Clone, Copy)]
enum NumberState {
    Minus,
    Zero,
    Integer,
    Dot,
    Fraction,
    Exponent,
    ExponentSign,
    ExponentDigits,
}

impl NumberState {
    fn accepting(self) -> bool {
        matches!(self, Self::Zero | Self::Integer | Self::Fraction | Self::ExponentDigits)
    }

    fn advance(self, byte: u8) -> Option<Self> {
        match (self, byte) {
            (Self::Minus, b'0') => Some(Self::Zero),
            (Self::Minus, b'1'..=b'9') => Some(Self::Integer),
            (Self::Integer, b'0'..=b'9') => Some(Self::Integer),
            (Self::Zero | Self::Integer, b'.') => Some(Self::Dot),
            (Self::Dot | Self::Fraction, b'0'..=b'9') => Some(Self::Fraction),
            (Self::Zero | Self::Integer | Self::Fraction, b'e' | b'E') => Some(Self::Exponent),
            (Self::Exponent, b'+' | b'-') => Some(Self::ExponentSign),
            (Self::Exponent | Self::ExponentSign | Self::ExponentDigits, b'0'..=b'9') => Some(Self::ExponentDigits),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Literal {
    True,
    False,
    Null,
}

impl Literal {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::True => b"true",
            Self::False => b"false",
            Self::Null => b"null",
        }
    }

    fn token(self) -> JsonStreamToken<'static> {
        match self {
            Self::True => JsonStreamToken::Boolean(true),
            Self::False => JsonStreamToken::Boolean(false),
            Self::Null => JsonStreamToken::Null,
        }
    }
}

#[derive(Clone, Copy)]
enum EscapeState {
    Plain,
    Slash,
    Unicode { value: u16, digits: u8 },
    LowSlash { high: u16 },
    LowU { high: u16 },
    LowDigits { high: u16, value: u16, digits: u8 },
}

#[derive(Clone, Copy)]
enum Lexical {
    Idle,
    String { key: bool, escape: EscapeState },
    Number(NumberState),
    Literal { literal: Literal, next: usize },
}

/// Limits for validating and discarding string values while capturing keys/numbers.
/// The lexical budgets retain their default meaning for the capture-all API.
#[derive(Debug, Clone, Copy)]
pub struct JsonStreamSelectiveLimits {
    pub lexical: JsonStreamLimits,
    pub max_key_bytes: usize,
    pub max_number_bytes: usize,
}

/// Metadata from a validated string value. Original text is never retained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonDiscardedStringSummary {
    pub decoded_bytes: usize,
    pub decoded_chars: usize,
}

/// Synchronous consumer of selective JSON events. `on_token` never receives
/// String/Number; those values use their explicit callbacks instead.
pub trait JsonStreamSelectiveSink {
    fn on_token(&mut self, token: JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>;
    fn on_number(&mut self, raw: &str) -> Result<(), JsonStreamSinkError>;
    fn on_discarded_string(&mut self, summary: JsonDiscardedStringSummary) -> Result<(), JsonStreamSinkError>;
    /// Called before a value string starts. None preserves allocation-free discard.
    /// Captured values share the tokenizer's retained/source/work budgets.
    fn string_capture_limit(&mut self) -> Option<usize> {
        None
    }
    fn on_captured_string(&mut self, _value: &str) -> Result<(), JsonStreamSinkError> {
        Err(JsonStreamSinkError)
    }
}

/// Opt-in selective capture over the same grammar as `JsonStream`.
#[derive(Debug)]
pub struct JsonStreamSelective {
    inner: JsonStream,
}

impl JsonStreamSelective {
    pub fn new(limits: JsonStreamSelectiveLimits) -> Self {
        let mut inner = JsonStream::new(limits.lexical);
        inner.selective = Some(limits);
        Self { inner }
    }

    pub fn feed(
        &mut self,
        input: JsonStreamInput<'_>,
        sink: &mut impl JsonStreamSelectiveSink,
    ) -> Result<(), JsonStreamError> {
        self.inner.feed_internal(input, &mut SelectiveSink(sink))
    }

    pub const fn usage(&self) -> JsonStreamUsage {
        self.inner.usage()
    }
}

trait InternalSink {
    fn token(&mut self, token: JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>;
    fn number(&mut self, raw: &str) -> Result<(), JsonStreamSinkError>;
    fn discarded(&mut self, summary: JsonDiscardedStringSummary) -> Result<(), JsonStreamSinkError>;
    fn string_capture_limit(&mut self) -> Option<usize> {
        None
    }
    fn captured_string(&mut self, _value: &str) -> Result<(), JsonStreamSinkError> {
        Err(JsonStreamSinkError)
    }
}

struct CaptureAll<F>(F);
impl<F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>> InternalSink for CaptureAll<F> {
    fn token(&mut self, token: JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError> {
        (self.0)(token)
    }
    fn number(&mut self, _raw: &str) -> Result<(), JsonStreamSinkError> {
        Err(JsonStreamSinkError)
    }
    fn discarded(&mut self, _summary: JsonDiscardedStringSummary) -> Result<(), JsonStreamSinkError> {
        Err(JsonStreamSinkError)
    }
}
struct SelectiveSink<'a, S>(&'a mut S);
impl<S: JsonStreamSelectiveSink> InternalSink for SelectiveSink<'_, S> {
    fn token(&mut self, token: JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError> {
        self.0.on_token(token)
    }
    fn number(&mut self, raw: &str) -> Result<(), JsonStreamSinkError> {
        self.0.on_number(raw)
    }
    fn discarded(&mut self, summary: JsonDiscardedStringSummary) -> Result<(), JsonStreamSinkError> {
        self.0.on_discarded_string(summary)
    }
    fn string_capture_limit(&mut self) -> Option<usize> {
        self.0.string_capture_limit()
    }
    fn captured_string(&mut self, value: &str) -> Result<(), JsonStreamSinkError> {
        self.0.on_captured_string(value)
    }
}

/// One bounded token buffer and a grammar stack. No document or token list is retained.
pub struct JsonStream {
    limits: JsonStreamLimits,
    selective: Option<JsonStreamSelectiveLimits>,
    discarded: JsonDiscardedStringSummary,
    selected_string_limit: Option<usize>,
    usage: JsonStreamUsage,
    terminal: Terminal,
    lexical: Lexical,
    stack: Vec<Frame>,
    token: SmallVec<[u8; 1024]>,
    raw_token_bytes: usize,
    root_started: bool,
    utf8_remaining: u8,
    utf8_value: u32,
    utf8_minimum: u32,
    #[cfg(test)]
    sequential_ascii: bool,
}

impl fmt::Debug for JsonStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonStream")
            .field("limits", &self.limits)
            .field("usage", &self.usage)
            .field("terminal", &self.terminal)
            .field("depth", &self.stack.len())
            .finish()
    }
}

impl JsonStream {
    pub fn new(limits: JsonStreamLimits) -> Self {
        #[cfg(test)]
        measurements::record(Event::GrammarInstances, 1);
        Self {
            limits,
            selective: None,
            discarded: JsonDiscardedStringSummary::default(),
            selected_string_limit: None,
            usage: JsonStreamUsage::default(),
            terminal: Terminal::Active,
            lexical: Lexical::Idle,
            stack: Vec::new(),
            token: SmallVec::new(),
            raw_token_bytes: 0,
            root_started: false,
            utf8_remaining: 0,
            utf8_value: 0,
            utf8_minimum: 0,
            #[cfg(test)]
            sequential_ascii: false,
        }
    }

    pub const fn usage(&self) -> JsonStreamUsage {
        self.usage
    }

    /// An error is sticky. Error, abandonment and successful completion release
    /// allocated buffers; subsequent calls never invoke the token consumer.
    pub fn feed<F>(&mut self, input: JsonStreamInput<'_>, sink: F) -> Result<(), JsonStreamError>
    where
        F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>,
    {
        self.feed_internal(input, &mut CaptureAll(sink))
    }

    fn feed_internal(
        &mut self,
        input: JsonStreamInput<'_>,
        sink: &mut impl InternalSink,
    ) -> Result<(), JsonStreamError> {
        match self.terminal {
            Terminal::Failed(error) => return Err(error),
            Terminal::Complete => return Err(JsonStreamError::AlreadyComplete),
            Terminal::Active => {}
        }
        let result = self.feed_active(input, sink);
        if let Err(error) = result {
            self.terminal = Terminal::Failed(error);
            self.release();
        }
        result
    }

    fn feed_active<F>(&mut self, input: JsonStreamInput<'_>, sink: &mut F) -> Result<(), JsonStreamError>
    where
        F: InternalSink,
    {
        match input {
            JsonStreamInput::Data(bytes) => {
                let mut offset = 0;
                while offset < bytes.len() {
                    let byte = bytes[offset];
                    if let Lexical::String {
                        key,
                        escape: EscapeState::Plain,
                    } = self.lexical
                    {
                        if self.utf8_remaining == 0 && self.selective.is_none() {
                            if self.token.is_empty() {
                                offset += self.borrow_string_prefix(&bytes[offset..], key, sink)?;
                                continue;
                            }
                            if (0x20..=0x7f).contains(&byte) && !matches!(byte, b'"' | b'\\') {
                                let run = self.buffer_string_prefix(&bytes[offset..]);
                                if run != 0 {
                                    offset += run;
                                    continue;
                                }
                            }
                        }
                    }
                    charge(
                        &mut self.usage.source_bytes,
                        1,
                        self.limits.max_source_bytes,
                        JsonStreamLimit::SourceBytes,
                    )?;
                    #[cfg(test)]
                    measurements::record(Event::SourceBytes, 1);
                    self.work(1)?;
                    self.byte(byte, sink)?;
                    offset += 1;
                }
                Ok(())
            }
            JsonStreamInput::Complete => {
                self.work(1)?;
                match self.lexical {
                    Lexical::Number(state) if state.accepting() => self.finish_number(sink)?,
                    Lexical::Idle => {}
                    _ => return Err(JsonStreamError::Incomplete),
                }
                if !self.root_started || !self.stack.is_empty() {
                    return Err(JsonStreamError::Incomplete);
                }
                self.terminal = Terminal::Complete;
                #[cfg(test)]
                measurements::record(Event::Completions, 1);
                self.release();
                Ok(())
            }
            JsonStreamInput::Abandoned => Err(JsonStreamError::Abandoned),
        }
    }

    // Only fill already allocated token space. Growth and special bytes retain
    // the single-byte path's admission order and capacity trajectory.
    fn buffer_string_prefix(&mut self, bytes: &[u8]) -> usize {
        #[cfg(test)]
        if self.sequential_ascii {
            return 0;
        }
        let admitted = bytes
            .len()
            .min(self.limits.max_source_bytes.saturating_sub(self.usage.source_bytes))
            .min(self.limits.max_raw_token_bytes.saturating_sub(self.raw_token_bytes))
            .min(self.limits.max_decoded_bytes.saturating_sub(self.usage.decoded_bytes))
            .min(self.limits.max_work.saturating_sub(self.usage.work) / 2)
            .min(self.token.capacity() - self.token.len());
        let run = bytes[..admitted]
            .iter()
            .take_while(|&&byte| (0x20..=0x7f).contains(&byte) && !matches!(byte, b'"' | b'\\'))
            .count();
        self.usage.source_bytes += run;
        self.raw_token_bytes += run;
        self.usage.decoded_bytes += run;
        self.usage.work += run * 2;
        self.token.extend_from_slice(&bytes[..run]);
        #[cfg(test)]
        measurements::record(Event::SourceBytes, run);
        #[cfg(test)]
        measurements::record(Event::TokenCopyBytes, run);
        run
    }

    // Batch admitted ordinary ASCII; preserve sequential admission at boundaries.
    // No lookahead reads past the admitted prefix. A complete unescaped token
    // borrows this chunk; escape or chunk end retains only its consumed prefix.
    fn borrow_string_prefix<F>(&mut self, bytes: &[u8], key: bool, sink: &mut F) -> Result<usize, JsonStreamError>
    where
        F: InternalSink,
    {
        #[cfg(test)]
        if self.sequential_ascii {
            return self.borrow_string_prefix_sequential(bytes, key, &mut |token| sink.token(token));
        }
        let mut offset = 0;
        while offset < bytes.len() {
            if self.utf8_remaining == 0 {
                // Only inspect a prefix whose ordinary bytes can all be admitted.
                // Boundary and special bytes retain sequential error precedence.
                let admitted = (bytes.len() - offset)
                    .min(self.limits.max_source_bytes.saturating_sub(self.usage.source_bytes))
                    .min(self.limits.max_raw_token_bytes.saturating_sub(self.raw_token_bytes))
                    .min(self.limits.max_decoded_bytes.saturating_sub(self.usage.decoded_bytes))
                    .min(self.limits.max_work.saturating_sub(self.usage.work) / 2);
                let run = bytes[offset..offset + admitted]
                    .iter()
                    .take_while(|&&byte| (0x20..=0x7f).contains(&byte) && !matches!(byte, b'"' | b'\\'))
                    .count();
                self.usage.source_bytes += run;
                self.raw_token_bytes += run;
                self.usage.decoded_bytes += run;
                self.usage.work += run * 2;
                #[cfg(test)]
                measurements::record(Event::SourceBytes, run);
                offset += run;
                if offset == bytes.len() {
                    break;
                }
            }
            let byte = bytes[offset];
            charge(
                &mut self.usage.source_bytes,
                1,
                self.limits.max_source_bytes,
                JsonStreamLimit::SourceBytes,
            )?;
            #[cfg(test)]
            measurements::record(Event::SourceBytes, 1);
            self.work(1)?;
            self.raw_byte()?;
            if self.utf8_remaining != 0 {
                self.utf8(byte)?;
                self.decoded(1)?;
                offset += 1;
                continue;
            }
            match byte {
                b'"' => {
                    self.work(offset)?;
                    self.event()?;
                    let value = std::str::from_utf8(&bytes[..offset]).map_err(|_| JsonStreamError::Utf8)?;
                    sink.token(if key {
                        JsonStreamToken::Key(value)
                    } else {
                        JsonStreamToken::String(value)
                    })
                    .map_err(|_| JsonStreamError::SinkRejected)?;
                    self.lexical = Lexical::Idle;
                    if key {
                        *self.stack.last_mut().ok_or(JsonStreamError::Syntax)? = Frame::ObjectColon;
                    }
                    return Ok(offset + 1);
                }
                b'\\' => {
                    self.work(offset)?;
                    self.retain_token_bytes(&bytes[..offset])?;
                    self.lexical = Lexical::String {
                        key,
                        escape: EscapeState::Slash,
                    };
                    return Ok(offset + 1);
                }
                0..=0x1f => return Err(JsonStreamError::Syntax),
                _ => {
                    self.utf8(byte)?;
                    self.decoded(1)?;
                }
            }
            offset += 1;
        }
        self.work(bytes.len())?;
        self.retain_token_bytes(bytes)?;
        Ok(bytes.len())
    }

    fn byte<F>(&mut self, byte: u8, sink: &mut F) -> Result<(), JsonStreamError>
    where
        F: InternalSink,
    {
        match self.lexical {
            Lexical::Idle => self.idle(byte, sink),
            Lexical::String { key, escape } => self.string(byte, key, escape, sink),
            Lexical::Number(state) => {
                if let Some(next) = state.advance(byte) {
                    self.raw_byte()?;
                    self.append(&[byte])?;
                    self.lexical = Lexical::Number(next);
                    Ok(())
                } else if state.accepting() && (is_whitespace(byte) || matches!(byte, b',' | b']' | b'}')) {
                    self.finish_number(sink)?;
                    self.work(1)?;
                    self.idle(byte, sink)
                } else {
                    Err(JsonStreamError::Number)
                }
            }
            Lexical::Literal { literal, next } => {
                if literal.bytes().get(next) != Some(&byte) {
                    return Err(JsonStreamError::Syntax);
                }
                self.raw_byte()?;
                self.decoded(1)?;
                if next + 1 == literal.bytes().len() {
                    self.lexical = Lexical::Idle;
                    self.emit(literal.token(), sink)
                } else {
                    self.lexical = Lexical::Literal {
                        literal,
                        next: next + 1,
                    };
                    Ok(())
                }
            }
        }
    }

    fn idle<F>(&mut self, byte: u8, sink: &mut F) -> Result<(), JsonStreamError>
    where
        F: InternalSink,
    {
        if is_whitespace(byte) {
            return Ok(());
        }
        self.raw_token_bytes = 0;
        self.raw_byte()?;
        match byte {
            b'{' | b'[' => {
                self.start_value()?;
                self.push_frame(if byte == b'{' {
                    Frame::ObjectFirst
                } else {
                    Frame::ArrayFirst
                })?;
                self.emit(
                    if byte == b'{' {
                        JsonStreamToken::ObjectStart
                    } else {
                        JsonStreamToken::ArrayStart
                    },
                    sink,
                )
            }
            b'}' | b']' => {
                if !matches!(
                    (byte, self.stack.last()),
                    (b'}', Some(Frame::ObjectFirst | Frame::ObjectComma))
                        | (b']', Some(Frame::ArrayFirst | Frame::ArrayComma))
                ) {
                    return Err(JsonStreamError::Syntax);
                }
                self.stack.pop();
                self.emit(
                    if byte == b'}' {
                        JsonStreamToken::ObjectEnd
                    } else {
                        JsonStreamToken::ArrayEnd
                    },
                    sink,
                )
            }
            b':' => match self.stack.last_mut() {
                Some(frame @ Frame::ObjectColon) => {
                    *frame = Frame::ObjectValue;
                    Ok(())
                }
                _ => Err(JsonStreamError::Syntax),
            },
            b',' => match self.stack.last_mut() {
                Some(frame @ Frame::ObjectComma) => {
                    *frame = Frame::ObjectKey;
                    Ok(())
                }
                Some(frame @ Frame::ArrayComma) => {
                    *frame = Frame::ArrayValue;
                    Ok(())
                }
                _ => Err(JsonStreamError::Syntax),
            },
            b'"' => {
                let key = matches!(self.stack.last(), Some(Frame::ObjectFirst | Frame::ObjectKey));
                if !key {
                    self.start_value()?;
                }
                self.token.clear();
                self.discarded = JsonDiscardedStringSummary::default();
                self.selected_string_limit = if !key && self.selective.is_some() {
                    sink.string_capture_limit()
                } else {
                    None
                };
                self.utf8_remaining = 0;
                self.lexical = Lexical::String {
                    key,
                    escape: EscapeState::Plain,
                };
                Ok(())
            }
            b'-' | b'0'..=b'9' => {
                self.start_value()?;
                self.token.clear();
                if self.selective.is_some_and(|limits| limits.max_number_bytes == 0) {
                    return Err(limit_error(JsonStreamLimit::RawTokenBytes, 0));
                }
                self.append(&[byte])?;
                self.lexical = Lexical::Number(match byte {
                    b'-' => NumberState::Minus,
                    b'0' => NumberState::Zero,
                    _ => NumberState::Integer,
                });
                Ok(())
            }
            b't' | b'f' | b'n' => {
                self.start_value()?;
                self.decoded(1)?;
                self.lexical = Lexical::Literal {
                    literal: match byte {
                        b't' => Literal::True,
                        b'f' => Literal::False,
                        _ => Literal::Null,
                    },
                    next: 1,
                };
                Ok(())
            }
            _ => Err(JsonStreamError::Syntax),
        }
    }

    fn start_value(&mut self) -> Result<(), JsonStreamError> {
        if self.stack.len() > self.limits.max_depth {
            return Err(limit_error(JsonStreamLimit::Depth, self.limits.max_depth));
        }
        match self.stack.last_mut() {
            Some(frame @ (Frame::ArrayFirst | Frame::ArrayValue)) => *frame = Frame::ArrayComma,
            Some(frame @ Frame::ObjectValue) => *frame = Frame::ObjectComma,
            None if !self.root_started => self.root_started = true,
            _ => return Err(JsonStreamError::Syntax),
        }
        Ok(())
    }

    fn string<F>(&mut self, byte: u8, key: bool, escape: EscapeState, sink: &mut F) -> Result<(), JsonStreamError>
    where
        F: InternalSink,
    {
        self.raw_byte()?;
        let next = match escape {
            EscapeState::Plain => {
                if self.utf8_remaining != 0 {
                    self.utf8(byte)?;
                    self.append(&[byte])?;
                    EscapeState::Plain
                } else {
                    match byte {
                        b'"' => {
                            self.work(self.token.len())?;
                            self.event()?;
                            if self.selective.is_some() && !key {
                                if self.selected_string_limit.is_some() {
                                    let value = std::str::from_utf8(&self.token).map_err(|_| JsonStreamError::Utf8)?;
                                    sink.captured_string(value).map_err(|_| JsonStreamError::SinkRejected)?;
                                } else {
                                    sink.discarded(self.discarded)
                                        .map_err(|_| JsonStreamError::SinkRejected)?;
                                }
                            } else {
                                let value = std::str::from_utf8(&self.token).map_err(|_| JsonStreamError::Utf8)?;
                                sink.token(if key {
                                    JsonStreamToken::Key(value)
                                } else {
                                    JsonStreamToken::String(value)
                                })
                                .map_err(|_| JsonStreamError::SinkRejected)?;
                            }
                            self.token.clear();
                            self.lexical = Lexical::Idle;
                            if key {
                                let frame = self.stack.last_mut().ok_or(JsonStreamError::Syntax)?;
                                *frame = Frame::ObjectColon;
                            }
                            return Ok(());
                        }
                        b'\\' => EscapeState::Slash,
                        0..=0x1f => return Err(JsonStreamError::Syntax),
                        _ => {
                            self.utf8(byte)?;
                            self.append(&[byte])?;
                            EscapeState::Plain
                        }
                    }
                }
            }
            EscapeState::Slash => {
                let decoded = match byte {
                    b'"' | b'\\' | b'/' => byte,
                    b'b' => 8,
                    b'f' => 12,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'u' => {
                        self.lexical = Lexical::String {
                            key,
                            escape: EscapeState::Unicode { value: 0, digits: 0 },
                        };
                        return Ok(());
                    }
                    _ => return Err(JsonStreamError::Escape),
                };
                self.append(&[decoded])?;
                EscapeState::Plain
            }
            EscapeState::Unicode { value, digits } => {
                let value = (value << 4) | hex(byte)?;
                if digits < 3 {
                    EscapeState::Unicode {
                        value,
                        digits: digits + 1,
                    }
                } else if (0xd800..=0xdbff).contains(&value) {
                    EscapeState::LowSlash { high: value }
                } else if (0xdc00..=0xdfff).contains(&value) {
                    return Err(JsonStreamError::Escape);
                } else {
                    self.scalar(u32::from(value))?;
                    EscapeState::Plain
                }
            }
            EscapeState::LowSlash { high } => {
                if byte != b'\\' {
                    return Err(JsonStreamError::Escape);
                }
                EscapeState::LowU { high }
            }
            EscapeState::LowU { high } => {
                if byte != b'u' {
                    return Err(JsonStreamError::Escape);
                }
                EscapeState::LowDigits {
                    high,
                    value: 0,
                    digits: 0,
                }
            }
            EscapeState::LowDigits { high, value, digits } => {
                let value = (value << 4) | hex(byte)?;
                if digits < 3 {
                    EscapeState::LowDigits {
                        high,
                        value,
                        digits: digits + 1,
                    }
                } else {
                    if !(0xdc00..=0xdfff).contains(&value) {
                        return Err(JsonStreamError::Escape);
                    }
                    self.scalar(0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(value) - 0xdc00)?;
                    EscapeState::Plain
                }
            }
        };
        self.lexical = Lexical::String { key, escape: next };
        Ok(())
    }

    fn utf8(&mut self, byte: u8) -> Result<(), JsonStreamError> {
        if self.utf8_remaining == 0 {
            let (remaining, value, minimum) = match byte {
                0..=0x7f => return Ok(()),
                0xc2..=0xdf => (1, u32::from(byte & 0x1f), 0x80),
                0xe0..=0xef => (2, u32::from(byte & 0x0f), 0x800),
                0xf0..=0xf4 => (3, u32::from(byte & 0x07), 0x10000),
                _ => return Err(JsonStreamError::Utf8),
            };
            self.utf8_remaining = remaining;
            self.utf8_value = value;
            self.utf8_minimum = minimum;
        } else {
            if !(0x80..=0xbf).contains(&byte) {
                return Err(JsonStreamError::Utf8);
            }
            self.utf8_value = (self.utf8_value << 6) | u32::from(byte & 0x3f);
            self.utf8_remaining -= 1;
            if self.utf8_remaining == 0
                && (self.utf8_value < self.utf8_minimum
                    || self.utf8_value > 0x10ffff
                    || (0xd800..=0xdfff).contains(&self.utf8_value))
            {
                return Err(JsonStreamError::Utf8);
            }
        }
        Ok(())
    }

    fn scalar(&mut self, value: u32) -> Result<(), JsonStreamError> {
        let value = char::from_u32(value).ok_or(JsonStreamError::Escape)?;
        let mut bytes = [0; 4];
        self.append(value.encode_utf8(&mut bytes).as_bytes())
    }

    fn finish_number<F>(&mut self, sink: &mut F) -> Result<(), JsonStreamError>
    where
        F: InternalSink,
    {
        // Charge both UTF-8 views, form classification, integer accumulation,
        // and possible float fallback as a conservative linear scan bound.
        self.work(self.token.len().checked_mul(5).ok_or(JsonStreamError::Arithmetic)?)?;
        let raw = std::str::from_utf8(&self.token).map_err(|_| JsonStreamError::Number)?;
        if self.selective.is_some() {
            // Grammar is already validated. Do not coerce exact spelling through f64.
            self.event()?;
            let raw = std::str::from_utf8(&self.token).map_err(|_| JsonStreamError::Number)?;
            sink.number(raw).map_err(|_| JsonStreamError::SinkRejected)?;
        } else {
            let value = JsonNumber::from_validated_lexeme(raw).map_err(|_| JsonStreamError::Number)?;
            self.event()?;
            let raw = std::str::from_utf8(&self.token).map_err(|_| JsonStreamError::Number)?;
            sink.token(JsonStreamToken::Number { raw, value })
                .map_err(|_| JsonStreamError::SinkRejected)?;
        }
        self.token.clear();
        self.lexical = Lexical::Idle;
        Ok(())
    }

    fn emit<F>(&mut self, token: JsonStreamToken<'_>, sink: &mut F) -> Result<(), JsonStreamError>
    where
        F: InternalSink,
    {
        self.event()?;
        sink.token(token).map_err(|_| JsonStreamError::SinkRejected)
    }

    fn event(&mut self) -> Result<(), JsonStreamError> {
        charge(
            &mut self.usage.events,
            1,
            self.limits.max_events,
            JsonStreamLimit::Events,
        )
    }

    fn work(&mut self, amount: usize) -> Result<(), JsonStreamError> {
        charge(
            &mut self.usage.work,
            amount,
            self.limits.max_work,
            JsonStreamLimit::Work,
        )
    }

    fn raw_byte(&mut self) -> Result<(), JsonStreamError> {
        if self.selective.is_some() && matches!(self.lexical, Lexical::String { key: false, .. }) {
            return Ok(());
        }
        let limit = if matches!(self.lexical, Lexical::Number(_)) {
            self.selective
                .map_or(self.limits.max_raw_token_bytes, |limits| limits.max_number_bytes)
        } else {
            self.limits.max_raw_token_bytes
        };
        charge(&mut self.raw_token_bytes, 1, limit, JsonStreamLimit::RawTokenBytes)
    }

    fn decoded(&mut self, amount: usize) -> Result<(), JsonStreamError> {
        charge(
            &mut self.usage.decoded_bytes,
            amount,
            self.limits.max_decoded_bytes,
            JsonStreamLimit::DecodedBytes,
        )?;
        self.work(amount)
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), JsonStreamError> {
        self.decoded(bytes.len())?;
        if let Some(limits) = self.selective {
            if matches!(self.lexical, Lexical::String { key: false, .. }) {
                self.discarded.decoded_bytes = self
                    .discarded
                    .decoded_bytes
                    .checked_add(bytes.len())
                    .ok_or(JsonStreamError::Arithmetic)?;
                if bytes.len() > 1 || self.utf8_remaining == 0 {
                    self.discarded.decoded_chars = self
                        .discarded
                        .decoded_chars
                        .checked_add(1)
                        .ok_or(JsonStreamError::Arithmetic)?;
                }
                if let Some(limit) = self.selected_string_limit {
                    if self.discarded.decoded_bytes > limit {
                        return Err(limit_error(JsonStreamLimit::DecodedBytes, limit));
                    }
                    return self.retain_token_bytes(bytes);
                }
                return Ok(());
            }
            if matches!(self.lexical, Lexical::String { key: true, .. })
                && self
                    .token
                    .len()
                    .checked_add(bytes.len())
                    .is_none_or(|size| size > limits.max_key_bytes)
            {
                return Err(limit_error(JsonStreamLimit::DecodedBytes, limits.max_key_bytes));
            }
        }
        self.retain_token_bytes(bytes)
    }

    fn retain_token_bytes(&mut self, bytes: &[u8]) -> Result<(), JsonStreamError> {
        let remaining = self.limits.max_retained_bytes.saturating_sub(self.usage.retained_bytes);
        let target = growth_target(&self.token, bytes.len(), remaining).map_err(|error| match error {
            GrowthError::Arithmetic => JsonStreamError::Arithmetic,
            GrowthError::Retained => limit_error(JsonStreamLimit::RetainedBytes, self.limits.max_retained_bytes),
        })?;
        if let Some(target) = target {
            self.work(self.token.len())?;
            self.token
                .try_reserve_exact(target - self.token.len())
                .map_err(|_| JsonStreamError::Allocation)?;
            self.account_retained()?;
        }
        if let [byte] = bytes {
            self.token.push(*byte);
        } else {
            self.token.extend_from_slice(bytes);
        }
        #[cfg(test)]
        measurements::record(Event::TokenCopyBytes, bytes.len());
        Ok(())
    }

    fn push_frame(&mut self, frame: Frame) -> Result<(), JsonStreamError> {
        let required = self.stack.len().checked_add(1).ok_or(JsonStreamError::Arithmetic)?;
        if required > self.stack.capacity() {
            let target = self.growth_capacity(self.stack.capacity(), required, size_of::<Frame>())?;
            self.work(
                self.stack
                    .len()
                    .checked_mul(size_of::<Frame>())
                    .ok_or(JsonStreamError::Arithmetic)?,
            )?;
            self.stack
                .try_reserve_exact(target - self.stack.len())
                .map_err(|_| JsonStreamError::Allocation)?;
            self.account_retained()?;
        }
        self.stack.push(frame);
        Ok(())
    }

    fn growth_capacity(&self, capacity: usize, required: usize, element_size: usize) -> Result<usize, JsonStreamError> {
        let spare = self.limits.max_retained_bytes.saturating_sub(self.usage.retained_bytes) / element_size;
        let maximum = capacity.checked_add(spare).ok_or(JsonStreamError::Arithmetic)?;
        if required > maximum {
            return Err(limit_error(
                JsonStreamLimit::RetainedBytes,
                self.limits.max_retained_bytes,
            ));
        }
        Ok(capacity.saturating_mul(2).max(required).min(maximum))
    }

    fn account_retained(&mut self) -> Result<(), JsonStreamError> {
        let retained = self
            .stack
            .capacity()
            .checked_mul(size_of::<Frame>())
            .and_then(|bytes| bytes.checked_add(heap_capacity(&self.token)))
            .ok_or(JsonStreamError::Arithmetic)?;
        self.usage.retained_bytes = retained;
        self.usage.peak_retained_bytes = self.usage.peak_retained_bytes.max(retained);
        // Allocators may give reserve_exact more capacity than requested. Never
        // use such an allocation if its actual Vec capacity violates the cap.
        if retained > self.limits.max_retained_bytes {
            return Err(limit_error(
                JsonStreamLimit::RetainedBytes,
                self.limits.max_retained_bytes,
            ));
        }
        Ok(())
    }

    fn release(&mut self) {
        self.token = SmallVec::new();
        self.stack = Vec::new();
        self.lexical = Lexical::Idle;
        self.utf8_remaining = 0;
        self.utf8_value = 0;
        self.utf8_minimum = 0;
        self.usage.retained_bytes = 0;
    }
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

fn hex(byte: u8) -> Result<u16, JsonStreamError> {
    match byte {
        b'0'..=b'9' => Ok(u16::from(byte - b'0')),
        b'a'..=b'f' => Ok(u16::from(byte - b'a' + 10)),
        b'A'..=b'F' => Ok(u16::from(byte - b'A' + 10)),
        _ => Err(JsonStreamError::Escape),
    }
}

fn limit_error(kind: JsonStreamLimit, limit: usize) -> JsonStreamError {
    JsonStreamError::Limit { kind, limit }
}

fn charge(counter: &mut usize, amount: usize, limit: usize, kind: JsonStreamLimit) -> Result<(), JsonStreamError> {
    let next = counter.checked_add(amount).ok_or(JsonStreamError::Arithmetic)?;
    if next > limit {
        return Err(limit_error(kind, limit));
    }
    *counter = next;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_stream_test.rs"]
mod tests;
