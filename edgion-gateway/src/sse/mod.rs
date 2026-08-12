//! Bounded slice-chain Server-Sent Events (SSE) substrate.

pub(crate) mod chain;
pub mod window;

use std::borrow::Cow;
use std::io::{self, Read};

use bytes::Bytes;
use serde::de::DeserializeOwned;

pub use chain::ByteRange;
use chain::SliceChain;

/// Hard cap on an incomplete (unterminated) SSE event.
///
/// Sized to the largest single event a legitimate provider can emit, not to the
/// smallest buffer that works. A "fake streaming" upstream — one that generates
/// the whole completion and then emits it as a single SSE event, common among
/// OpenAI-compatible aggregators and self-hosted shims — produces roughly four
/// bytes per output token, so today's 128K-output-token models reach ~512 KB in
/// one event. 1 MiB covers every shipping model with headroom and matches
/// `ai::usage::MAX_NON_STREAMING_USAGE_BYTES`, which already accepts the same
/// payload in its non-streaming form.
///
/// Raw logical bytes and visible retained capacity are each bounded by this
/// value. Slice handles, lines, ranges, and metadata have independent lower
/// limits. Shared or slack-bearing input is normalized only when an incomplete
/// suffix must survive its callback; complete events remain borrowed. At 256
/// suffix handles one block coalesces once into a capped arena.
///
/// Anthropic translation and its output-character accounting now share this
/// one event pipeline; translated output is not decoded again.
pub const DEFAULT_MAX_PENDING_EVENT_BYTES: usize = 1024 * 1024;

const UTF8_BOM: &[u8; 3] = b"\xef\xbb\xbf";

const MAX_EVENT_RANGES: usize = 4096;
const MAX_EVENT_LINES: usize = 4096;
const MAX_EVENT_METADATA_BYTES: usize = 128 * 1024;

/// Compatibility-owned event. New synchronous consumers should use
/// [`SseEventRef`] to avoid materializing data they do not inspect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The latest `event:` field in the block, if present.
    pub event: Option<String>,
    /// Concatenated `data:` fields, joined with newlines.
    pub data: String,
    /// The latest `id:` field in the block, if present.
    pub id: Option<String>,
}

/// Incremental SSE decoder with bounded per-event state.
pub struct SseDecoder {
    bom_prefix: [u8; UTF8_BOM.len()],
    bom_prefix_len: usize,
    bom_checked: bool,
    max_pending_bytes: usize,
    degraded: bool,
    malformed: bool,
    malformed_events: u64,
    block: BlockBuilder,
    skip_lf_after_cr: bool,
    data_chars: Option<DataCharObserver>,
    pending_data_chars: u64,
}

#[derive(Debug, Default)]
struct BlockBuilder {
    raw: SliceChain,
    line_start: usize,
    lines: Vec<ByteRange>,
    data_lines: usize,
    has_event: bool,
    has_id: bool,
    poisoned: bool,
}

#[derive(Clone, Copy)]
enum ParsedFieldKind {
    Data,
    Event,
    Id,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseBlockStatus {
    Data,
    Dataless,
    Poisoned,
}

pub enum SseRecordRef<'a> {
    Preamble(&'a [u8]),
    Block {
        status: SseBlockStatus,
        event: SseEventRef<'a>,
    },
    FramingContinuation(&'a [u8]),
    EosTail(RawSseBytesRef<'a>),
    Opaque(RawSseBytesRef<'a>),
}

pub struct RawSseBytesRef<'a> {
    chain: &'a SliceChain,
}

impl RawSseBytesRef<'_> {
    pub fn len(&self) -> usize {
        self.chain.len()
    }

    pub fn for_each_slice(&self, visitor: impl FnMut(&[u8])) {
        let _ = self.chain.for_each_range(
            ByteRange {
                start: 0,
                end: self.chain.len(),
            },
            visitor,
        );
    }

    pub fn to_bytes(&self) -> Bytes {
        match self.chain.range(ByteRange {
            start: 0,
            end: self.chain.len(),
        }) {
            Some(Cow::Borrowed(bytes)) => Bytes::copy_from_slice(bytes),
            Some(Cow::Owned(bytes)) => Bytes::from(bytes),
            None => Bytes::new(),
        }
    }

    /// Clone the original source handles in order. A block coalesced under
    /// handle pressure returns its one bounded compact slice instead.
    pub fn to_slices(&self) -> Vec<Bytes> {
        self.chain.raw_slices()
    }
}

pub struct SseEventRef<'a> {
    raw: &'a SliceChain,
    line_count: usize,
    event: Option<ByteRange>,
    id: Option<ByteRange>,
    data: Vec<ByteRange>,
}

impl<'a> SseEventRef<'a> {
    pub fn raw(&self) -> RawSseBytesRef<'a> {
        RawSseBytesRef { chain: self.raw }
    }

    pub fn event_range(&self) -> Option<ByteRange> {
        self.event
    }

    pub fn id_range(&self) -> Option<ByteRange> {
        self.id
    }

    pub fn data_ranges(&self) -> &[ByteRange] {
        &self.data
    }

    /// Map a range in the semantic `data:` view back to the original raw SSE
    /// record. A token crossing the synthetic newline inserted between two
    /// `data:` fields is deliberately rejected.
    pub fn semantic_range_to_raw(&self, semantic: ByteRange) -> Result<ByteRange, ()> {
        if semantic.start > semantic.end {
            return Err(());
        }
        let mut offset = 0usize;
        for (index, raw) in self.data.iter().copied().enumerate() {
            let end = offset.checked_add(raw.len()).ok_or(())?;
            if semantic.start >= offset && semantic.end <= end {
                return Ok(ByteRange::new(
                    raw.start.checked_add(semantic.start - offset).ok_or(())?,
                    raw.start.checked_add(semantic.end - offset).ok_or(())?,
                ));
            }
            offset = end;
            if index + 1 < self.data.len() {
                offset = offset.checked_add(1).ok_or(())?;
            }
        }
        Err(())
    }

