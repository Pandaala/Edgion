//! Bounded, incremental Server-Sent Events (SSE) decoder.
//!
//! This is shared framing infrastructure for response-stream observers. Feed
//! raw transport chunks and receive complete events; callers remain
//! responsible for forwarding the original bytes unchanged. The decoder
//! permanently stops parsing if one incomplete event exceeds its configured
//! limit.

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
/// The cost is worst-case memory per in-flight decoder, and it is only ever
/// reached by a pathological stream: a normal event ends within a few hundred
/// bytes and resets this accounting to zero. Sizing that worst case
/// deliberately, because it grew 16x with this value:
///
/// - The cap bounds `retained_event_bytes + pending.len()`, but the resident
///   allocation is `pending` plus `cur_data` carrying amortized-doubling
///   slack, so budget ~1-2 MiB per decoder rather than exactly 1 MiB.
/// - An Anthropic request runs **two** decoders (the translator's, fed the raw
///   provider stream, and `UsageExtractionProcessor`'s, fed the translated
///   output — see `ai::dispatch`), so that path doubles again.
/// - There is no aggregate bound across concurrent streams; the ceiling is
///   per-request and multiplies by AI stream concurrency.
///
/// Reaching it requires a hostile or broken **provider** (operator-configured),
/// not an arbitrary client, which is why a per-request ceiling is accepted here
/// instead of a global budget.
pub const DEFAULT_MAX_PENDING_EVENT_BYTES: usize = 1024 * 1024;

const UTF8_BOM: &[u8; 3] = b"\xef\xbb\xbf";

/// One dispatched SSE event.
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
    /// At most two undecided bytes matching the start of a UTF-8 BOM. The
    /// third byte either completes the one permitted leading BOM or causes
    /// the whole prefix to be replayed through normal bounded parsing.
    bom_prefix: [u8; UTF8_BOM.len()],
    bom_prefix_len: usize,
    bom_checked: bool,
    /// Bytes in the current incomplete field line.
    pending: Vec<u8>,
    max_pending_bytes: usize,
    degraded: bool,
    malformed: bool,
    cur_event: Option<String>,
    cur_id: Option<String>,
    cur_data: String,
    cur_has_data: bool,
    cur_poisoned: bool,
    /// Raw bytes consumed for complete lines in the current event. Together
    /// with `pending`, this accounts for the whole unterminated event rather
    /// than only the last partial line.
    retained_event_bytes: usize,
    /// A CR terminates its line immediately; ignore an LF that follows it,
    /// including when the pair is split across transport chunks.
    skip_lf_after_cr: bool,
}

impl SseDecoder {
    /// Create a decoder with a caller-selected incomplete-event byte limit.
    pub fn new(max_pending_bytes: usize) -> Self {
        Self {
            bom_prefix: [0; UTF8_BOM.len()],
            bom_prefix_len: 0,
            bom_checked: false,
            pending: Vec::new(),
            max_pending_bytes,
            degraded: false,
            malformed: false,
            cur_event: None,
            cur_id: None,
            cur_data: String::new(),
            cur_has_data: false,
            cur_poisoned: false,
            retained_event_bytes: 0,
            skip_lf_after_cr: false,
        }
    }

    /// Create a decoder with the standard [`DEFAULT_MAX_PENDING_EVENT_BYTES`]
    /// incomplete-event limit.
    pub fn with_default_limit() -> Self {
        Self::new(DEFAULT_MAX_PENDING_EVENT_BYTES)
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

    /// Whether no incomplete event bytes or fields remain buffered.
    pub fn is_idle(&self) -> bool {
        self.bom_prefix_len == 0
            && self.pending.is_empty()
            && self.cur_event.is_none()
            && self.cur_id.is_none()
            && self.cur_data.is_empty()
            && !self.cur_has_data
            && self.retained_event_bytes == 0
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
        self.feed_each_dyn(chunk, &mut visitor);
    }

    fn feed_each_dyn(&mut self, chunk: &[u8], visitor: &mut dyn FnMut(SseEvent)) {
        if self.degraded {
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
                for &byte in &replay[..replay_len] {
                    if !self.process_byte(byte, visitor) {
                        return;
                    }
                }
            } else if self.bom_prefix_len == UTF8_BOM.len() {
                // WHATWG: strip one UTF-8 BOM, and only at stream start.
                self.bom_prefix_len = 0;
                self.bom_checked = true;
            }
        }

        // A one- or two-byte prefix may still become a BOM in a later chunk.
        if !self.bom_checked {
            return;
        }

        for &byte in &chunk[chunk_index..] {
            if !self.process_byte(byte, visitor) {
                return;
            }
        }
    }

