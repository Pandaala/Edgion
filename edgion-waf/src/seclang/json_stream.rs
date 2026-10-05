//! Transport-neutral, bounded JSON tokenization. Tokens are provisional until
//! `Complete` succeeds: later syntax errors and duplicate keys can invalidate
//! any application decision based on an earlier token. This module neither
//! publishes WAF variables nor selects an execution lane.

use std::fmt;
use std::mem::size_of;

use thiserror::Error;

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
    /// Sum of live token/stack Vec capacities, excluding the fixed struct,
    /// allocator overhead, and transient storage during allocator reallocation.
    pub max_retained_bytes: usize,
    /// Source visits, decoded writes, validation scans and capacity-growth copies.
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
/// its finite-number acceptance is checked by the same serde_json parser as Value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JsonStreamToken<'a> {
    ObjectStart,
    ObjectEnd,
    ArrayStart,
    ArrayEnd,
    Key(&'a str),
    String(&'a str),
    Number(&'a str),
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
            Self::Number(value) => f.debug_tuple("Number").field(&value.len()).finish(),
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

/// One bounded token buffer and a grammar stack. No document or token list is retained.
pub struct JsonStream {
    limits: JsonStreamLimits,
    usage: JsonStreamUsage,
    terminal: Terminal,
    lexical: Lexical,
    stack: Vec<Frame>,
    token: Vec<u8>,
    raw_token_bytes: usize,
    root_started: bool,
    reference_recursion_guard: bool,
    utf8_remaining: u8,
    utf8_value: u32,
    utf8_minimum: u32,
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
        Self {
            limits,
            usage: JsonStreamUsage::default(),
            terminal: Terminal::Active,
            lexical: Lexical::Idle,
            stack: Vec::new(),
            token: Vec::new(),
            raw_token_bytes: 0,
            root_started: false,
            reference_recursion_guard: false,
            utf8_remaining: 0,
            utf8_value: 0,
            utf8_minimum: 0,
        }
    }

    pub(super) fn new_for_body(limits: JsonStreamLimits) -> Self {
        Self {
            reference_recursion_guard: true,
            ..Self::new(limits)
        }
    }

    pub const fn usage(&self) -> JsonStreamUsage {
        self.usage
    }

    /// An error is sticky. Error, abandonment and successful completion release
    /// allocated buffers; subsequent calls never invoke the token consumer.
    pub fn feed<F>(&mut self, input: JsonStreamInput<'_>, mut sink: F) -> Result<(), JsonStreamError>
    where
        F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>,
    {
        match self.terminal {
            Terminal::Failed(error) => return Err(error),
            Terminal::Complete => return Err(JsonStreamError::AlreadyComplete),
            Terminal::Active => {}
        }
        let result = self.feed_active(input, &mut sink);
        if let Err(error) = result {
            self.terminal = Terminal::Failed(error);
            self.release();
        }
        result
    }

    fn feed_active<F>(&mut self, input: JsonStreamInput<'_>, sink: &mut F) -> Result<(), JsonStreamError>
    where
        F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>,
    {
        match input {
            JsonStreamInput::Data(bytes) => {
                for &byte in bytes {
                    charge(
                        &mut self.usage.source_bytes,
                        1,
                        self.limits.max_source_bytes,
                        JsonStreamLimit::SourceBytes,
                    )?;
                    self.work(1)?;
                    self.byte(byte, sink)?;
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
                self.release();
                Ok(())
            }
            JsonStreamInput::Abandoned => Err(JsonStreamError::Abandoned),
        }
    }

    fn byte<F>(&mut self, byte: u8, sink: &mut F) -> Result<(), JsonStreamError>
    where
        F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>,
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
        F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>,
    {
        if is_whitespace(byte) {
            return Ok(());
        }
        self.raw_token_bytes = 0;
        self.raw_byte()?;
        match byte {
            b'{' | b'[' => {
                // The default serde_json body parser allows 127 nested containers
                // and rejects the 128th, including subsequently replaced subtrees.
                // This source syntax rule is separate from configured depth budgets.
                if self.reference_recursion_guard && self.stack.len() >= 127 {
                    return Err(JsonStreamError::Syntax);
                }
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
        F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>,
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
                            let value = std::str::from_utf8(&self.token).map_err(|_| JsonStreamError::Utf8)?;
                            sink(if key {
                                JsonStreamToken::Key(value)
                            } else {
                                JsonStreamToken::String(value)
                            })
                            .map_err(|_| JsonStreamError::SinkRejected)?;
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
        F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>,
    {
        // Reuse the configured serde_json number semantics without constructing a DOM.
        // Charge both the number parser and subsequent UTF-8 validation scan.
        self.work(self.token.len().checked_mul(2).ok_or(JsonStreamError::Arithmetic)?)?;
        serde_json::from_slice::<serde_json::Number>(&self.token).map_err(|_| JsonStreamError::Number)?;
        self.event()?;
        let value = std::str::from_utf8(&self.token).map_err(|_| JsonStreamError::Number)?;
        sink(JsonStreamToken::Number(value)).map_err(|_| JsonStreamError::SinkRejected)?;
        self.token.clear();
        self.lexical = Lexical::Idle;
        Ok(())
    }

    fn emit<F>(&mut self, token: JsonStreamToken<'_>, sink: &mut F) -> Result<(), JsonStreamError>
    where
        F: FnMut(JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError>,
    {
        self.event()?;
        sink(token).map_err(|_| JsonStreamError::SinkRejected)
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
        charge(
            &mut self.raw_token_bytes,
            1,
            self.limits.max_raw_token_bytes,
            JsonStreamLimit::RawTokenBytes,
        )
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
        let required = self
            .token
            .len()
            .checked_add(bytes.len())
            .ok_or(JsonStreamError::Arithmetic)?;
        if required > self.token.capacity() {
            let target = self.growth_capacity(self.token.capacity(), required, 1)?;
            self.work(self.token.len())?;
            self.token
                .try_reserve_exact(target - self.token.len())
                .map_err(|_| JsonStreamError::Allocation)?;
            self.account_retained()?;
        }
        self.token.extend_from_slice(bytes);
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
            .and_then(|bytes| bytes.checked_add(self.token.capacity()))
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
        self.token = Vec::new();
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
