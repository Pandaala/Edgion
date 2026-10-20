//! Bounded incremental XML 1.0 inspection subset. Values remain provisional until EOF.
use super::json_scratch::{growth_target, heap_capacity, GrowthError};
use smallvec::{Array, SmallVec};
use std::mem::size_of;
use thiserror::Error;

#[derive(Debug, Clone, Copy)]
pub struct XmlStreamLimits {
    pub max_source_bytes: usize,
    pub max_raw_token_bytes: usize,
    pub max_name_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_events: usize,
    pub max_depth: usize,
    pub max_attributes: usize,
    pub max_namespaces: usize,
    pub max_structure_bytes: usize,
    pub max_retained_bytes: usize,
    pub max_work: usize,
}
impl Default for XmlStreamLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 16 << 20,
            max_raw_token_bytes: 16 << 20,
            max_name_bytes: 16 << 10,
            max_decoded_bytes: 32 << 20,
            max_events: 262_144,
            max_depth: 64,
            max_attributes: 65_536,
            max_namespaces: 65_536,
            max_structure_bytes: 8 << 20,
            max_retained_bytes: 32 << 20,
            max_work: 128 << 20,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlStreamLimit {
    SourceBytes,
    RawTokenBytes,
    NameBytes,
    DecodedBytes,
    Events,
    Depth,
    Attributes,
    Namespaces,
    StructureBytes,
    RetainedBytes,
    Work,
}
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum XmlStreamError {
    #[error("XML stream syntax is invalid")]
    Syntax,
    #[error("XML stream contains invalid UTF-8 or XML characters")]
    Character,
    #[error("XML stream ended before a complete document")]
    Incomplete,
    #[error("XML stream was abandoned")]
    Abandoned,
    #[error("XML stream is already complete")]
    AlreadyComplete,
    #[error("XML stream exceeds {kind:?} limit {limit}")]
    Limit { kind: XmlStreamLimit, limit: usize },
    #[error("cannot allocate bounded XML state")]
    Allocation,
    #[error("XML accounting arithmetic overflowed")]
    Arithmetic,
    #[error("XML value consumer rejected an event")]
    SinkRejected,
}
#[derive(Clone, Copy)]
pub enum XmlStreamInput<'a> {
    Data(&'a [u8]),
    Complete,
    Abandoned,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlStreamSinkError;
pub trait XmlStreamSink {
    fn value(&mut self, name: Option<&[u8]>, value: &[u8]) -> Result<(), XmlStreamSinkError>;
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct XmlStreamUsage {
    pub source_bytes: usize,
    pub decoded_bytes: usize,
    /// Projected values, completed start/end tags (two for empty tags), comments and PIs.
    pub events: usize,
    pub work: usize,
    pub retained_bytes: usize,
    pub peak_retained_bytes: usize,
    pub structure_bytes: usize,
    pub peak_structure_bytes: usize,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct XmlSemanticSummary {
    pub value_count: usize,
    pub decoded_bytes: usize,
    pub max_name_bytes: usize,
    pub max_depth: usize,
    pub traversal_work: usize,
}
#[derive(Clone, Copy, Default)]
struct Span {
    start: usize,
    len: usize,
}
#[derive(Clone, Copy)]
struct Frame {
    name: Span,
    mark: usize,
    bindings: usize,
}
#[derive(Clone, Copy)]
struct Binding {
    prefix: Span,
    uri: Span,
}
#[derive(Clone, Copy)]
struct Attr {
    name: Span,
    declaration: bool,
    local: Span,
    uri: Uri,
}
#[derive(Clone, Copy)]
enum Uri {
    Empty,
    Xml,
    Arena(Span),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Beginning,
    Text,
    Open,
    StartName,
    Between,
    AttrName,
    Equals,
    Quote,
    Value(u8),
    AfterValue,
    Slash,
    EndName,
    EndSpace,
    Declaration,
    Comment(u8),
    Cdata(u8),
    PiName,
    Pi(bool),
    Entity,
}
#[derive(Clone, Copy)]
enum Entity {
    Initial,
    Named { bytes: [u8; 5], len: usize },
    Hash,
    Numeric { radix: u32, value: u32, digits: usize },
}

pub struct XmlStream {
    limits: XmlStreamLimits,
    usage: XmlStreamUsage,
    summary: XmlSemanticSummary,
    state: State,
    failure: Option<XmlStreamError>,
    complete: bool,
    token: SmallVec<[u8; 1024]>,
    name: SmallVec<[u8; 256]>,
    arena: SmallVec<[u8; 256]>,
    frames: Vec<Frame>,
    bindings: Vec<Binding>,
    attrs: Vec<Attr>,
    element: Span,
    tag_mark: usize,
    binding_mark: usize,
    attribute: Span,
    roots: usize,
    declaration_allowed: bool,
    declaration: bool,
    pi_separator: bool,
    bom: u8,
    raw_token: usize,
    tag_work: usize,
    tag_trailing: usize,
    trailing: usize,
    brackets: u8,
    entity: Entity,
    entity_return: State,
    cr: bool,
    utf8: [u8; 4],
    utf8_len: usize,
    utf8_expected: usize,
    held_heap: usize,
    held_name: usize,
}
impl XmlStream {
    pub fn new(limits: XmlStreamLimits) -> Self {
        Self {
            limits,
            usage: XmlStreamUsage::default(),
            summary: XmlSemanticSummary::default(),
            state: State::Beginning,
            failure: None,
            complete: false,
            token: SmallVec::new(),
            name: SmallVec::new(),
            arena: SmallVec::new(),
            frames: Vec::new(),
            bindings: Vec::new(),
            attrs: Vec::new(),
            element: Span::default(),
            tag_mark: 0,
            binding_mark: 0,
            attribute: Span::default(),
            roots: 0,
            declaration_allowed: true,
            declaration: false,
            pi_separator: false,
            bom: 0,
            raw_token: 0,
            tag_work: 0,
            tag_trailing: 0,
            trailing: 0,
            brackets: 0,
            entity: Entity::Initial,
            entity_return: State::Text,
            cr: false,
            utf8: [0; 4],
            utf8_len: 0,
            utf8_expected: 0,
            held_heap: 0,
            held_name: 0,
        }
    }
    pub fn usage(&self) -> XmlStreamUsage {
        self.usage
    }
    pub fn summary(&self) -> XmlSemanticSummary {
        self.summary
    }
    pub fn feed(&mut self, input: XmlStreamInput<'_>, sink: &mut impl XmlStreamSink) -> Result<(), XmlStreamError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.complete {
            return Err(XmlStreamError::AlreadyComplete);
        }
        let result = match input {
            XmlStreamInput::Abandoned => Err(XmlStreamError::Abandoned),
            XmlStreamInput::Complete => self.finish(sink),
            XmlStreamInput::Data(data) => self.data(data, sink),
        };
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
    fn limit(&self, kind: XmlStreamLimit, value: usize, limit: usize) -> Result<(), XmlStreamError> {
        if value > limit {
            Err(XmlStreamError::Limit { kind, limit })
        } else {
            Ok(())
        }
    }
    fn work(&mut self, amount: usize) -> Result<(), XmlStreamError> {
        charge_work(&mut self.usage, self.limits.max_work, amount)
    }
    fn decoded(&mut self, amount: usize) -> Result<(), XmlStreamError> {
        let next = add(self.usage.decoded_bytes, amount)?;
        self.limit(XmlStreamLimit::DecodedBytes, next, self.limits.max_decoded_bytes)?;
        self.usage.decoded_bytes = next;
        Ok(())
    }
    /// A bounded lookahead cannot inspect source that the current token cannot admit.
    fn lookahead(&mut self, distance: usize) -> Result<(), XmlStreamError> {
        self.limit(
            XmlStreamLimit::SourceBytes,
            add(self.usage.source_bytes, distance)?,
            self.limits.max_source_bytes,
        )?;
        self.limit(
            XmlStreamLimit::RawTokenBytes,
            add(self.raw_token, distance)?,
            self.limits.max_raw_token_bytes,
        )?;
        self.work(1)
    }
    fn event(&mut self) -> Result<(), XmlStreamError> {
        self.usage.events = add(self.usage.events, 1)?;
        self.limit(XmlStreamLimit::Events, self.usage.events, self.limits.max_events)
    }
    fn heap(&self) -> Result<usize, XmlStreamError> {
        add(
            add(
                add(
                    add(heap_capacity(&self.token), heap_capacity(&self.name))?,
                    heap_capacity(&self.arena),
                )?,
                self.held_heap,
            )?,
            add(
                add(
                    mul(self.frames.capacity(), size_of::<Frame>())?,
                    mul(self.bindings.capacity(), size_of::<Binding>())?,
                )?,
                mul(self.attrs.capacity(), size_of::<Attr>())?,
            )?,
        )
    }
    fn refresh(&mut self) -> Result<(), XmlStreamError> {
        self.usage.retained_bytes = self.heap()?;
        self.usage.peak_retained_bytes = self.usage.peak_retained_bytes.max(self.usage.retained_bytes);
        // Declaration markers share inline scratch but are not active names.
        let name_bytes = if matches!(self.state, State::Declaration) {
            0
        } else {
            self.name.len()
        };
        self.usage.structure_bytes = add(add(self.arena.len(), name_bytes)?, self.held_name)?;
        self.usage.peak_structure_bytes = self.usage.peak_structure_bytes.max(self.usage.structure_bytes);
        self.limit(
            XmlStreamLimit::RetainedBytes,
            self.usage.retained_bytes,
            self.limits.max_retained_bytes,
        )?;
        self.limit(
            XmlStreamLimit::StructureBytes,
            self.usage.structure_bytes,
            self.limits.max_structure_bytes,
        )
    }
    fn reserve_bytes<A: Array<Item = u8>>(
        bytes: &mut SmallVec<A>,
        additional: usize,
        heap: usize,
        limit: usize,
        usage: &mut XmlStreamUsage,
        max_work: usize,
    ) -> Result<(), XmlStreamError> {
        let remaining = limit.checked_sub(heap).ok_or(XmlStreamError::Limit {
            kind: XmlStreamLimit::RetainedBytes,
            limit,
        })?;
        let target = growth_target(bytes, additional, remaining).map_err(|error| match error {
            GrowthError::Arithmetic => XmlStreamError::Arithmetic,
            GrowthError::Retained => XmlStreamError::Limit {
                kind: XmlStreamLimit::RetainedBytes,
                limit,
            },
        })?;
        let copied = if target.is_some() { bytes.len() } else { 0 };
        charge_work(usage, max_work, add(copied, additional)?)?;
        if let Some(target) = target {
            let old = heap_capacity(bytes);
            // Admit old/new allocation overlap, including the first inline spill.
            if add(heap, target)? > limit {
                return Err(XmlStreamError::Limit {
                    kind: XmlStreamLimit::RetainedBytes,
                    limit,
                });
            }
            bytes
                .try_reserve_exact(target - bytes.len())
                .map_err(|_| XmlStreamError::Allocation)?;
            refresh_heap(usage, add(heap - old, heap_capacity(bytes))?, limit)?;
        }
        Ok(())
    }
    fn token_byte(&mut self, byte: u8) -> Result<(), XmlStreamError> {
        if self.token.len() < self.token.capacity() {
            self.work(1)?;
            self.token.push(byte);
            return Ok(());
        }
        let heap = self.heap()?;
        Self::reserve_bytes(
            &mut self.token,
            1,
            heap,
            self.limits.max_retained_bytes,
            &mut self.usage,
            self.limits.max_work,
        )?;
        self.token.push(byte);
        self.refresh()
    }
    fn name_byte(&mut self, byte: u8) -> Result<(), XmlStreamError> {
        self.limit(
            XmlStreamLimit::NameBytes,
            add(self.name.len(), 1)?,
            self.limits.max_name_bytes,
        )?;
        self.limit(
            XmlStreamLimit::StructureBytes,
            add(add(add(self.arena.len(), self.name.len())?, self.held_name)?, 1)?,
            self.limits.max_structure_bytes,
        )?;
        let heap = self.heap()?;
        Self::reserve_bytes(
            &mut self.name,
            1,
            heap,
            self.limits.max_retained_bytes,
            &mut self.usage,
            self.limits.max_work,
        )?;
        self.name.push(byte);
        self.refresh()
    }
    fn arena_copy(&mut self, bytes: &[u8]) -> Result<Span, XmlStreamError> {
        self.limit(
            XmlStreamLimit::StructureBytes,
            add(
                add(add(self.arena.len(), self.name.len())?, self.held_name)?,
                bytes.len(),
            )?,
            self.limits.max_structure_bytes,
        )?;
        let heap = self.heap()?;
        Self::reserve_bytes(
            &mut self.arena,
            bytes.len(),
            heap,
            self.limits.max_retained_bytes,
            &mut self.usage,
            self.limits.max_work,
        )?;
        let span = Span {
            start: self.arena.len(),
            len: bytes.len(),
        };
        self.arena.extend_from_slice(bytes);
        self.refresh()?;
        Ok(span)
    }
    fn save_name(&mut self) -> Result<Span, XmlStreamError> {
        self.work(mul(self.name.len(), 4)?)?;
        validate_qname(&self.name)?;
        self.held_name = self.name.len();
        self.held_heap = add(self.held_heap, heap_capacity(&self.name))?;
        let mut name = std::mem::take(&mut self.name);
        let result = self.arena_copy(&name);
        name.clear();
        self.held_name = 0;
        self.held_heap -= heap_capacity(&name);
        self.name = name;
        let current = self.refresh();
        result.and_then(|span| current.map(|()| span))
    }
    fn reserve_vec<T>(
        values: &mut Vec<T>,
        heap: usize,
        limit: usize,
        usage: &mut XmlStreamUsage,
        max_work: usize,
    ) -> Result<(), XmlStreamError> {
        if values.len() < values.capacity() {
            return Ok(());
        }
        let target = values
            .capacity()
            .checked_mul(2)
            .ok_or(XmlStreamError::Arithmetic)?
            .max(4);
        let old = mul(values.capacity(), size_of::<T>())?;
        if add(heap, mul(target, size_of::<T>())?)? > limit {
            return Err(XmlStreamError::Limit {
                kind: XmlStreamLimit::RetainedBytes,
                limit,
            });
        }
        charge_work(usage, max_work, mul(values.len(), size_of::<T>())?)?;
        values
            .try_reserve_exact(target - values.len())
            .map_err(|_| XmlStreamError::Allocation)?;
        refresh_heap(usage, add(heap - old, mul(values.capacity(), size_of::<T>())?)?, limit)
    }
    fn span(&self, span: Span) -> &[u8] {
        &self.arena[span.start..span.start + span.len]
    }
    fn equal_spans(&mut self, left: Span, right: Span) -> Result<bool, XmlStreamError> {
        self.work(add(left.len.min(right.len), 1)?)?;
        Ok(self.span(left) == self.span(right))
    }
    fn character(&mut self, byte: u8) -> Result<(), XmlStreamError> {
        if self.utf8_expected == 0 {
            if byte < 128 {
                if !xml_char(u32::from(byte)) {
                    return Err(XmlStreamError::Character);
                }
                return Ok(());
            }
            self.utf8_expected = match byte {
                0xC2..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF4 => 4,
                _ => return Err(XmlStreamError::Character),
            };
            self.utf8[0] = byte;
            self.utf8_len = 1;
        } else {
            if byte & 0xC0 != 0x80 {
                return Err(XmlStreamError::Character);
            }
            self.utf8[self.utf8_len] = byte;
            self.utf8_len += 1;
            if self.utf8_len == self.utf8_expected {
                let value = std::str::from_utf8(&self.utf8[..self.utf8_len]).map_err(|_| XmlStreamError::Character)?;
                if !xml_char(value.chars().next().ok_or(XmlStreamError::Character)? as u32) {
                    return Err(XmlStreamError::Character);
                }
                self.utf8_expected = 0;
                self.utf8_len = 0;
            }
        }
        Ok(())
    }
    fn admit_byte(&mut self, byte: u8) -> Result<(), XmlStreamError> {
        let source = add(self.usage.source_bytes, 1)?;
        self.limit(XmlStreamLimit::SourceBytes, source, self.limits.max_source_bytes)?;
        self.usage.source_bytes = source;
        self.raw_token = add(self.raw_token, 1)?;
        self.limit(
            XmlStreamLimit::RawTokenBytes,
            self.raw_token,
            self.limits.max_raw_token_bytes,
        )?;
        self.work(1)?;
        self.character(byte)
    }
    fn data(&mut self, data: &[u8], sink: &mut impl XmlStreamSink) -> Result<(), XmlStreamError> {
        let mut cursor = 0;
        let mut probe_until = 0;
        while cursor < data.len() {
            // CDATA has no entity decoding or trimming. Its complete ordinary
            // source can be observed directly, including the empty value.
            if cursor >= probe_until && self.state == State::Cdata(0) && self.token.is_empty() && !self.cr {
                let mut scan = cursor;
                let mut brackets = 0u8;
                let mut plain = true;
                let mut delimiter = None;
                while scan < data.len() {
                    self.lookahead(scan - cursor + 1)?;
                    let byte = data[scan];
                    scan += 1;
                    plain &= byte != b'\r';
                    if byte == b'>' && brackets == 2 {
                        delimiter = Some(scan - 3);
                        break;
                    }
                    brackets = if byte == b']' { (brackets + 1).min(2) } else { 0 };
                }
                probe_until = scan;
                if let Some(end) = delimiter.filter(|_| plain) {
                    for &byte in &data[cursor..end + 3] {
                        self.admit_byte(byte)?;
                    }
                    if self.utf8_expected != 0 {
                        return Err(XmlStreamError::Character);
                    }
                    self.decoded(end - cursor)?;
                    self.emit(&data[cursor..end], None, sink)?;
                    self.state = State::Text;
                    self.raw_token = 0;
                    cursor = end + 3;
                    continue;
                }
            }
            // Each failed probe is scanned at most once per Data chunk. Whitespace
            // may leave token empty, so scratch occupancy alone is not a guard.
            if cursor >= probe_until && self.token.is_empty() && !self.cr && self.utf8_expected == 0 {
                let quote = match self.state {
                    State::Value(q) => Some(q),
                    _ => None,
                };
                if self.state == State::Text || quote.is_some() {
                    let start = cursor;
                    let mut end = cursor;
                    while end < data.len() {
                        self.lookahead(end - start + 1)?;
                        let byte = data[end];
                        if byte == b'&'
                            || byte == b'<'
                            || byte == b'\r'
                            || byte == b']'
                            || quote == Some(byte)
                            || (quote.is_some() && matches!(byte, b'\n' | b'\t'))
                        {
                            break;
                        }
                        end += 1;
                    }
                    probe_until = end.max(cursor + 1);
                    let terminated = data
                        .get(end)
                        .is_some_and(|byte| (self.state == State::Text && *byte == b'<') || quote == Some(*byte));
                    if terminated && (end > start || quote.is_some()) {
                        for &byte in &data[start..end] {
                            self.admit_byte(byte)?;
                            self.account_tag_byte(byte)?;
                        }
                        if self.utf8_expected != 0 {
                            return Err(XmlStreamError::Character);
                        }
                        if quote.is_none() && self.frames.is_empty() {
                            self.declaration_allowed = false;
                        }
                        if quote.is_none() {
                            self.work(mul(end - start, 2)?)?;
                        }
                        let value = if quote.is_some() {
                            &data[start..end]
                        } else {
                            trim_xml(&data[start..end])
                        };
                        self.decoded(value.len())?;
                        if quote.is_some() || !value.is_empty() {
                            self.emit(value, quote.is_some().then_some(self.attribute), sink)?;
                        }
                        if quote.is_some() {
                            self.attribute_done(value)?;
                            self.state = State::AfterValue;
                        }
                        self.admit_byte(data[end])?;
                        self.account_tag_byte(data[end])?;
                        if quote.is_none() {
                            self.begin_markup();
                        }
                        if quote.is_some() {
                            self.raw_token = 0;
                        }
                        cursor = end + 1;
                        continue;
                    }
                }
            }
            let byte = data[cursor];
            self.admit_byte(byte)?;
            self.account_tag_byte(byte)?;
            cursor += 1;
            if self.cr && byte == b'\n' {
                self.cr = false;
                continue;
            }
            self.cr = byte == b'\r';
            self.advance(if self.cr { b'\n' } else { byte }, sink)?;
        }
        Ok(())
    }
    fn account_tag_byte(&mut self, byte: u8) -> Result<(), XmlStreamError> {
        let tag = matches!(
            self.state,
            State::StartName
                | State::Between
                | State::AttrName
                | State::Equals
                | State::Quote
                | State::Value(_)
                | State::AfterValue
                | State::Slash
                | State::EndName
                | State::EndSpace
        ) || (self.state == State::Entity && matches!(self.entity_return, State::Value(_)));
        let close = byte == b'>'
            && matches!(
                self.state,
                State::StartName | State::Between | State::AfterValue | State::Slash | State::EndName | State::EndSpace
            );
        if tag && !close {
            self.tag_work = add(self.tag_work, 1)?;
            self.tag_trailing = if xml_ws(byte) { add(self.tag_trailing, 1)? } else { 0 };
        }
        Ok(())
    }
    fn begin_markup(&mut self) {
        self.state = State::Open;
        self.raw_token = 1;
        self.brackets = 0;
        self.tag_work = 0;
        self.tag_trailing = 0;
    }
    fn prepare_emit(&mut self, value_len: usize, name: Option<Span>) -> Result<(), XmlStreamError> {
        if name.is_none() && self.frames.is_empty() {
            return Err(XmlStreamError::Syntax);
        }
        self.event()?;

        self.summary.value_count = add(self.summary.value_count, 1)?;
        self.summary.decoded_bytes = add(
            self.summary.decoded_bytes,
            add(name.map_or(0, |name| name.len), value_len)?,
        )?;
        self.summary.max_name_bytes = self.summary.max_name_bytes.max(name.map_or(0, |name| name.len));
        self.work(value_len)?;
        Ok(())
    }
    fn emit(&mut self, value: &[u8], name: Option<Span>, sink: &mut impl XmlStreamSink) -> Result<(), XmlStreamError> {
        self.prepare_emit(value.len(), name)?;
        sink.value(name.map(|name| self.span(name)), value)
            .map_err(|_| XmlStreamError::SinkRejected)
    }
    fn emit_token(
        &mut self,
        text: bool,
        name: Option<Span>,
        sink: &mut impl XmlStreamSink,
    ) -> Result<(), XmlStreamError> {
        if text {
            self.token.truncate(self.token.len() - self.trailing);
        }
        let result = if !text || !self.token.is_empty() {
            self.prepare_emit(self.token.len(), name).and_then(|()| {
                sink.value(name.map(|name| self.span(name)), &self.token)
                    .map_err(|_| XmlStreamError::SinkRejected)
            })
        } else {
            Ok(())
        };
        self.token.clear();
        self.trailing = 0;
        result
    }
    fn start_element(&mut self) -> Result<(), XmlStreamError> {
        self.element = self.save_name()?;
        self.summary.max_depth = self.summary.max_depth.max(add(self.frames.len(), 1)?);
        self.limit(XmlStreamLimit::Depth, add(self.frames.len(), 1)?, self.limits.max_depth)?;
        if self.frames.is_empty() {
            self.roots = add(self.roots, 1)?;
            if self.roots != 1 {
                return Err(XmlStreamError::Syntax);
            }
        }
        Ok(())
    }
    fn attribute_name(&mut self) -> Result<(), XmlStreamError> {
        self.attribute = self.save_name()?;
        self.limit(
            XmlStreamLimit::Attributes,
            add(self.attrs.len(), 1)?,
            self.limits.max_attributes,
        )?;
        Ok(())
    }
    fn attribute_done(&mut self, value: &[u8]) -> Result<(), XmlStreamError> {
        let name = self.attribute;
        let declaration = self.span(name) == b"xmlns" || self.span(name).starts_with(b"xmlns:");
        if declaration {
            let prefix = if self.span(name) == b"xmlns" {
                Span {
                    start: name.start,
                    len: 0,
                }
            } else {
                Span {
                    start: name.start + 6,
                    len: name.len - 6,
                }
            };
            if self.span(prefix) == b"xmlns"
                || value == b"http://www.w3.org/2000/xmlns/"
                || (self.span(prefix) == b"xml") != (value == b"http://www.w3.org/XML/1998/namespace")
                || (!self.span(prefix).is_empty() && value.is_empty())
            {
                return Err(XmlStreamError::Syntax);
            }
            self.limit(
                XmlStreamLimit::Namespaces,
                add(self.bindings.len(), 1)?,
                self.limits.max_namespaces,
            )?;
            let uri = self.arena_copy(value)?;
            let heap = self.heap()?;
            Self::reserve_vec(
                &mut self.bindings,
                heap,
                self.limits.max_retained_bytes,
                &mut self.usage,
                self.limits.max_work,
            )?;
            self.bindings.push(Binding { prefix, uri });
        }
        let heap = self.heap()?;
        Self::reserve_vec(
            &mut self.attrs,
            heap,
            self.limits.max_retained_bytes,
            &mut self.usage,
            self.limits.max_work,
        )?;
        self.attrs.push(Attr {
            name,
            declaration,
            local: name,
            uri: Uri::Empty,
        });
        self.refresh()
    }
    fn prefix_local(&self, name: Span) -> (Span, Span) {
        match memchr::memchr(b':', self.span(name)) {
            Some(index) => (
                Span {
                    start: name.start,
                    len: index,
                },
                Span {
                    start: name.start + index + 1,
                    len: name.len - index - 1,
                },
            ),
            None => (
                Span {
                    start: name.start,
                    len: 0,
                },
                name,
            ),
        }
    }
    fn uri(&mut self, name: Span, attribute: bool) -> Result<Uri, XmlStreamError> {
        self.work(name.len)?;
        let (prefix, _) = self.prefix_local(name);
        self.prefix_uri(prefix, attribute)
    }
    fn prefix_uri(&mut self, prefix: Span, attribute: bool) -> Result<Uri, XmlStreamError> {
        if self.span(prefix) == b"xml" {
            return Ok(Uri::Xml);
        }
        if self.span(prefix) == b"xmlns" {
            return Err(XmlStreamError::Syntax);
        }
        if prefix.len == 0 && attribute {
            return Ok(Uri::Empty);
        }
        for index in (0..self.bindings.len()).rev() {
            let binding = self.bindings[index];
            if self.equal_spans(prefix, binding.prefix)? {
                return Ok(Uri::Arena(binding.uri));
            }
        }
        if prefix.len != 0 {
            Err(XmlStreamError::Syntax)
        } else {
            Ok(Uri::Empty)
        }
    }
    fn uri_bytes(&self, uri: Uri) -> &[u8] {
        match uri {
            Uri::Empty => b"",
            Uri::Xml => b"http://www.w3.org/XML/1998/namespace",
            Uri::Arena(span) => self.span(span),
        }
    }
    fn attr_compare(
        &mut self,
        left: usize,
        right: usize,
        expanded: bool,
    ) -> Result<std::cmp::Ordering, XmlStreamError> {
        let left = self.attrs[left];
        let right = self.attrs[right];
        if expanded {
            self.work(1)?;
            let order = left.declaration.cmp(&right.declaration);
            if order != std::cmp::Ordering::Equal {
                return Ok(order);
            }
            self.work(add(
                self.uri_bytes(left.uri).len().min(self.uri_bytes(right.uri).len()),
                1,
            )?)?;
            let order = self.uri_bytes(left.uri).cmp(self.uri_bytes(right.uri));
            if order != std::cmp::Ordering::Equal {
                return Ok(order);
            }
        }
        let left = if expanded { left.local } else { left.name };
        let right = if expanded { right.local } else { right.name };
        self.work(add(left.len.min(right.len), 1)?)?;
        Ok(self.span(left).cmp(self.span(right)))
    }
    fn attr_sift(&mut self, mut root: usize, end: usize, expanded: bool) -> Result<(), XmlStreamError> {
        while let Some(left) = root.checked_mul(2).and_then(|n| n.checked_add(1)).filter(|n| *n < end) {
            let mut child = left;
            if left + 1 < end && self.attr_compare(left, left + 1, expanded)? == std::cmp::Ordering::Less {
                child += 1;
            }
            if self.attr_compare(root, child, expanded)? != std::cmp::Ordering::Less {
                break;
            }
            self.work(1)?;
            self.attrs.swap(root, child);
            root = child;
        }
        Ok(())
    }
    fn sort_attrs(&mut self, expanded: bool) -> Result<(), XmlStreamError> {
        let n = self.attrs.len();
        for root in (0..n / 2).rev() {
            self.attr_sift(root, n, expanded)?;
        }
        for end in (1..n).rev() {
            self.work(1)?;
            self.attrs.swap(0, end);
            self.attr_sift(0, end, expanded)?;
        }
        Ok(())
    }
    fn close_start(&mut self, empty: bool) -> Result<(), XmlStreamError> {
        self.uri(self.element, false)?;
        self.sort_attrs(false)?;
        for index in 1..self.attrs.len() {
            if self.equal_spans(self.attrs[index - 1].name, self.attrs[index].name)? {
                return Err(XmlStreamError::Syntax);
            }
        }
        let mut expanded_needed = false;
        for index in 0..self.attrs.len() {
            if self.attrs[index].declaration {
                continue;
            }
            let name = self.attrs[index].name;
            self.work(name.len)?;
            let (prefix, local) = self.prefix_local(name);
            let uri = if prefix.len == 0 {
                Uri::Empty
            } else {
                expanded_needed = true;
                self.prefix_uri(prefix, true)?
            };
            self.attrs[index].local = local;
            self.attrs[index].uri = uri;
        }
        if expanded_needed {
            self.sort_attrs(true)?;
            for index in 1..self.attrs.len() {
                if !self.attrs[index].declaration
                    && self.attr_compare(index - 1, index, true)? == std::cmp::Ordering::Equal
                {
                    return Err(XmlStreamError::Syntax);
                }
            }
        }
        self.event()?;
        if empty {
            self.arena.truncate(self.tag_mark);
            self.bindings.truncate(self.binding_mark);
            self.event()?;
        } else {
            // Compact only the bindings introduced by this tag; ordinary attribute
            // identities cease to be live after namespace/duplicate validation.
            let mut end = add(self.element.start, self.element.len)?;
            for index in self.binding_mark..self.bindings.len() {
                let binding = self.bindings[index];
                self.work(add(binding.prefix.len, binding.uri.len)?)?;
                self.arena.copy_within(
                    binding.prefix.start..add(binding.prefix.start, binding.prefix.len)?,
                    end,
                );
                let prefix = Span {
                    start: end,
                    len: binding.prefix.len,
                };
                end = add(end, prefix.len)?;
                self.arena
                    .copy_within(binding.uri.start..add(binding.uri.start, binding.uri.len)?, end);
                let uri = Span {
                    start: end,
                    len: binding.uri.len,
                };
                end = add(end, uri.len)?;
                self.bindings[index] = Binding { prefix, uri };
            }
            self.arena.truncate(end);
            let heap = self.heap()?;
            Self::reserve_vec(
                &mut self.frames,
                heap,
                self.limits.max_retained_bytes,
                &mut self.usage,
                self.limits.max_work,
            )?;
            self.frames.push(Frame {
                name: self.element,
                mark: self.tag_mark,
                bindings: self.binding_mark,
            });
        }
        self.attrs.clear();
        self.summary.traversal_work = add(self.summary.traversal_work, self.tag_work - self.tag_trailing)?;
        self.state = State::Text;
        self.raw_token = 0;
        self.refresh()
    }
    fn close_end(&mut self) -> Result<(), XmlStreamError> {
        self.work(mul(self.name.len(), 4)?)?;
        validate_qname(&self.name)?;
        let frame = *self.frames.last().ok_or(XmlStreamError::Syntax)?;
        self.work(add(self.name.len().min(frame.name.len), 1)?)?;
        if self.name.as_slice() != self.span(frame.name) {
            return Err(XmlStreamError::Syntax);
        }
        self.name.clear();
        self.frames.pop();
        self.arena.truncate(frame.mark);
        self.bindings.truncate(frame.bindings);
        self.event()?;
        self.summary.traversal_work = add(self.summary.traversal_work, self.tag_work - self.tag_trailing)?;
        self.state = State::Text;
        self.raw_token = 0;
        self.refresh()
    }
    fn entity_byte(&mut self, byte: u8) -> Result<(), XmlStreamError> {
        if byte == b';' {
            let value = match self.entity {
                Entity::Named { bytes, len } => match &bytes[..len] {
                    b"amp" => '&',
                    b"lt" => '<',
                    b"gt" => '>',
                    b"quot" => '"',
                    b"apos" => '\'',
                    _ => return Err(XmlStreamError::Syntax),
                },
                Entity::Numeric { value, digits, .. } if digits != 0 && xml_char(value) => {
                    char::from_u32(value).ok_or(XmlStreamError::Syntax)?
                }
                _ => return Err(XmlStreamError::Syntax),
            };
            let mut bytes = [0; 4];
            let bytes = value.encode_utf8(&mut bytes).as_bytes();
            self.decoded(add(
                bytes.len(),
                if self.entity_return == State::Text {
                    self.trailing
                } else {
                    0
                },
            )?)?;
            for &byte in bytes {
                self.token_byte(byte)?;
            }
            self.trailing = 0;
            self.state = self.entity_return;
            return Ok(());
        }
        self.entity = match self.entity {
            Entity::Initial if byte == b'#' => Entity::Hash,
            Entity::Initial => Entity::Named {
                bytes: [byte, 0, 0, 0, 0],
                len: 1,
            },
            Entity::Named { mut bytes, len } if len < 5 => {
                bytes[len] = byte;
                Entity::Named { bytes, len: len + 1 }
            }
            Entity::Hash if byte == b'x' => Entity::Numeric {
                radix: 16,
                value: 0,
                digits: 0,
            },
            Entity::Hash => {
                let value = (byte as char).to_digit(10).ok_or(XmlStreamError::Syntax)?;
                Entity::Numeric {
                    radix: 10,
                    value,
                    digits: 1,
                }
            }
            Entity::Numeric { radix, value, digits } => {
                let digit = (byte as char).to_digit(radix).ok_or(XmlStreamError::Syntax)?;
                let value = value
                    .checked_mul(radix)
                    .and_then(|v| v.checked_add(digit))
                    .filter(|v| *v <= 0x10FFFF)
                    .ok_or(XmlStreamError::Syntax)?;
                Entity::Numeric {
                    radix,
                    value,
                    digits: add(digits, 1)?,
                }
            }
            _ => return Err(XmlStreamError::Syntax),
        };
        Ok(())
    }
    fn advance(&mut self, byte: u8, sink: &mut impl XmlStreamSink) -> Result<(), XmlStreamError> {
        match self.state {
            State::Beginning => {
                let bom = [0xEF, 0xBB, 0xBF];
                if self.bom != 0 || byte == 0xEF {
                    if bom[usize::from(self.bom)] != byte {
                        return Err(XmlStreamError::Syntax);
                    }
                    self.bom += 1;
                    if self.bom == 3 {
                        self.state = State::Text;
                        self.raw_token = 0;
                    }
                } else {
                    self.state = State::Text;
                    self.advance(byte, sink)?;
                }
            }
            State::Text => {
                if byte == b'<' {
                    self.emit_token(true, None, sink)?;
                    self.begin_markup();
                } else if byte == b'&' {
                    if self.frames.is_empty() {
                        return Err(XmlStreamError::Syntax);
                    }
                    self.declaration_allowed = false;
                    self.brackets = 0;
                    self.entity_return = State::Text;
                    self.entity = Entity::Initial;
                    self.state = State::Entity;
                } else {
                    if self.frames.is_empty() {
                        if !xml_ws(byte) {
                            return Err(XmlStreamError::Syntax);
                        }
                        self.declaration_allowed = false;
                    } else {
                        self.declaration_allowed = false;
                        if byte == b'>' && self.brackets == 2 {
                            return Err(XmlStreamError::Syntax);
                        }
                        self.brackets = if byte == b']' { (self.brackets + 1).min(2) } else { 0 };
                        if !xml_ws(byte) || !self.token.is_empty() {
                            if !xml_ws(byte) {
                                self.decoded(add(self.trailing, 1)?)?;
                            }
                            self.token_byte(byte)?;
                            self.trailing = if xml_ws(byte) { add(self.trailing, 1)? } else { 0 };
                        }
                    }
                }
            }
            State::Open => match byte {
                b'/' => {
                    self.name.clear();
                    self.state = State::EndName;
                    self.tag_work = 1;
                    self.declaration_allowed = false;
                }
                b'!' => {
                    self.name.clear();
                    self.state = State::Declaration;
                    self.declaration_allowed = false;
                }
                b'?' => {
                    self.name.clear();
                    self.token.clear();
                    self.state = State::PiName;
                }
                _ => {
                    self.declaration_allowed = false;
                    self.tag_mark = self.arena.len();
                    self.binding_mark = self.bindings.len();
                    self.attrs.clear();
                    self.name.clear();
                    self.state = State::StartName;
                    self.tag_work = 1;
                    self.name_byte(byte)?;
                }
            },
            State::StartName => {
                if xml_ws(byte) || matches!(byte, b'/' | b'>') {
                    self.start_element()?;
                    self.state = State::Between;
                    self.advance(byte, sink)?;
                } else {
                    self.name_byte(byte)?;
                }
            }
            State::Between => match byte {
                b'>' => self.close_start(false)?,
                b'/' => self.state = State::Slash,
                byte if xml_ws(byte) => {}
                _ => {
                    self.name.clear();
                    self.name_byte(byte)?;
                    self.state = State::AttrName;
                }
            },
            State::AttrName => {
                if xml_ws(byte) || byte == b'=' {
                    self.attribute_name()?;
                    self.state = State::Equals;
                    self.advance(byte, sink)?;
                } else {
                    self.name_byte(byte)?;
                }
            }
            State::Equals => {
                if byte == b'=' {
                    self.state = State::Quote;
                } else if !xml_ws(byte) {
                    return Err(XmlStreamError::Syntax);
                }
            }
            State::Quote => {
                if matches!(byte, b'\'' | b'"') {
                    self.token.clear();
                    self.state = State::Value(byte);
                    self.raw_token = 0;
                } else if !xml_ws(byte) {
                    return Err(XmlStreamError::Syntax);
                }
            }
            State::Value(quote) => {
                if byte == quote {
                    self.held_heap = add(self.held_heap, heap_capacity(&self.token))?;
                    let mut token = std::mem::take(&mut self.token);
                    let result = self
                        .emit(&token, Some(self.attribute), sink)
                        .and_then(|()| self.attribute_done(&token));
                    token.clear();
                    self.held_heap -= heap_capacity(&token);
                    self.token = token;
                    result?;
                    self.state = State::AfterValue;
                    self.raw_token = 0;
                } else if byte == b'&' {
                    self.entity_return = self.state;
                    self.entity = Entity::Initial;
                    self.state = State::Entity;
                } else if byte == b'<' {
                    return Err(XmlStreamError::Syntax);
                } else {
                    self.decoded(1)?;
                    self.token_byte(if xml_ws(byte) { b' ' } else { byte })?;
                }
            }
            State::AfterValue => {
                if xml_ws(byte) {
                    self.state = State::Between;
                } else if matches!(byte, b'/' | b'>') {
                    self.state = State::Between;
                    self.advance(byte, sink)?;
                } else {
                    return Err(XmlStreamError::Syntax);
                }
            }
            State::Slash => {
                if byte != b'>' {
                    return Err(XmlStreamError::Syntax);
                }
                self.close_start(true)?;
            }
            State::EndName => {
                if byte == b'>' {
                    self.close_end()?;
                } else if xml_ws(byte) {
                    self.state = State::EndSpace;
                } else {
                    self.name_byte(byte)?;
                }
            }
            State::EndSpace => {
                if byte == b'>' {
                    self.close_end()?;
                } else if !xml_ws(byte) {
                    return Err(XmlStreamError::Syntax);
                }
            }
            State::Declaration => {
                // Fixed markup discriminators are not lexical names.
                if self.name.len() >= 7 {
                    return Err(XmlStreamError::Syntax);
                }
                self.work(1)?;
                self.name.push(byte);
                self.refresh()?;
                self.work(14)?;
                if self.name.as_slice() == b"--" {
                    self.name.clear();
                    self.state = State::Comment(0);
                } else if self.name.as_slice() == b"[CDATA[" {
                    if self.frames.is_empty() {
                        return Err(XmlStreamError::Syntax);
                    }
                    self.name.clear();
                    self.token.clear();
                    self.state = State::Cdata(0);
                } else if !b"--".starts_with(&self.name) && !b"[CDATA[".starts_with(&self.name) {
                    return Err(XmlStreamError::Syntax);
                }
            }
            State::Comment(dashes) => {
                if dashes == 2 {
                    if byte != b'>' {
                        return Err(XmlStreamError::Syntax);
                    }
                    self.event()?;
                    self.state = State::Text;
                    self.raw_token = 0;
                } else {
                    self.state = State::Comment(if byte == b'-' { dashes + 1 } else { 0 });
                }
            }
            State::Cdata(brackets) => {
                if brackets == 2 && byte == b'>' {
                    self.emit_token(false, None, sink)?;
                    self.state = State::Text;
                    self.raw_token = 0;
                } else if byte == b']' {
                    if brackets == 2 {
                        self.decoded(1)?;
                        self.token_byte(b']')?;
                    }
                    self.state = State::Cdata((brackets + 1).min(2));
                } else {
                    self.decoded(add(usize::from(brackets), 1)?)?;
                    for _ in 0..brackets {
                        self.token_byte(b']')?;
                    }
                    self.token_byte(byte)?;
                    self.state = State::Cdata(0);
                }
            }
            State::PiName => {
                if xml_ws(byte) || byte == b'?' {
                    self.work(mul(self.name.len(), 2)?)?;
                    validate_ncname(&self.name)?;
                    self.declaration = self.name.as_slice() == b"xml";
                    if self.name.eq_ignore_ascii_case(b"xml")
                        && (!self.declaration || !self.declaration_allowed || byte == b'?')
                    {
                        return Err(XmlStreamError::Syntax);
                    }
                    self.declaration_allowed = false;
                    self.name.clear();
                    self.refresh()?;
                    self.pi_separator = xml_ws(byte);
                    self.state = State::Pi(byte == b'?');
                    if self.declaration {
                        self.token_byte(byte)?;
                    }
                } else {
                    self.name_byte(byte)?;
                }
            }
            State::Pi(question) => {
                if !(self.pi_separator || question && byte == b'>') {
                    return Err(XmlStreamError::Syntax);
                }
                if question && byte == b'>' {
                    if self.declaration {
                        self.work(mul(self.token.len(), 4)?)?;
                        validate_declaration(&self.token)?;
                        self.token.clear();
                    }
                    self.event()?;
                    self.state = State::Text;
                    self.raw_token = 0;
                } else {
                    if self.declaration {
                        if question {
                            self.token_byte(b'?')?;
                        }
                        if byte != b'?' {
                            self.token_byte(byte)?;
                        }
                    }
                    self.state = State::Pi(byte == b'?');
                }
            }
            State::Entity => self.entity_byte(byte)?,
        }
        Ok(())
    }
    fn finish(&mut self, sink: &mut impl XmlStreamSink) -> Result<(), XmlStreamError> {
        if self.utf8_expected != 0 {
            return Err(XmlStreamError::Character);
        }
        if self.state != State::Text || !self.frames.is_empty() || self.roots != 1 {
            return Err(XmlStreamError::Incomplete);
        }
        self.emit_token(true, None, sink)?;
        self.complete = true;
        Ok(())
    }
}
fn charge_work(usage: &mut XmlStreamUsage, limit: usize, amount: usize) -> Result<(), XmlStreamError> {
    usage.work = add(usage.work, amount)?;
    if usage.work > limit {
        Err(XmlStreamError::Limit {
            kind: XmlStreamLimit::Work,
            limit,
        })
    } else {
        Ok(())
    }
}
fn refresh_heap(usage: &mut XmlStreamUsage, heap: usize, limit: usize) -> Result<(), XmlStreamError> {
    usage.retained_bytes = heap;
    usage.peak_retained_bytes = usage.peak_retained_bytes.max(heap);
    if heap > limit {
        Err(XmlStreamError::Limit {
            kind: XmlStreamLimit::RetainedBytes,
            limit,
        })
    } else {
        Ok(())
    }
}
fn add(left: usize, right: usize) -> Result<usize, XmlStreamError> {
    left.checked_add(right).ok_or(XmlStreamError::Arithmetic)
}
fn mul(left: usize, right: usize) -> Result<usize, XmlStreamError> {
    left.checked_mul(right).ok_or(XmlStreamError::Arithmetic)
}
fn xml_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}
fn trim_xml(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(|b| xml_ws(*b)) {
        value = &value[1..];
    }
    while value.last().is_some_and(|b| xml_ws(*b)) {
        value = &value[..value.len() - 1];
    }
    value
}
fn xml_char(value: u32) -> bool {
    matches!(value, 9 | 10 | 13)
        || (0x20..=0xD7FF).contains(&value)
        || (0xE000..=0xFFFD).contains(&value)
        || (0x10000..=0x10FFFF).contains(&value)
}
fn name_start(value: char) -> bool {
    let c = value as u32;
    value == '_'
        || value.is_ascii_alphabetic()
        || (0xC0..=0xD6).contains(&c)
        || (0xD8..=0xF6).contains(&c)
        || (0xF8..=0x2FF).contains(&c)
        || (0x370..=0x37D).contains(&c)
        || (0x37F..=0x1FFF).contains(&c)
        || (0x200C..=0x200D).contains(&c)
        || (0x2070..=0x218F).contains(&c)
        || (0x2C00..=0x2FEF).contains(&c)
        || (0x3001..=0xD7FF).contains(&c)
        || (0xF900..=0xFDCF).contains(&c)
        || (0xFDF0..=0xFFFD).contains(&c)
        || (0x10000..=0xEFFFF).contains(&c)
}
fn validate_ncname(bytes: &[u8]) -> Result<(), XmlStreamError> {
    let text = std::str::from_utf8(bytes).map_err(|_| XmlStreamError::Character)?;
    let mut chars = text.chars();
    if !chars.next().is_some_and(name_start) {
        return Err(XmlStreamError::Syntax);
    }
    if chars.any(|c| {
        !(name_start(c)
            || c.is_ascii_digit()
            || matches!(c, '-' | '.' | '\u{B7}')
            || (0x300..=0x36F).contains(&(c as u32))
            || (0x203F..=0x2040).contains(&(c as u32)))
    }) {
        return Err(XmlStreamError::Syntax);
    }
    Ok(())
}
fn validate_qname(bytes: &[u8]) -> Result<(), XmlStreamError> {
    let mut multiple = false;
    for (index, part) in bytes.split(|b| *b == b':').enumerate() {
        validate_ncname(part)?;
        multiple |= index > 1;
    }
    if multiple {
        return Err(XmlStreamError::Syntax);
    }
    Ok(())
}
fn validate_declaration(bytes: &[u8]) -> Result<(), XmlStreamError> {
    let mut rest = bytes;
    let mut index = 0;
    loop {
        let trimmed = trim_xml(rest);
        if trimmed.is_empty() {
            break;
        }
        if !rest.first().is_some_and(|b| xml_ws(*b)) {
            return Err(XmlStreamError::Syntax);
        }
        rest = trimmed;
        let end = rest
            .iter()
            .position(|b| xml_ws(*b) || *b == b'=')
            .ok_or(XmlStreamError::Syntax)?;
        let key = &rest[..end];
        rest = trim_xml(&rest[end..]);
        if rest.first() != Some(&b'=') {
            return Err(XmlStreamError::Syntax);
        }
        rest = trim_xml(&rest[1..]);
        let quote = *rest.first().ok_or(XmlStreamError::Syntax)?;
        if !matches!(quote, b'\'' | b'"') {
            return Err(XmlStreamError::Syntax);
        }
        rest = &rest[1..];
        let end = memchr::memchr(quote, rest).ok_or(XmlStreamError::Syntax)?;
        let value = &rest[..end];
        rest = &rest[end + 1..];
        index = match (index, key, value) {
            (0, b"version", b"1.0") => 1,
            (1, b"encoding", value) if value.eq_ignore_ascii_case(b"UTF-8") => 2,
            (1 | 2, b"standalone", b"yes" | b"no") => 3,
            _ => return Err(XmlStreamError::Syntax),
        };
    }
    if index == 0 {
        Err(XmlStreamError::Syntax)
    } else {
        Ok(())
    }
}
#[cfg(test)]
#[path = "../../tests/unit/seclang/xml_stream_test.rs"]
mod tests;