    pub fn line_count(&self) -> usize {
        self.line_count
    }

    pub fn event(&self) -> Option<Cow<'a, str>> {
        self.event.and_then(|range| range_str(self.raw, range).ok())
    }

    pub fn id(&self) -> Option<Cow<'a, str>> {
        self.id.and_then(|range| range_str(self.raw, range).ok())
    }

    pub fn data_is_literal(&self, literal: &[u8]) -> bool {
        self.data.len() == 1
            && self
                .raw
                .range(self.data[0])
                .is_some_and(|value| value.as_ref() == literal)
    }

    pub fn semantic_data(&self) -> Result<Cow<'a, str>, ()> {
        if self.data.len() == 1 {
            return range_str(self.raw, self.data[0]);
        }
        let length = self
            .data
            .iter()
            .try_fold(self.data.len().saturating_sub(1), |total, range| {
                total.checked_add(range.len())
            })
            .ok_or(())?;
        let mut joined = Vec::with_capacity(length);
        for (index, range) in self.data.iter().copied().enumerate() {
            if index != 0 {
                joined.push(b'\n');
            }
            self.raw
                .for_each_range(range, |part| joined.extend_from_slice(part))
                .ok_or(())?;
        }
        String::from_utf8(joined).map(Cow::Owned).map_err(|_| ())
    }

    pub fn deserialize_data<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_reader(SemanticDataReader::new(self.raw, &self.data))
    }

    pub fn to_owned(&self) -> Result<SseEvent, ()> {
        Ok(SseEvent {
            event: self.event().map(Cow::into_owned),
            data: self.semantic_data()?.into_owned(),
            id: self.id().map(Cow::into_owned),
        })
    }
}

struct SemanticDataReader<'a> {
    chain: &'a SliceChain,
    ranges: &'a [ByteRange],
    index: usize,
    consumed: usize,
    newline_pending: bool,
}

impl<'a> SemanticDataReader<'a> {
    fn new(chain: &'a SliceChain, ranges: &'a [ByteRange]) -> Self {
        Self {
            chain,
            ranges,
            index: 0,
            consumed: 0,
            newline_pending: false,
        }
    }
}

impl Read for SemanticDataReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            if self.newline_pending {
                output[0] = b'\n';
                self.newline_pending = false;
                return Ok(1);
            }
            let Some(range) = self.ranges.get(self.index).copied() else {
                return Ok(0);
            };
            let read = self
                .chain
                .read_range(range, self.consumed, output)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid SSE data range"))?;
            self.consumed += read;
            if self.consumed == range.len() {
                self.index += 1;
                self.consumed = 0;
                self.newline_pending = self.index < self.ranges.len();
            }
            if read != 0 {
                return Ok(read);
            }
        }
    }
}

fn range_str<'a>(chain: &'a SliceChain, range: ByteRange) -> Result<Cow<'a, str>, ()> {
    match chain.range(range).ok_or(())? {
        Cow::Borrowed(bytes) => std::str::from_utf8(bytes).map(Cow::Borrowed).map_err(|_| ()),
        Cow::Owned(bytes) => String::from_utf8(bytes).map(Cow::Owned).map_err(|_| ()),
    }
}

impl SseDecoder {
    /// Create a decoder with a caller-selected incomplete-event byte limit.
    pub fn new(max_pending_bytes: usize) -> Self {
        Self {
            bom_prefix: [0; UTF8_BOM.len()],
            bom_prefix_len: 0,
            bom_checked: false,
            max_pending_bytes,
            degraded: false,
            malformed: false,
            malformed_events: 0,
            block: BlockBuilder::default(),
            skip_lf_after_cr: false,
            data_chars: None,
            pending_data_chars: 0,
        }
    }

    /// Create a decoder with the standard [`DEFAULT_MAX_PENDING_EVENT_BYTES`]
    /// incomplete-event limit.
    pub fn with_default_limit() -> Self {
        Self::new(DEFAULT_MAX_PENDING_EVENT_BYTES)
    }

    /// Enable incremental character accounting for semantic `data:` values.
    ///
    /// This is opt-in so semantic consumers such as Anthropic translation do
    /// not pay for an unused second pass over provider bytes. Once enabled,
    /// the observer is driven by this decoder's line boundaries rather than
    /// owning another SSE framing state machine.
    pub fn enable_data_char_counting(&mut self) {
        if self.data_chars.is_none() {
            self.data_chars = Some(DataCharObserver::default());
        }
    }

    /// Whether oversized input permanently disabled parsing for this stream.
    pub fn degraded(&self) -> bool {
        self.degraded
    }

    /// Whether any field line contained invalid UTF-8.
    ///
    /// Observational consumers may keep the decoder's tolerant behavior, while
    /// protocol translators can use this sticky signal to fail closed.
    pub fn malformed(&self) -> bool {
        self.malformed
    }

    /// Number of completed events that contained invalid UTF-8 field lines.
    ///
    /// The count is cumulative for the decoder lifetime and saturates on
    /// hostile input. Multiple invalid lines in one event count once.
    pub fn malformed_events(&self) -> u64 {
        self.malformed_events
    }

    /// Whether no incomplete event bytes or fields remain buffered.
    pub fn is_idle(&self) -> bool {
        self.bom_prefix_len == 0 && self.block.raw.is_empty() && self.block.lines.is_empty()
    }