    fn process_byte(&mut self, byte: u8, visitor: &mut dyn FnMut(SseEvent)) -> bool {
        if self.skip_lf_after_cr {
            self.skip_lf_after_cr = false;
            if byte == b'\n' {
                // The preceding CR already ended the line, but the LF is
                // still part of the retained incomplete event's framing. A
                // CR blank line dispatched and reset the event, so in that
                // case there is nothing left to account for.
                if self.retained_event_bytes != 0 {
                    if self.would_exceed_limit(1) {
                        self.degrade();
                        return false;
                    }
                    self.retained_event_bytes = self.retained_event_bytes.saturating_add(1);
                }
                return true;
            }
        }

        match byte {
            b'\r' => {
                if !self.finish_line(visitor) {
                    return false;
                }
                self.skip_lf_after_cr = true;
            }
            b'\n' => {
                if !self.finish_line(visitor) {
                    return false;
                }
            }
            _ => {
                if self.would_exceed_limit(1) {
                    self.degrade();
                    return false;
                }
                self.pending.push(byte);
            }
        }
        true
    }

    fn would_exceed_limit(&self, additional_bytes: usize) -> bool {
        self.retained_event_bytes
            .checked_add(self.pending.len())
            .and_then(|size| size.checked_add(additional_bytes))
            .map(|size| size > self.max_pending_bytes)
            .unwrap_or(true)
    }

    /// Finish the current line. Returns false when accounting overflowed and
    /// the decoder degraded.
    fn finish_line(&mut self, visitor: &mut dyn FnMut(SseEvent)) -> bool {
        if self.pending.is_empty() {
            self.dispatch(visitor);
            return true;
        }

        // Count the line terminator as part of the incomplete event. For
        // CRLF, this counts CR now and LF in `feed` when it arrives.
        if self.would_exceed_limit(1) {
            self.degrade();
            return false;
        }
        let line_bytes = self.pending.len().saturating_add(1);
        self.retained_event_bytes = self.retained_event_bytes.saturating_add(line_bytes);

        let line = std::mem::take(&mut self.pending);
        self.consume_line(&line);
        true
    }

    fn consume_line(&mut self, line: &[u8]) {
        if line.first() == Some(&b':') {
            return;
        }
        let Ok(line) = std::str::from_utf8(line) else {
            self.cur_poisoned = true;
            self.malformed = true;
            return;
        };
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };

        match field {
            "data" => {
                if self.cur_has_data {
                    self.cur_data.push('\n');
                }
                self.cur_data.push_str(value);
                self.cur_has_data = true;
            }
            "event" => self.cur_event = Some(value.to_owned()),
            "id" if !value.contains('\0') => self.cur_id = Some(value.to_owned()),
            _ => {}
        }
    }

    fn dispatch(&mut self, visitor: &mut dyn FnMut(SseEvent)) {
        if self.cur_has_data && !self.cur_poisoned {
            visitor(SseEvent {
                event: self.cur_event.take(),
                data: std::mem::take(&mut self.cur_data),
                id: self.cur_id.take(),
            });
        }
        self.reset_event();
    }

    fn reset_event(&mut self) {
        self.pending.clear();
        self.cur_event = None;
        self.cur_id = None;
        self.cur_data.clear();
        self.cur_has_data = false;
        self.cur_poisoned = false;
        self.retained_event_bytes = 0;
    }

    fn degrade(&mut self) {
        self.degraded = true;
        self.bom_prefix = [0; UTF8_BOM.len()];
        self.bom_prefix_len = 0;
        self.bom_checked = true;
        self.pending = Vec::new();
        self.cur_event = None;
        self.cur_id = None;
        self.cur_data = String::new();
        self.cur_has_data = false;
        self.cur_poisoned = false;
        self.retained_event_bytes = 0;
        self.skip_lf_after_cr = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(
                decoder.degraded()
                    || decoder.retained_event_bytes.saturating_add(decoder.pending.len()) <= decoder.max_pending_bytes
            );
        }

        assert!(decoder.degraded());
        assert!(events.is_empty());
        assert_eq!(decoder.retained_event_bytes, 0);
    }

    #[test]
    fn crlf_bytes_count_toward_the_whole_event_limit() {
        let mut decoder = SseDecoder::new(8);
        let mut events = Vec::new();

        decoder.feed(b"data:x\r", &mut events);
        assert!(!decoder.degraded());
        assert_eq!(decoder.retained_event_bytes, 7);

        decoder.feed(b"\n", &mut events);
        assert!(!decoder.degraded());
        assert_eq!(decoder.retained_event_bytes, 8);

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
        assert!(decoder.is_idle());
        assert_eq!(events.len(), 1, "the malformed event is dropped, the stream continues");
        assert_eq!(events[0].data, "ok");
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
}
