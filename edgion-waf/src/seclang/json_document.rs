//! Bounded reduction of provisional JSON tokens into ordered, payload-free
//! publication metadata. This is not a variable store or a rule executor.
//! Objects replace complete duplicate subtrees and sort decoded keys; arrays
//! retain index order and publish their length after their children.

use std::cmp::Ordering;
use std::fmt;
use std::io::Write;
use std::mem::size_of;

use thiserror::Error;

use super::{
    JsonStream, JsonStreamError, JsonStreamInput, JsonStreamLimits, JsonStreamSinkError, JsonStreamToken,
    JsonStreamUsage,
};

/// Independent reducer limits. These are additional source-state budgets, not
/// substitutes for complete-body publication limits. No runtime defaults are
/// selected here. Tokenizer limits are supplied separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonDocumentLimits {
    /// Sum of allocated Vec capacities, including keys and event/result slots.
    /// Excludes fixed structs, allocator overhead/transient reallocation memory,
    /// the separately limited tokenizer, and caller-owned callback state.
    pub max_retained_bytes: usize,
    /// Live decoded member-key bytes in open objects (not allocated capacity).
    pub max_key_bytes: usize,
    /// Live member slots in all open objects, including a pending child.
    pub max_members: usize,
    /// Live accounting events, including provisional and surviving subtrees.
    pub max_events: usize,
    /// Current external path bytes, including the root name `json`.
    pub max_path_bytes: usize,
    /// Root value depth is zero. Independent of the tokenizer depth limit.
    pub max_depth: usize,
    /// Token dispatch, key comparisons, byte copies/scans, slot shifts, growth
    /// copies, sequence links, and discarded-event walks. Excludes callback work,
    /// which the caller must limit separately, and terminal deallocation.
    pub max_work: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonDocumentLimit {
    RetainedBytes,
    KeyBytes,
    Members,
    Events,
    PathBytes,
    Depth,
    Work,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum JsonDocumentError {
    #[error("JSON document tokenizer failed: {0}")]
    Lexical(JsonStreamError),
    #[error("JSON document exceeds {kind:?} limit {limit}")]
    Limit { kind: JsonDocumentLimit, limit: usize },
    #[error("cannot allocate bounded JSON document state")]
    Allocation,
    #[error("JSON document accounting arithmetic overflowed")]
    Arithmetic,
    #[error("JSON document consumer rejected a provisional scalar")]
    SinkRejected,
    #[error("JSON document state is inconsistent")]
    State,
    #[error("JSON document number normalization failed")]
    Number,
    #[error("JSON document is already complete")]
    AlreadyComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonDocumentSinkError;

/// All events describe surviving values in complete-body traversal order.
/// A scalar stands for both ARGS and ARGS_NAMES publication. `result` is an
/// application-supplied fixed-size fact, never formatted by this module.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JsonDocumentEvent<S: Copy> {
    /// Check the resulting path length before visiting the child node.
    ExtendPath { path_bytes: usize },
    /// Check depth, then charge path bytes as body traversal work.
    Visit { path_bytes: usize, depth: usize },
    Scalar {
        path_bytes: usize,
        value_bytes: usize,
        result: S,
    },
}

impl<S: Copy> fmt::Debug for JsonDocumentEvent<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExtendPath { path_bytes } => f.debug_struct("ExtendPath").field("path_bytes", path_bytes).finish(),
            Self::Visit { path_bytes, depth } => f
                .debug_struct("Visit")
                .field("path_bytes", path_bytes)
                .field("depth", depth)
                .finish(),
            Self::Scalar {
                path_bytes,
                value_bytes,
                ..
            } => f
                .debug_struct("Scalar")
                .field("path_bytes", path_bytes)
                .field("value_bytes", value_bytes)
                .finish_non_exhaustive(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JsonDocumentUsage {
    pub retained_bytes: usize,
    pub peak_retained_bytes: usize,
    pub key_bytes: usize,
    pub peak_key_bytes: usize,
    pub members: usize,
    pub peak_members: usize,
    pub events: usize,
    pub peak_events: usize,
    pub work: usize,
}

#[derive(Clone, Copy, Default)]
struct Sequence {
    head: Option<usize>,
    tail: Option<usize>,
}

// Free slots reuse `next`; no separately allocated freelist is needed.
struct Slot<S: Copy> {
    event: Option<JsonDocumentEvent<S>>,
    next: Option<usize>,
}

struct Member {
    key: Vec<u8>,
    sequence: Sequence,
}

enum Container {
    Object {
        members: Vec<Member>,
        pending: Option<usize>,
    },
    Array {
        length: usize,
    },
}

struct Frame {
    container: Container,
    sequence: Sequence,
    previous_path: usize,
}

struct Budget {
    limits: JsonDocumentLimits,
    usage: JsonDocumentUsage,
}

impl Budget {
    fn check(actual: usize, limit: usize, kind: JsonDocumentLimit) -> Result<(), JsonDocumentError> {
        if actual > limit {
            Err(JsonDocumentError::Limit { kind, limit })
        } else {
            Ok(())
        }
    }

    fn work(&mut self, amount: usize) -> Result<(), JsonDocumentError> {
        let next = add(self.usage.work, amount)?;
        Self::check(next, self.limits.max_work, JsonDocumentLimit::Work)?;
        self.usage.work = next;
        Ok(())
    }

    fn reserve<T>(&mut self, values: &mut Vec<T>, additional: usize) -> Result<(), JsonDocumentError> {
        let needed = add(values.len(), additional)?;
        if needed <= values.capacity() {
            return Ok(());
        }
        let old_bytes = mul(values.capacity(), size_of::<T>())?;
        let base = self
            .usage
            .retained_bytes
            .checked_sub(old_bytes)
            .ok_or(JsonDocumentError::State)?;
        // Amortized growth for event/stack/path appends. A tight allocation
        // budget may admit a smaller expansion, but never less than `needed`.
        let available = self.limits.max_retained_bytes.saturating_sub(base) / size_of::<T>().max(1);
        let capacity = needed.max(values.capacity().saturating_mul(2).min(available));
        let requested = mul(capacity, size_of::<T>())?;
        Self::check(
            add(base, requested)?,
            self.limits.max_retained_bytes,
            JsonDocumentLimit::RetainedBytes,
        )?;
        self.work(mul(values.len(), size_of::<T>())?)?;
        values
            .try_reserve_exact(capacity - values.len())
            .map_err(|_| JsonDocumentError::Allocation)?;
        self.usage.retained_bytes = add(base, mul(values.capacity(), size_of::<T>())?)?;
        self.usage.peak_retained_bytes = self.usage.peak_retained_bytes.max(self.usage.retained_bytes);
        Self::check(
            self.usage.retained_bytes,
            self.limits.max_retained_bytes,
            JsonDocumentLimit::RetainedBytes,
        )
    }

    fn release<T>(&mut self, values: Vec<T>) {
        // Every capacity was admitted by reserve; this subtraction cannot fail.
        self.usage.retained_bytes -= values.capacity() * size_of::<T>();
    }
}

fn add(left: usize, right: usize) -> Result<usize, JsonDocumentError> {
    left.checked_add(right).ok_or(JsonDocumentError::Arithmetic)
}

fn mul(left: usize, right: usize) -> Result<usize, JsonDocumentError> {
    left.checked_mul(right).ok_or(JsonDocumentError::Arithmetic)
}

struct Reducer<S: Copy> {
    budget: Budget,
    path: Vec<u8>,
    stack: Vec<Frame>,
    arena: Vec<Slot<S>>,
    free: Option<usize>,
    root: Sequence,
    root_started: bool,
}

impl<S: Copy> Reducer<S> {
    fn event(&mut self, event: JsonDocumentEvent<S>) -> Result<Sequence, JsonDocumentError> {
        self.budget.work(size_of::<Slot<S>>())?;
        let live = add(self.budget.usage.events, 1)?;
        Budget::check(live, self.budget.limits.max_events, JsonDocumentLimit::Events)?;
        let index = if let Some(index) = self.free {
            self.free = self.arena[index].next;
            self.arena[index] = Slot {
                event: Some(event),
                next: None,
            };
            index
        } else {
            self.budget.reserve(&mut self.arena, 1)?;
            let index = self.arena.len();
            self.arena.push(Slot {
                event: Some(event),
                next: None,
            });
            index
        };
        self.budget.usage.events = live;
        self.budget.usage.peak_events = self.budget.usage.peak_events.max(live);
        Ok(Sequence {
            head: Some(index),
            tail: Some(index),
        })
    }

    fn link(&mut self, mut left: Sequence, right: Sequence) -> Result<Sequence, JsonDocumentError> {
        self.budget.work(1)?;
        if let Some(tail) = left.tail {
            self.arena[tail].next = right.head;
        } else {
            left.head = right.head;
        }
        if right.tail.is_some() {
            left.tail = right.tail;
        }
        Ok(left)
    }

    fn discard(&mut self, sequence: Sequence) -> Result<(), JsonDocumentError> {
        let mut next = sequence.head;
        while let Some(index) = next {
            self.budget.work(1)?;
            next = self.arena[index].next;
            self.arena[index].event = None;
            self.arena[index].next = self.free;
            self.free = Some(index);
            self.budget.usage.events -= 1;
        }
        Ok(())
    }

    fn key(&mut self, key: &str) -> Result<(), JsonDocumentError> {
        let frame = self.stack.last_mut().ok_or(JsonDocumentError::State)?;
        let Container::Object { members, pending } = &mut frame.container else {
            return Err(JsonDocumentError::State);
        };
        if pending.is_some() {
            return Err(JsonDocumentError::State);
        }
        let mut lo = 0;
        let mut hi = members.len();
        let mut found = None;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            // An upper bound on the lexicographic comparison, including empty keys.
            self.budget.work(add(key.len().min(members[mid].key.len()), 1)?)?;
            match members[mid].key.as_slice().cmp(key.as_bytes()) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => {
                    found = Some(mid);
                    break;
                }
            }
        }
        if let Some(index) = found {
            let old = std::mem::take(&mut members[index].sequence);
            *pending = Some(index);
            self.discard(old)?;
        } else {
            let count = add(self.budget.usage.members, 1)?;
            Budget::check(count, self.budget.limits.max_members, JsonDocumentLimit::Members)?;
            let bytes = add(self.budget.usage.key_bytes, key.len())?;
            Budget::check(bytes, self.budget.limits.max_key_bytes, JsonDocumentLimit::KeyBytes)?;
            self.budget
                .work(add(key.len(), mul(members.len() - lo, size_of::<Member>())?)?)?;
            self.budget.reserve(members, 1)?;
            let mut owned = Vec::new();
            self.budget.reserve(&mut owned, key.len())?;
            owned.extend_from_slice(key.as_bytes());
            members.insert(
                lo,
                Member {
                    key: owned,
                    sequence: Sequence::default(),
                },
            );
            *pending = Some(lo);
            self.budget.usage.members = count;
            self.budget.usage.peak_members = self.budget.usage.peak_members.max(count);
            self.budget.usage.key_bytes = bytes;
            self.budget.usage.peak_key_bytes = self.budget.usage.peak_key_bytes.max(bytes);
        }
        Ok(())
    }

    fn begin(&mut self) -> Result<(Sequence, usize), JsonDocumentError> {
        let depth = self.stack.len();
        Budget::check(depth, self.budget.limits.max_depth, JsonDocumentLimit::Depth)?;
        let previous = self.path.len();
        let mut sequence = Sequence::default();
        if let Some(parent) = self.stack.last() {
            let mut index = SmallText::new();
            let component = match &parent.container {
                Container::Object { members, pending } => &members[pending.ok_or(JsonDocumentError::State)?].key[..],
                Container::Array { length } => {
                    write!(&mut index, "{length}").map_err(|_| JsonDocumentError::Arithmetic)?;
                    index.as_bytes()
                }
            };
            let extra = add(1, component.len())?;
            let length = add(previous, extra)?;
            Budget::check(length, self.budget.limits.max_path_bytes, JsonDocumentLimit::PathBytes)?;
            self.budget.work(extra)?;
            self.budget.reserve(&mut self.path, extra)?;
            self.path.push(b'.');
            self.path.extend_from_slice(component);
            sequence = self.event(JsonDocumentEvent::ExtendPath { path_bytes: length })?;
        } else {
            if self.root_started {
                return Err(JsonDocumentError::State);
            }
            Budget::check(4, self.budget.limits.max_path_bytes, JsonDocumentLimit::PathBytes)?;
            self.budget.work(4)?;
            self.budget.reserve(&mut self.path, 4)?;
            self.path.extend_from_slice(b"json");
            self.root_started = true;
        }
        let visit = self.event(JsonDocumentEvent::Visit {
            path_bytes: self.path.len(),
            depth,
        })?;
        Ok((self.link(sequence, visit)?, previous))
    }

    fn scalar<F>(&mut self, value: &[u8], sink: &mut F) -> Result<Sequence, JsonDocumentError>
    where
        F: FnMut(&str, &[u8]) -> Result<S, JsonDocumentSinkError>,
    {
        self.budget.work(add(self.path.len(), value.len())?)?;
        let path = std::str::from_utf8(&self.path).map_err(|_| JsonDocumentError::State)?;
        let result = sink(path, value).map_err(|_| JsonDocumentError::SinkRejected)?;
        self.event(JsonDocumentEvent::Scalar {
            path_bytes: self.path.len(),
            value_bytes: value.len(),
            result,
        })
    }

    fn finish_value(&mut self, sequence: Sequence, previous: usize) -> Result<(), JsonDocumentError> {
        self.path.truncate(previous);
        let Some(parent) = self.stack.last_mut() else {
            self.root = sequence;
            return Ok(());
        };
        match &mut parent.container {
            Container::Object { members, pending } => {
                let index = pending.take().ok_or(JsonDocumentError::State)?;
                members[index].sequence = sequence;
            }
            Container::Array { length } => {
                *length = add(*length, 1)?;
                let left = parent.sequence;
                let combined = self.link(left, sequence)?;
                self.stack.last_mut().ok_or(JsonDocumentError::State)?.sequence = combined;
            }
        }
        Ok(())
    }

    fn token<F>(&mut self, token: JsonStreamToken<'_>, sink: &mut F) -> Result<(), JsonDocumentError>
    where
        F: FnMut(&str, &[u8]) -> Result<S, JsonDocumentSinkError>,
    {
        self.budget.work(1)?;
        match token {
            JsonStreamToken::Key(key) => self.key(key),
            JsonStreamToken::ObjectEnd | JsonStreamToken::ArrayEnd => {
                let frame = self.stack.pop().ok_or(JsonDocumentError::State)?;
                let mut sequence = frame.sequence;
                match frame.container {
                    Container::Object { members, pending: None } if matches!(token, JsonStreamToken::ObjectEnd) => {
                        for member in &members {
                            sequence = self.link(sequence, member.sequence)?;
                        }
                        self.budget.usage.members -= members.len();
                        // Drain keeps the vector allocation accounted until release.
                        let mut members = members;
                        for member in members.drain(..) {
                            self.budget.usage.key_bytes -= member.key.len();
                            self.budget.release(member.key);
                        }
                        self.budget.release(members);
                    }
                    Container::Array { length } if matches!(token, JsonStreamToken::ArrayEnd) => {
                        let mut value = SmallText::new();
                        write!(&mut value, "{length}").map_err(|_| JsonDocumentError::Arithmetic)?;
                        let scalar = self.scalar(value.as_bytes(), sink)?;
                        sequence = self.link(sequence, scalar)?;
                    }
                    _ => return Err(JsonDocumentError::State),
                }
                self.finish_value(sequence, frame.previous_path)
            }
            JsonStreamToken::ObjectStart | JsonStreamToken::ArrayStart => {
                let (sequence, previous_path) = self.begin()?;
                self.budget.reserve(&mut self.stack, 1)?;
                self.stack.push(Frame {
                    container: if matches!(token, JsonStreamToken::ObjectStart) {
                        Container::Object {
                            members: Vec::new(),
                            pending: None,
                        }
                    } else {
                        Container::Array { length: 0 }
                    },
                    sequence,
                    previous_path,
                });
                Ok(())
            }
            scalar => {
                let (sequence, previous) = self.begin()?;
                let mut normalized = SmallText::new();
                let bytes = match scalar {
                    JsonStreamToken::String(value) => value.as_bytes(),
                    JsonStreamToken::Boolean(true) => b"true",
                    JsonStreamToken::Boolean(false) => b"false",
                    JsonStreamToken::Null => b"",
                    JsonStreamToken::Number(raw) => {
                        self.budget.work(raw.len())?;
                        let number: serde_json::Number =
                            serde_json::from_str(raw).map_err(|_| JsonDocumentError::Number)?;
                        serde_json::to_writer(&mut normalized, &number).map_err(|_| JsonDocumentError::Number)?;
                        normalized.as_bytes()
                    }
                    _ => return Err(JsonDocumentError::State),
                };
                let scalar = self.scalar(bytes, sink)?;
                let sequence = self.link(sequence, scalar)?;
                self.finish_value(sequence, previous)
            }
        }
    }

    fn complete(&mut self) -> Result<(), JsonDocumentError> {
        if !self.stack.is_empty() || !self.root_started || self.root.head.is_none() {
            return Err(JsonDocumentError::State);
        }
        self.budget.release(std::mem::take(&mut self.path));
        self.budget.release(std::mem::take(&mut self.stack));
        Ok(())
    }

    fn release(&mut self) {
        self.path = Vec::new();
        self.stack = Vec::new();
        self.arena = Vec::new();
        self.free = None;
        self.root = Sequence::default();
        self.budget.usage.retained_bytes = 0;
        self.budget.usage.key_bytes = 0;
        self.budget.usage.members = 0;
        self.budget.usage.events = 0;
    }
}

// serde_json's ordinary Number profile renders finite f64/i64/u64 without
// heap storage. A fixed output writer also bounds formatting if that changes.
struct SmallText {
    bytes: [u8; 64],
    len: usize,
}

impl SmallText {
    fn new() -> Self {
        Self { bytes: [0; 64], len: 0 }
    }
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for SmallText {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        let end = self
            .len
            .checked_add(input.len())
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
        self.bytes[self.len..end].copy_from_slice(input);
        self.len = end;
        Ok(input.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum Terminal {
    Active,
    Complete,
    Failed(JsonDocumentError),
}

/// The consumer computes provisional fixed-size facts from borrowed scalar
/// bytes. Store deferred predicate failures inside `S`; returning a sink error
/// terminates the document even if a later duplicate would replace that value.
/// Do not commit decisions or retain scalar payloads in `S` or callback state.
/// Callback work and any caller-owned memory require their own bounds.
///
/// On successful `Complete`, only the event arena remains owned. `events()`
/// exposes its reference-order sequence; dropping this object releases it.
/// Failure/abandonment release all owned allocations and are sticky. Calls
/// after completion return `AlreadyComplete` without discarding final events.
pub struct JsonDocument<S: Copy> {
    lexical: JsonStream,
    reducer: Reducer<S>,
    terminal: Terminal,
}

impl<S: Copy> fmt::Debug for JsonDocument<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonDocument")
            .field("usage", &self.usage())
            .field("lexical_usage", &self.lexical_usage())
            .field("terminal", &self.terminal)
            .finish()
    }
}

impl<S: Copy> JsonDocument<S> {
    pub fn new(lexical: JsonStreamLimits, limits: JsonDocumentLimits) -> Self {
        Self {
            lexical: JsonStream::new(lexical),
            reducer: Reducer {
                budget: Budget {
                    limits,
                    usage: JsonDocumentUsage::default(),
                },
                path: Vec::new(),
                stack: Vec::new(),
                arena: Vec::new(),
                free: None,
                root: Sequence::default(),
                root_started: false,
            },
            terminal: Terminal::Active,
        }
    }

    pub(super) fn new_for_body(lexical: JsonStreamLimits, limits: JsonDocumentLimits) -> Self {
        Self {
            lexical: JsonStream::new_for_body(lexical),
            ..Self::new(lexical, limits)
        }
    }

    pub fn usage(&self) -> JsonDocumentUsage {
        self.reducer.budget.usage
    }
    pub fn lexical_usage(&self) -> JsonStreamUsage {
        self.lexical.usage()
    }

    pub fn feed<F>(&mut self, input: JsonStreamInput<'_>, mut sink: F) -> Result<(), JsonDocumentError>
    where
        F: FnMut(&str, &[u8]) -> Result<S, JsonDocumentSinkError>,
    {
        match self.terminal {
            Terminal::Active => {}
            Terminal::Complete => return Err(JsonDocumentError::AlreadyComplete),
            Terminal::Failed(error) => return Err(error),
        }
        let mut reducer_error = None;
        let result = self
            .lexical
            .feed(input, |token| {
                self.reducer.token(token, &mut sink).map_err(|error| {
                    reducer_error = Some(error);
                    JsonStreamSinkError
                })
            })
            .map_err(|error| reducer_error.unwrap_or(JsonDocumentError::Lexical(error)));
        let result = result.and_then(|()| {
            if matches!(input, JsonStreamInput::Complete) {
                self.reducer.complete()?;
                self.terminal = Terminal::Complete;
            }
            Ok(())
        });
        if let Err(error) = result {
            self.terminal = Terminal::Failed(error);
            self.reducer.release();
        }
        result
    }

    /// Iteration does no allocation and yields at most `max_events` events.
    /// Only valid completion makes the surviving sequence observable.
    pub fn events(&self) -> Option<impl Iterator<Item = JsonDocumentEvent<S>> + '_> {
        if !matches!(self.terminal, Terminal::Complete) {
            return None;
        }
        let mut next = self.reducer.root.head;
        Some(std::iter::from_fn(move || {
            let index = next?;
            let slot = &self.reducer.arena[index];
            next = slot.next;
            slot.event
        }))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_document_test.rs"]
mod tests;