    /// Observe one raw transport chunk and append complete events to `out`.
    ///
    /// Decoder-owned incomplete-event state remains bounded, but `out` grows
    /// with every complete event. Inspection-only consumers should use
    /// [`Self::feed_each`] to avoid retaining a whole chunk's event batch.
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<SseEvent>) {
        self.feed_each(chunk, |event| out.push(event));
    }

    /// Observe one raw transport chunk and visit complete events one at a time.
    ///
    /// Unlike [`Self::feed`], this API does not require the caller to retain all
    /// events dispatched by one transport chunk. Consumers that only inspect an
    /// event should prefer this form so their additional memory remains bounded
    /// by one event, even when one chunk contains many small events.
    pub fn feed_each(&mut self, chunk: &[u8], mut visitor: impl FnMut(SseEvent)) {
        self.feed_each_bytes(Bytes::copy_from_slice(chunk), |event| {
            if let Ok(event) = event.to_owned() {
                visitor(event);
            }
        });
    }

    pub fn feed_each_bytes(&mut self, chunk: Bytes, mut visitor: impl FnMut(SseEventRef<'_>)) {
        self.feed_records(chunk, |record| {
            if let SseRecordRef::Block {
                status: SseBlockStatus::Data,
                event,
            } = record
            {
                visitor(event);
            }
        });
    }

    pub fn feed_records(&mut self, chunk: Bytes, mut visitor: impl FnMut(SseRecordRef<'_>)) {
        if self.degraded {
            if let Some(counter) = &mut self.data_chars {
                self.pending_data_chars = self.pending_data_chars.saturating_add(counter.feed_opaque(&chunk));
            }
            let mut raw = SliceChain::default();
            let _ = raw.push(chunk, usize::MAX);
            visitor(SseRecordRef::Opaque(RawSseBytesRef { chain: &raw }));
            return;
        }

        let mut chunk_index = 0;
        while !self.bom_checked && chunk_index < chunk.len() {
            self.bom_prefix[self.bom_prefix_len] = chunk[chunk_index];
            self.bom_prefix_len = self.bom_prefix_len.saturating_add(1);
            chunk_index = chunk_index.saturating_add(1);

            let prefix = &self.bom_prefix[..self.bom_prefix_len];
            if !UTF8_BOM.starts_with(prefix) {
                let replay = self.bom_prefix;
                let replay_len = self.bom_prefix_len;
                self.bom_prefix_len = 0;
                self.bom_checked = true;
                self.process_bytes(Bytes::copy_from_slice(&replay[..replay_len]), &mut visitor);
                if self.degraded {
                    if chunk_index < chunk.len() {
                        let remainder = chunk.slice(chunk_index..);
                        if let Some(counter) = &mut self.data_chars {
                            self.pending_data_chars =
                                self.pending_data_chars.saturating_add(counter.feed_opaque(&remainder));
                        }
                        let mut raw = SliceChain::default();
                        let _ = raw.push(remainder, usize::MAX);
                        visitor(SseRecordRef::Opaque(RawSseBytesRef { chain: &raw }));
                    }
                    return;
                }
            } else if self.bom_prefix_len == UTF8_BOM.len() {
                visitor(SseRecordRef::Preamble(UTF8_BOM));
                self.bom_prefix_len = 0;
                self.bom_checked = true;
            }
        }

        // A one- or two-byte prefix may still become a BOM in a later chunk.
        if !self.bom_checked {
            return;
        }

        if chunk_index < chunk.len() {
            self.process_bytes(chunk.slice(chunk_index..), &mut visitor);
        }
        if !self.degraded && self.block.raw.normalize_retained(self.max_pending_bytes).is_err() {
            self.degrade(&mut visitor, &[]);
        }
    }

    fn process_bytes(&mut self, bytes: Bytes, visitor: &mut impl FnMut(SseRecordRef<'_>)) {
        let mut index = 0;
        if self.skip_lf_after_cr {
            self.skip_lf_after_cr = false;
            if bytes.first() == Some(&b'\n') {
                if self.block.raw.is_empty() {
                    visitor(SseRecordRef::FramingContinuation(&bytes[..1]));
                } else if self.append(bytes.slice(..1)).is_err() {
                    self.degrade(visitor, &bytes);
                    return;
                } else {
                    self.block.line_start = self.block.line_start.saturating_add(1);
                }
                index = 1;
            }
        }

        while index < bytes.len() {
            if self.skip_lf_after_cr {
                self.skip_lf_after_cr = false;
                if bytes[index] == b'\n' {
                    if self.block.raw.is_empty() {
                        visitor(SseRecordRef::FramingContinuation(&bytes[index..=index]));
                    } else if self.append(bytes.slice(index..=index)).is_err() {
                        self.degrade(visitor, &bytes[index..]);
                        return;
                    } else {
                        self.block.line_start = self.block.line_start.saturating_add(1);
                    }
                    index += 1;
                    continue;
                }
            }
            let relative = bytes[index..].iter().position(|byte| matches!(byte, b'\r' | b'\n'));
            let Some(relative) = relative else {
                if self.append(bytes.slice(index..)).is_err() {
                    if let Some(counter) = &mut self.data_chars {
                        self.pending_data_chars = self
                            .pending_data_chars
                            .saturating_add(counter.feed_opaque(&bytes[index..]));
                    }
                    self.degrade(visitor, &bytes[index..]);
                } else if let Some(counter) = &mut self.data_chars {
                    self.pending_data_chars = self
                        .pending_data_chars
                        .saturating_add(counter.feed_segment(&bytes[index..]));
                }
                return;
            };
            let delimiter = index + relative;
            if self.append(bytes.slice(index..=delimiter)).is_err() {
                if let Some(counter) = &mut self.data_chars {
                    self.pending_data_chars = self
                        .pending_data_chars
                        .saturating_add(counter.feed_opaque(&bytes[index..]));
                }
                self.degrade(visitor, &bytes[index..]);
                return;
            }
            let is_cr = bytes[delimiter] == b'\r';
            if let Some(counter) = &mut self.data_chars {
                let count = counter
                    .feed_segment(&bytes[index..delimiter])
                    .saturating_add(counter.finish_line(is_cr));
                self.pending_data_chars = self.pending_data_chars.saturating_add(count);
            }
            if self.finish_line(visitor).is_err() {
                self.degrade(visitor, &bytes[delimiter + 1..]);
                return;
            }
            self.skip_lf_after_cr = is_cr;
            index = delimiter + 1;
        }
    }

    fn append(&mut self, bytes: Bytes) -> Result<(), ()> {
        self.block.raw.push(bytes, self.max_pending_bytes)
    }

    fn finish_line(&mut self, visitor: &mut impl FnMut(SseRecordRef<'_>)) -> Result<(), ()> {
        let terminator_end = self.block.raw.len();
        let line_end = terminator_end.saturating_sub(1);
        if line_end == self.block.line_start {
            self.dispatch(visitor);
            return Ok(());
        }
        if self.block.line_start > line_end {
            return Err(());
        }
        let range = ByteRange::new(self.block.line_start, line_end);
        let next_lines = self.block.lines.len().saturating_add(1);
        let field = parsed_field_kind(&self.block.raw, range);
        let next_data_lines = self
            .block
            .data_lines
            .saturating_add(usize::from(matches!(field, ParsedFieldKind::Data)));
        let next_has_event = self.block.has_event || matches!(field, ParsedFieldKind::Event);
        let next_has_id = self.block.has_id || matches!(field, ParsedFieldKind::Id);
        let next_ranges = next_lines
            .saturating_add(next_data_lines)
            .saturating_add(usize::from(next_has_event))
            .saturating_add(usize::from(next_has_id));
        let next_line_capacity = prospective_vec_capacity(self.block.lines.capacity(), next_lines);
        let metadata = decoder_metadata_charge(
            self.block.raw.slice_metadata_capacity(),
            next_line_capacity,
            next_data_lines,
        );
        if next_lines > MAX_EVENT_LINES || next_ranges > MAX_EVENT_RANGES || metadata > MAX_EVENT_METADATA_BYTES {
            return Err(());
        }
        if next_line_capacity != self.block.lines.capacity() {
            let mut lines = Vec::with_capacity(next_line_capacity);
            if decoder_metadata_charge(
                self.block.raw.slice_metadata_capacity(),
                lines.capacity(),
                next_data_lines,
            ) > MAX_EVENT_METADATA_BYTES
            {
                return Err(());
            }
            lines.extend_from_slice(&self.block.lines);
            self.block.lines = lines;
        }
        self.block.lines.push(range);
        self.block.data_lines = next_data_lines;
        self.block.has_event = next_has_event;
        self.block.has_id = next_has_id;
        if self
            .block
            .raw
            .range(range)
            .is_some_and(|line| line.first() != Some(&b':') && std::str::from_utf8(line.as_ref()).is_err())
        {
            self.block.poisoned = true;
            self.malformed = true;
        }
        self.block.line_start = terminator_end;
        Ok(())
    }

    fn dispatch(&mut self, visitor: &mut impl FnMut(SseRecordRef<'_>)) {
        let block = std::mem::take(&mut self.block);
        let parsed = parse_block(&block);
        if parsed.poisoned {
            self.malformed = true;
            self.malformed_events = self.malformed_events.saturating_add(1);
        }
        let status = if parsed.poisoned {
            SseBlockStatus::Poisoned
        } else if parsed.data.is_empty() {
            SseBlockStatus::Dataless
        } else {
            SseBlockStatus::Data
        };
        visitor(SseRecordRef::Block {
            status,
            event: SseEventRef {
                raw: &block.raw,
                line_count: block.lines.len(),
                event: parsed.event,
                id: parsed.id,
                data: parsed.data,
            },
        });
    }

    pub fn finish_records(&mut self, mut visitor: impl FnMut(SseRecordRef<'_>)) {
        if self.bom_prefix_len != 0 {
            let mut prefix = SliceChain::default();
            let _ = prefix.push(
                Bytes::copy_from_slice(&self.bom_prefix[..self.bom_prefix_len]),
                self.max_pending_bytes,
            );
            visitor(SseRecordRef::EosTail(RawSseBytesRef { chain: &prefix }));
            self.bom_prefix_len = 0;
            self.bom_checked = true;
        }
        if !self.block.raw.is_empty() {
            visitor(SseRecordRef::EosTail(RawSseBytesRef { chain: &self.block.raw }));
        }
        self.block = BlockBuilder::default();
    }

    pub fn take_data_chars(&mut self) -> u64 {
        std::mem::take(&mut self.pending_data_chars)
    }

    pub fn finish_data_chars(&mut self) -> u64 {
        if let Some(counter) = &mut self.data_chars {
            self.pending_data_chars = self.pending_data_chars.saturating_add(counter.finish());
        }
        self.take_data_chars()
    }

    fn degrade(&mut self, visitor: &mut impl FnMut(SseRecordRef<'_>), remainder: &[u8]) {
        self.degraded = true;
        if !self.block.raw.is_empty() {
            visitor(SseRecordRef::Opaque(RawSseBytesRef { chain: &self.block.raw }));
        }
        if !remainder.is_empty() {
            let mut raw = SliceChain::default();
            let _ = raw.push(Bytes::copy_from_slice(remainder), usize::MAX);
            visitor(SseRecordRef::Opaque(RawSseBytesRef { chain: &raw }));
        }
        self.block = BlockBuilder::default();
        self.skip_lf_after_cr = false;
    }
}

#[derive(Default)]
struct DataCharObserver {
    utf8: SseUtf8Counter,
    state: DataLineState,
    matched: usize,
    event_has_data: bool,
    line_started: bool,
    skip_lf: bool,
}

#[derive(Default)]
enum DataLineState {
    #[default]
    Matching,
    NameComplete,
    ValueStart,
    Value,
    Ignore,
}

impl DataCharObserver {
    fn feed_segment(&mut self, bytes: &[u8]) -> u64 {
        let mut count = 0_u64;
        for byte in bytes {
            count = count.saturating_add(self.feed_line_byte(*byte));
        }
        count
    }

    /// Continue accounting after semantic degradation. Framing is scanned
    /// here only because the main parser deliberately stopped; normal parsing
    /// never performs this second scan.
    fn feed_opaque(&mut self, bytes: &[u8]) -> u64 {
        let mut count = 0_u64;
        let mut start = 0;
        for (index, byte) in bytes.iter().copied().enumerate() {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    start = index + 1;
                    continue;
                }
            }
            if matches!(byte, b'\r' | b'\n') {
                count = count.saturating_add(self.feed_segment(&bytes[start..index]));
                count = count.saturating_add(self.finish_line(byte == b'\r'));
                start = index + 1;
            }
        }
        count.saturating_add(self.feed_segment(&bytes[start..]))
    }

    fn feed_line_byte(&mut self, byte: u8) -> u64 {
        const NAME: &[u8] = b"data";
        match self.state {
            DataLineState::Matching if byte == NAME[self.matched] => {
                self.matched += 1;
                if self.matched == NAME.len() {
                    self.state = DataLineState::NameComplete;
                }
                0
            }
            DataLineState::Matching => {
                self.state = DataLineState::Ignore;
                0
            }
            DataLineState::NameComplete if byte == b':' => {
                self.state = DataLineState::ValueStart;
                0
            }
            DataLineState::NameComplete => {
                self.state = DataLineState::Ignore;
                0
            }
            DataLineState::ValueStart => {
                self.state = DataLineState::Value;
                if byte == b' ' {
                    0
                } else {
                    self.feed_value(byte)
                }
            }
            DataLineState::Value => self.feed_value(byte),
            DataLineState::Ignore => 0,
        }
    }

    fn feed_value(&mut self, byte: u8) -> u64 {
        self.start_line().saturating_add(self.utf8.feed_byte(byte))
    }

    fn start_line(&mut self) -> u64 {
        if self.line_started {
            return 0;
        }
        self.line_started = true;
        if self.event_has_data {
            self.utf8.feed_byte(b'\n')
        } else {
            0
        }
    }

    fn finish_line(&mut self, ended_with_cr: bool) -> u64 {
        let blank = matches!(self.state, DataLineState::Matching) && self.matched == 0;
        let data = matches!(
            self.state,
            DataLineState::NameComplete | DataLineState::ValueStart | DataLineState::Value
        );
        let mut count = 0;
        if data {
            count = self.start_line().saturating_add(self.utf8.finish());
            self.event_has_data = true;
        }
        if blank {
            self.event_has_data = false;
        }
        self.state = DataLineState::Matching;
        self.matched = 0;
        self.line_started = false;
        self.skip_lf = ended_with_cr;
        count
    }

    fn finish(&mut self) -> u64 {
        self.utf8.finish()
    }
}

#[derive(Default)]
struct SseUtf8Counter {
    pending: [u8; 4],
    len: usize,
    expected: usize,
}

impl SseUtf8Counter {
    fn feed_byte(&mut self, byte: u8) -> u64 {
        if self.len == 0 {
            if byte.is_ascii() {
                return 1;
            }
            self.expected = match byte {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return 1,
            };
            self.pending[0] = byte;
            self.len = 1;
            return 1;
        }
        if byte & 0xc0 != 0x80 {
            let invalid = u64::try_from(self.len.saturating_sub(1)).unwrap_or(u64::MAX);
            self.len = 0;
            self.expected = 0;
            return invalid.saturating_add(self.feed_byte(byte));
        }
        self.pending[self.len] = byte;
        self.len += 1;
        if self.len != self.expected {
            return 0;
        }
        let len = self.len;
        self.len = 0;
        self.expected = 0;
        if std::str::from_utf8(&self.pending[..len]).is_ok() {
            0
        } else {
            u64::try_from(len.saturating_sub(1)).unwrap_or(u64::MAX)
        }
    }

    fn finish(&mut self) -> u64 {
        self.len = 0;
        self.expected = 0;
        0
    }
}

struct ParsedBlock {
    event: Option<ByteRange>,
    id: Option<ByteRange>,
    data: Vec<ByteRange>,
    poisoned: bool,
}

fn parse_block(block: &BlockBuilder) -> ParsedBlock {
    let mut parsed = ParsedBlock {
        event: None,
        id: None,
        data: Vec::with_capacity(block.data_lines),
        poisoned: block.poisoned,
    };
    for range in &block.lines {
        let Some(line) = block.raw.range(*range) else {
            parsed.poisoned = true;
            continue;
        };
        if line.first() == Some(&b':') {
            continue;
        }
        let Ok(line) = std::str::from_utf8(line.as_ref()) else {
            parsed.poisoned = true;
            continue;
        };
        let colon = line.as_bytes().iter().position(|byte| *byte == b':');
        let (field, value_offset) = match colon {
            Some(index) => (
                index,
                index + 1 + usize::from(line.as_bytes().get(index + 1) == Some(&b' ')),
            ),
            None => (line.len(), line.len()),
        };
        let value = ByteRange {
            start: range.start + value_offset,
            end: range.end,
        };
        match &line[..field] {
            "data" => parsed.data.push(value),
            "event" => parsed.event = Some(value),
            "id" if !line.as_bytes()[value_offset..].contains(&0) => parsed.id = Some(value),
            _ => {}
        }
    }
    parsed
}

fn prospective_vec_capacity(current: usize, required: usize) -> usize {
    if required <= current {
        current
    } else {
        current.max(4).saturating_mul(2).max(required)
    }
}

fn decoder_metadata_charge(slice_bytes: usize, line_capacity: usize, data_capacity: usize) -> usize {
    slice_bytes
        .saturating_add(line_capacity.saturating_mul(std::mem::size_of::<ByteRange>()))
        .saturating_add(data_capacity.saturating_mul(std::mem::size_of::<ByteRange>()))
}

fn parsed_field_kind(raw: &SliceChain, range: ByteRange) -> ParsedFieldKind {
    let Some(line) = raw.range(range) else {
        return ParsedFieldKind::Other;
    };
    if line.first() == Some(&b':') {
        return ParsedFieldKind::Other;
    }
    let Ok(line) = std::str::from_utf8(line.as_ref()) else {
        return ParsedFieldKind::Other;
    };
    let field_end = line
        .as_bytes()
        .iter()
        .position(|byte| *byte == b':')
        .unwrap_or(line.len());
    match &line[..field_end] {
        "data" => ParsedFieldKind::Data,
        "event" => ParsedFieldKind::Event,
        "id" => ParsedFieldKind::Id,
        _ => ParsedFieldKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn decode_all(chunks: &[&[u8]]) -> (Vec<SseEvent>, bool) {
        let mut decoder = SseDecoder::with_default_limit();
        let mut events = Vec::new();
        for chunk in chunks {
            decoder.feed(chunk, &mut events);
        }
        (events, decoder.degraded())
    }

    #[test]
    fn single_event_single_chunk() {
        let (events, degraded) = decode_all(&[b"data: hello\n\n"]);
        assert!(!degraded);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
        assert_eq!(events[0].event, None);
    }

    #[test]
    fn event_split_across_chunks_mid_line_is_reassembled() {
        let (events, _) = decode_all(&[b"data: {\"a\"", b":1}\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "{\"a\":1}");
    }

    #[test]
    fn leading_utf8_bom_is_stripped() {
        let (events, degraded) = decode_all(&[b"\xef\xbb\xbfdata: hello\n\n"]);
        assert!(!degraded);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn leading_utf8_bom_split_across_chunks_is_stripped() {
        let (events, degraded) = decode_all(&[b"\xef", b"\xbb", b"\xbfdata: hello\n\n"]);
        assert!(!degraded);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn later_utf8_bom_is_ordinary_data() {
        let (events, degraded) = decode_all(&[b"data: first\n\ndata: \xef\xbb\xbfsecond\n\n"]);
        assert!(!degraded);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].data.as_bytes(), b"\xef\xbb\xbfsecond");
    }

    #[test]
    fn non_bom_prefix_is_replayed_through_the_event_limit() {
        let mut decoder = SseDecoder::new(2);
        let mut events = Vec::new();

        decoder.feed(b"\xef\xbb", &mut events);
        assert!(!decoder.degraded());
        decoder.feed(b"x", &mut events);

        assert!(decoder.degraded());
        assert!(events.is_empty());
    }

    #[test]
    fn single_chunk_bom_replay_degradation_preserves_ordered_remainder() {
        let input = Bytes::from_static(b"\xef\xbbxremaining");
        let mut decoder = SseDecoder::new(2);
        let mut raw = Vec::new();

        collect_raw(&mut decoder, input.clone(), &mut raw);

        assert!(decoder.degraded());
        assert_eq!(raw, input);
    }

    #[test]
    fn cross_chunk_bom_replay_degradation_preserves_ordered_remainder() {
        let mut decoder = SseDecoder::new(2);
        let mut raw = Vec::new();

        collect_raw(&mut decoder, Bytes::from_static(b"\xef\xbb"), &mut raw);
        collect_raw(&mut decoder, Bytes::from_static(b"xremaining"), &mut raw);

        assert!(decoder.degraded());
        assert_eq!(raw, b"\xef\xbbxremaining");
    }

    #[test]
    fn crlf_line_endings_and_split_between_cr_and_lf() {
        let (events, _) = decode_all(&[b"data: a\r", b"\n\r\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "a");
    }

    #[test]
    fn multiple_events_in_one_chunk() {
        let (events, _) = decode_all(&[b"data: one\n\ndata: two\n\n"]);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "one");
        assert_eq!(events[1].data, "two");
    }

    #[test]
    fn feed_each_visits_many_events_without_a_batch() {
        let mut decoder = SseDecoder::with_default_limit();
        let chunk = "data: x\n\n".repeat(10_000);
        let mut count = 0;
        decoder.feed_each(chunk.as_bytes(), |event| {
            assert_eq!(event.data, "x");
            count += 1;
        });
        assert_eq!(count, 10_000);
        assert!(!decoder.degraded());
    }

    #[test]
    fn event_type_id_and_multiline_data() {
        let (events, _) = decode_all(&[b"event: delta\nid: 7\ndata: l1\ndata: l2\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("delta"));
        assert_eq!(events[0].id.as_deref(), Some("7"));
        assert_eq!(events[0].data, "l1\nl2");
    }

    #[test]
    fn comment_lines_and_dataless_blocks_dispatch_nothing() {
        let (events, _) = decode_all(&[b": keepalive\n\nevent: noop\n\n"]);
        assert!(events.is_empty());
    }

    #[test]
    fn empty_data_fields_dispatch_per_the_eventsource_algorithm() {
        let (events, degraded) = decode_all(&[
            b"data:\n\n",
            b"data: \n\n",
            b"data\n\n",
            b"data:\ndata:\n\n",
            b"event: ping\n\n",
            b"data:\n",
        ]);

        assert!(!degraded);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].data, "");
        assert_eq!(events[1].data, "");
        assert_eq!(events[2].data, "");
        assert_eq!(events[3].data, "\n");
    }

    #[test]
    fn done_sentinel_is_an_ordinary_data_event() {
        let (events, _) = decode_all(&[b"data: [DONE]\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "[DONE]");
    }

    #[test]
    fn space_after_colon_is_optional() {
        let (events, _) = decode_all(&[b"data:tight\n\n"]);
        assert_eq!(events[0].data, "tight");
    }

    #[test]
    fn oversized_partial_line_degrades_permanently() {
        let mut decoder = SseDecoder::new(1024);
        let mut events = Vec::new();
        decoder.feed(&vec![b'x'; 2048], &mut events);
        assert!(decoder.degraded());
        assert!(events.is_empty());

        decoder.feed(b"data: late\n\n", &mut events);
        assert!(events.is_empty());
        assert!(decoder.degraded());
    }

    #[test]
    fn many_complete_lines_without_dispatch_are_bounded() {
        let mut decoder = SseDecoder::new(128);
        let mut events = Vec::new();

        for _ in 0..32 {
            decoder.feed(b"data: x\n", &mut events);
            assert!(decoder.degraded() || decoder.block.raw.len() <= decoder.max_pending_bytes);
        }

        assert!(decoder.degraded());
        assert!(events.is_empty());
        assert_eq!(decoder.block.raw.len(), 0);
    }

    #[test]
    fn crlf_bytes_count_toward_the_whole_event_limit() {
        let mut decoder = SseDecoder::new(8);
        let mut events = Vec::new();

        decoder.feed(b"data:x\r", &mut events);
        assert!(!decoder.degraded());
        assert_eq!(decoder.block.raw.len(), 7);

        decoder.feed(b"\n", &mut events);
        assert!(!decoder.degraded());
        assert_eq!(decoder.block.raw.len(), 8);

        decoder.feed(b"x", &mut events);
        assert!(decoder.degraded());
    }

    #[test]
    fn invalid_utf8_in_a_field_line_is_skipped_not_fatal() {
        let mut decoder = SseDecoder::with_default_limit();
        let mut events = Vec::new();
        decoder.feed(b"data: \xff\xfe\n\ndata: ok\n\n", &mut events);
        let degraded = decoder.degraded();
        assert!(!degraded);
        assert!(decoder.malformed());
        assert_eq!(decoder.malformed_events(), 1);
        assert!(decoder.is_idle());
        assert_eq!(events.len(), 1, "the malformed event is dropped, the stream continues");
        assert_eq!(events[0].data, "ok");
    }

    #[test]
    fn multiple_invalid_lines_poison_one_completed_event_once() {
        let mut decoder = SseDecoder::with_default_limit();
        let mut events = Vec::new();

        decoder.feed(b"data: \xff\ndata: \xfe\nevent: \xff\n\n", &mut events);

        assert!(events.is_empty());
        assert!(decoder.malformed());
        assert_eq!(decoder.malformed_events(), 1);
    }

    #[test]
    fn two_poisoned_events_in_one_chunk_are_counted_separately() {
        let mut decoder = SseDecoder::with_default_limit();
        let mut events = Vec::new();

        decoder.feed(b"data: \xff\n\ndata: \xfe\n\n", &mut events);

        assert!(events.is_empty());
        assert_eq!(decoder.malformed_events(), 2);
    }

    #[test]
    fn split_invalid_line_counts_only_when_its_event_completes() {
        let mut decoder = SseDecoder::with_default_limit();
        let mut events = Vec::new();

        decoder.feed(b"data: \xc3", &mut events);
        assert_eq!(decoder.malformed_events(), 0);
        decoder.feed(b"\x28\n", &mut events);
        assert!(decoder.malformed());
        assert_eq!(decoder.malformed_events(), 0);
        decoder.feed(b"\n", &mut events);

        assert!(events.is_empty());
        assert_eq!(decoder.malformed_events(), 1);
    }

    #[test]
    fn valid_event_does_not_reset_cumulative_malformed_count() {
        let mut decoder = SseDecoder::with_default_limit();
        let mut events = Vec::new();

        decoder.feed(b"data: \xff\n\ndata: valid\n\ndata: \xfe\n\n", &mut events);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "valid");
        assert!(decoder.malformed());
        assert_eq!(decoder.malformed_events(), 2);
    }

    #[test]
    fn idle_reports_incomplete_event_state() {
        let mut decoder = SseDecoder::with_default_limit();
        let mut events = Vec::new();
        decoder.feed(b"data: partial", &mut events);
        assert!(!decoder.is_idle());
        decoder.feed(b"\n\n", &mut events);
        assert!(decoder.is_idle());
    }

    fn collect_raw(decoder: &mut SseDecoder, chunk: Bytes, output: &mut Vec<u8>) {
        decoder.feed_records(chunk, |record| match record {
            SseRecordRef::Preamble(bytes) | SseRecordRef::FramingContinuation(bytes) => output.extend_from_slice(bytes),
            SseRecordRef::Block { event, .. } => event.raw().for_each_slice(|bytes| output.extend_from_slice(bytes)),
            SseRecordRef::EosTail(raw) | SseRecordRef::Opaque(raw) => {
                raw.for_each_slice(|bytes| output.extend_from_slice(bytes))
            }
        });
    }

    #[test]
    fn raw_records_preserve_every_byte_for_all_two_and_three_way_splits() {
        let input = b"\xef\xbb\xbf: keep\r\ndata: h\xc3\xa9\r\nunknown: x\r\n\r\ndata: \xff\n\ntail";
        for first in 0..=input.len() {
            for second in first..=input.len() {
                let mut decoder = SseDecoder::with_default_limit();
                let mut raw = Vec::new();
                for part in [&input[..first], &input[first..second], &input[second..]] {
                    collect_raw(&mut decoder, Bytes::copy_from_slice(part), &mut raw);
                }
                decoder.finish_records(|record| match record {
                    SseRecordRef::EosTail(bytes) | SseRecordRef::Opaque(bytes) => {
                        bytes.for_each_slice(|part| raw.extend_from_slice(part))
                    }
                    _ => {}
                });
                assert_eq!(raw, input, "first={first}, second={second}");
            }
        }
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_and_chunking_preserve_raw_identity(
            input in prop::collection::vec(any::<u8>(), 0..4096),
            chunk_sizes in prop::collection::vec(1usize..128, 1..64),
        ) {
            let mut decoder = SseDecoder::with_default_limit();
            let mut raw = Vec::new();
            let mut offset = 0;
            let mut sizes = chunk_sizes.into_iter().cycle();
            while offset < input.len() {
                let end = offset.saturating_add(sizes.next().unwrap()).min(input.len());
                collect_raw(&mut decoder, Bytes::copy_from_slice(&input[offset..end]), &mut raw);
                offset = end;
            }
            decoder.finish_records(|record| match record {
                SseRecordRef::EosTail(bytes) | SseRecordRef::Opaque(bytes) => {
                    bytes.for_each_slice(|part| raw.extend_from_slice(part));
                }
                _ => {}
            });
            prop_assert_eq!(raw, input);
        }
    }

    #[test]
    fn one_byte_incomplete_event_coalesces_once_and_stays_bounded() {
        let mut decoder = SseDecoder::with_default_limit();
        for byte in std::iter::repeat_n(b'x', 4096) {
            decoder.feed_each_bytes(Bytes::from(vec![byte]), |_| {});
        }
        assert_eq!(decoder.block.raw.coalesce_count(), 1);
        assert_eq!(decoder.block.raw.handle_count(), 1);
        assert!(decoder.block.raw.resident_capacity() <= DEFAULT_MAX_PENDING_EVENT_BYTES);
        assert!(decoder.block.raw.copied_bytes() <= decoder.block.raw.len());
    }

    #[test]
    fn data_character_counting_is_opt_in_and_uses_parser_boundaries() {
        let mut unused = SseDecoder::with_default_limit();
        unused.feed_each_bytes(Bytes::from_static(b"data: ignored\n\n"), |_| {});
        assert_eq!(unused.take_data_chars(), 0);

        let mut decoder = SseDecoder::with_default_limit();
        decoder.enable_data_char_counting();
        decoder.feed_each_bytes(Bytes::from_static(b"data: h\xc3"), |_| {});
        assert_eq!(
            decoder.take_data_chars(),
            2,
            "h and the leading UTF-8 byte are observed incrementally"
        );
        decoder.feed_each_bytes(Bytes::from_static(b"\xa9\r"), |_| {});
        decoder.feed_each_bytes(Bytes::from_static(b"\ndata: x\n\n"), |_| {});
        assert_eq!(
            decoder.take_data_chars(),
            2,
            "the continuation completes the prior scalar; the second data line adds a semantic newline and x"
        );
    }

    #[test]
    fn metadata_charge_accepts_exact_limit_and_rejects_limit_plus_one() {
        let range_size = std::mem::size_of::<ByteRange>();
        let line_capacity = MAX_EVENT_METADATA_BYTES / range_size;
        assert_eq!(decoder_metadata_charge(0, line_capacity, 0), MAX_EVENT_METADATA_BYTES);
        assert!(decoder_metadata_charge(1, line_capacity, 0) > MAX_EVENT_METADATA_BYTES);
    }

    #[test]
    fn metadata_accounts_for_handles_lines_and_parsed_data_ranges() {
        let mut decoder = SseDecoder::with_default_limit();
        for _ in 0..MAX_EVENT_LINES {
            if decoder.degraded() {
                break;
            }
            decoder.feed_each_bytes(Bytes::from_static(b"data: x\n"), |_| {});
            if !decoder.degraded() {
                let charge = decoder_metadata_charge(
                    decoder.block.raw.slice_metadata_capacity(),
                    decoder.block.lines.capacity(),
                    decoder.block.data_lines,
                );
                assert!(charge <= MAX_EVENT_METADATA_BYTES);
                assert!(
                    decoder.block.lines.len()
                        + decoder.block.data_lines
                        + usize::from(decoder.block.has_event)
                        + usize::from(decoder.block.has_id)
                        <= MAX_EVENT_RANGES
                );
            }
        }
        assert!(
            decoder.degraded(),
            "combined metadata/range pressure must degrade before it can exceed a cap"
        );
    }
}
