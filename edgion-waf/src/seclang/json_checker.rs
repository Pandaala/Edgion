//! Source-neutral JSON inspection with pooled object identity and no value tape.
#[cfg(test)]
use crate::seclang::measurements::{self, Event};
use std::{cmp::Ordering, fmt, io::Write, mem::size_of};

use super::json_scratch::{growth_target, heap_capacity, GrowthError};
use smallvec::{Array, SmallVec};
use thiserror::Error;

use super::{
    JsonStream, JsonStreamError, JsonStreamInput, JsonStreamLimits, JsonStreamSinkError, JsonStreamToken,
    JsonStreamUsage,
};

/// Independent physical limits. Heap capacity stays charged after reuse; fixed inline storage is excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonStructureLimits {
    pub max_retained_bytes: usize,
    pub max_key_bytes: usize,
    pub max_members: usize,
    pub max_path_bytes: usize,
    /// Root value depth is zero.
    pub max_depth: usize,
    pub max_work: usize,
}

impl Default for JsonStructureLimits {
    fn default() -> Self {
        Self {
            max_retained_bytes: 8 << 20,
            max_key_bytes: 4 << 20,
            max_members: 65_536,
            max_path_bytes: 16 << 10,
            max_depth: 64,
            max_work: 128 << 20,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonStructureLimit {
    RetainedBytes,
    KeyBytes,
    Members,
    PathBytes,
    Depth,
    Work,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum JsonCheckerError {
    #[error("JSON tokenizer failed: {0}")]
    Lexical(JsonStreamError),
    #[error("JSON structure exceeds {kind:?} limit {limit}")]
    Limit { kind: JsonStructureLimit, limit: usize },
    #[error("cannot allocate bounded JSON structure")]
    Allocation,
    #[error("JSON structure arithmetic overflowed")]
    Arithmetic,
    #[error("JSON inspection consumer failed")]
    Sink,
    #[error("JSON structure state is inconsistent")]
    State,
    #[error("JSON inspection is already complete")]
    AlreadyComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonCheckerSinkError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonScalarKind {
    String,
    Number,
    Boolean,
    Null,
    ArrayLength,
}

/// Final surviving semantic totals, excluding HTTP body prelude/final metadata.
/// Logical publication checks belong to the adapter, after valid completion.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JsonSemanticSummary {
    pub argument_count: usize,
    pub argument_name_bytes: usize,
    pub argument_value_bytes: usize,
    pub body_entry_count: usize,
    pub decoded_bytes: usize,
    pub combined_size: usize,
    pub traversal_work: usize,
    pub max_checked_name_bytes: usize,
    pub max_value_depth: usize,
}

impl JsonSemanticSummary {
    fn combine(self, other: Self) -> Result<Self, JsonCheckerError> {
        Ok(Self {
            argument_count: add(self.argument_count, other.argument_count)?,
            argument_name_bytes: add(self.argument_name_bytes, other.argument_name_bytes)?,
            argument_value_bytes: add(self.argument_value_bytes, other.argument_value_bytes)?,
            body_entry_count: add(self.body_entry_count, other.body_entry_count)?,
            decoded_bytes: add(self.decoded_bytes, other.decoded_bytes)?,
            combined_size: add(self.combined_size, other.combined_size)?,
            traversal_work: add(self.traversal_work, other.traversal_work)?,
            max_checked_name_bytes: self.max_checked_name_bytes.max(other.max_checked_name_bytes),
            max_value_depth: self.max_value_depth.max(other.max_value_depth),
        })
    }

    fn scalar(name: usize, value: usize) -> Result<Self, JsonCheckerError> {
        Ok(Self {
            argument_count: 1,
            argument_name_bytes: name,
            argument_value_bytes: value,
            body_entry_count: 2,
            decoded_bytes: add(mul(name, 3)?, value)?,
            combined_size: add(name, value)?,
            max_checked_name_bytes: name,
            ..Self::default()
        })
    }
}

/// Receipts are sink-owned logical handles, never parser slots or borrowed bytes.
/// A pooled sink must generation-check reused IDs. Successful concat/discard
/// consumes its inputs. On any callback error, abandon clears all sink state;
/// no consumed receipt is retried. All callback payload borrows end on return.
pub trait JsonCheckerSink {
    type Receipt: Copy;
    fn scalar(
        &mut self,
        name: &[u8],
        value: &[u8],
        kind: JsonScalarKind,
    ) -> Result<Option<Self::Receipt>, JsonCheckerSinkError>;
    /// Must link receipt sequences in O(1), without copying their contents.
    fn concat(
        &mut self,
        left: Option<Self::Receipt>,
        right: Option<Self::Receipt>,
    ) -> Result<Option<Self::Receipt>, JsonCheckerSinkError>;
    /// Reclaims owned facts; the sink charges its own bounded walk work.
    fn discard(&mut self, receipt: Self::Receipt) -> Result<(), JsonCheckerSinkError>;
    fn seal(
        &mut self,
        receipt: Option<Self::Receipt>,
        summary: JsonSemanticSummary,
    ) -> Result<(), JsonCheckerSinkError>;
    /// Infallible, allocation-free cleanup, independent of exhausted work limits.
    fn abandon(&mut self);
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JsonStructureUsage {
    pub retained_bytes: usize,
    pub peak_retained_bytes: usize,
    pub key_bytes: usize,
    pub peak_key_bytes: usize,
    pub members: usize,
    pub peak_members: usize,
    pub work: usize,
}

#[derive(Clone, Copy)]
struct Subtree<R: Copy> {
    summary: JsonSemanticSummary,
    receipt: Option<R>,
}
impl<R: Copy> Default for Subtree<R> {
    fn default() -> Self {
        Self {
            summary: JsonSemanticSummary::default(),
            receipt: None,
        }
    }
}

#[derive(Clone, Copy)]
struct KeyRange {
    start: usize,
    len: usize,
}
#[derive(Clone, Copy)]
struct Member<R: Copy> {
    key: KeyRange,
    left: Option<usize>,
    right: Option<usize>,
    height: u8,
    value: Subtree<R>,
}
#[derive(Clone, Copy)]
enum Container {
    Object {
        root: Option<usize>,
        pending: Option<usize>,
        key_mark: usize,
        node_mark: usize,
    },
    Array {
        length: usize,
    },
}
#[derive(Clone, Copy)]
struct Frame<R: Copy> {
    container: Container,
    accumulated: Subtree<R>,
    restore: usize,
}

struct Budget {
    limits: JsonStructureLimits,
    usage: JsonStructureUsage,
}
impl Budget {
    fn reserve_bytes<A: Array<Item = u8>>(
        &mut self,
        values: &mut SmallVec<A>,
        additional: usize,
    ) -> Result<(), JsonCheckerError> {
        let remaining = self.limits.max_retained_bytes.saturating_sub(self.usage.retained_bytes);
        let target = growth_target(values, additional, remaining).map_err(|error| match error {
            GrowthError::Arithmetic => JsonCheckerError::Arithmetic,
            GrowthError::Retained => JsonCheckerError::Limit {
                kind: JsonStructureLimit::RetainedBytes,
                limit: self.limits.max_retained_bytes,
            },
        })?;
        let Some(target) = target else {
            return Ok(());
        };
        self.work(values.len())?;
        let previous = heap_capacity(values);
        values
            .try_reserve_exact(target - values.len())
            .map_err(|_| JsonCheckerError::Allocation)?;
        let increase = heap_capacity(values)
            .checked_sub(previous)
            .ok_or(JsonCheckerError::State)?;
        self.usage.retained_bytes = add(self.usage.retained_bytes, increase)?;
        self.usage.peak_retained_bytes = self.usage.peak_retained_bytes.max(self.usage.retained_bytes);
        Self::check(
            self.usage.retained_bytes,
            self.limits.max_retained_bytes,
            JsonStructureLimit::RetainedBytes,
        )
    }
    fn reserve_hint<T>(&mut self, values: &mut Vec<T>, target: usize) -> Result<(), JsonCheckerError> {
        debug_assert!(values.is_empty() && values.capacity() == 0);
        let bytes = mul(target, size_of::<T>())?;
        if bytes > self.limits.max_retained_bytes.saturating_sub(self.usage.retained_bytes) {
            return Ok(());
        }
        // A failed optional reservation leaves the vector unchanged. Required
        // demand growth below still reports its own allocation failure.
        if values.try_reserve_exact(target).is_err() {
            return Ok(());
        }
        self.usage.retained_bytes = add(self.usage.retained_bytes, mul(values.capacity(), size_of::<T>())?)?;
        self.usage.peak_retained_bytes = self.usage.peak_retained_bytes.max(self.usage.retained_bytes);
        Self::check(
            self.usage.retained_bytes,
            self.limits.max_retained_bytes,
            JsonStructureLimit::RetainedBytes,
        )
    }
    fn check(actual: usize, limit: usize, kind: JsonStructureLimit) -> Result<(), JsonCheckerError> {
        if actual > limit {
            Err(JsonCheckerError::Limit { kind, limit })
        } else {
            Ok(())
        }
    }
    fn work(&mut self, amount: usize) -> Result<(), JsonCheckerError> {
        self.usage.work = add(self.usage.work, amount)?;
        Self::check(self.usage.work, self.limits.max_work, JsonStructureLimit::Work)
    }
    fn reserve<T>(&mut self, values: &mut Vec<T>, additional: usize) -> Result<(), JsonCheckerError> {
        let required = add(values.len(), additional)?;
        if required <= values.capacity() {
            return Ok(());
        }
        let unit = size_of::<T>();
        let remaining = self.limits.max_retained_bytes.saturating_sub(self.usage.retained_bytes);
        let maximum = add(values.capacity(), remaining / unit)?;
        Self::check(required, maximum, JsonStructureLimit::RetainedBytes).map_err(|_| JsonCheckerError::Limit {
            kind: JsonStructureLimit::RetainedBytes,
            limit: self.limits.max_retained_bytes,
        })?;
        let target = required.max(values.capacity().saturating_mul(2).max(4)).min(maximum);
        self.work(mul(values.len(), unit)?)?;
        let previous = values.capacity();
        values
            .try_reserve_exact(target - values.len())
            .map_err(|_| JsonCheckerError::Allocation)?;
        let increase = mul(
            values.capacity().checked_sub(previous).ok_or(JsonCheckerError::State)?,
            unit,
        )?;
        self.usage.retained_bytes = add(self.usage.retained_bytes, increase)?;
        self.usage.peak_retained_bytes = self.usage.peak_retained_bytes.max(self.usage.retained_bytes);
        Self::check(
            self.usage.retained_bytes,
            self.limits.max_retained_bytes,
            JsonStructureLimit::RetainedBytes,
        )
    }
}

fn add(a: usize, b: usize) -> Result<usize, JsonCheckerError> {
    a.checked_add(b).ok_or(JsonCheckerError::Arithmetic)
}
fn mul(a: usize, b: usize) -> Result<usize, JsonCheckerError> {
    a.checked_mul(b).ok_or(JsonCheckerError::Arithmetic)
}

#[derive(Default)]
struct CapacityHints {
    nodes: usize,
}

impl CapacityHints {
    fn for_source<R: Copy>(len: usize, root_len: usize, limits: JsonStructureLimits) -> Self {
        fn capacity(maximum: usize) -> Option<usize> {
            if maximum == 0 {
                Some(0)
            } else {
                Some(maximum.checked_mul(2)?.max(4))
            }
        }
        // Cover malformed prefixes as well as complete documents. Overflow only
        // disables the optimization; it does not reject otherwise valid input.
        let upper = (|| {
            let nodes = capacity(limits.max_members.min(len))?.checked_mul(size_of::<Member<R>>())?;
            let keys = capacity(limits.max_key_bytes.min(len))?;
            let frames = capacity(limits.max_depth.checked_add(1)?.min(len.checked_add(1)?))?
                .checked_mul(size_of::<Frame<R>>())?;
            let path = capacity(limits.max_path_bytes.max(root_len))?;
            nodes.checked_add(keys)?.checked_add(frames)?.checked_add(path)
        })();
        if upper.is_none_or(|upper| upper > limits.max_retained_bytes) {
            return Self::default();
        }
        let nodes = (len / 50).clamp(10, 256).next_power_of_two();
        Self {
            nodes: if nodes <= limits.max_members && nodes <= len.saturating_mul(2).max(4) {
                nodes
            } else {
                0
            },
        }
    }
}

struct Reducer<R: Copy> {
    nodes: Vec<Member<R>>,
    hints: CapacityHints,
    keys: SmallVec<[u8; 256]>,
    path: SmallVec<[u8; 256]>,
    frames: Vec<Frame<R>>,
    root: Option<Subtree<R>>,
    empty_root: bool,
    budget: Budget,
}

impl<R: Copy> Reducer<R> {
    fn member(&mut self, id: usize) -> Result<Member<R>, JsonCheckerError> {
        self.budget.work(1)?;
        self.nodes.get(id).copied().ok_or(JsonCheckerError::State)
    }
    fn store(&mut self, id: usize, member: Member<R>) -> Result<(), JsonCheckerError> {
        self.budget.work(1)?;
        *self.nodes.get_mut(id).ok_or(JsonCheckerError::State)? = member;
        Ok(())
    }
    fn height(&mut self, id: Option<usize>) -> Result<u8, JsonCheckerError> {
        id.map_or(Ok(0), |id| Ok(self.member(id)?.height))
    }
    fn refresh(&mut self, id: usize) -> Result<(), JsonCheckerError> {
        let mut node = self.member(id)?;
        node.height = self
            .height(node.left)?
            .max(self.height(node.right)?)
            .checked_add(1)
            .ok_or(JsonCheckerError::Arithmetic)?;
        self.store(id, node)
    }
    fn rotate_left(&mut self, id: usize) -> Result<usize, JsonCheckerError> {
        let mut a = self.member(id)?;
        let child = a.right.ok_or(JsonCheckerError::State)?;
        let mut b = self.member(child)?;
        a.right = b.left;
        b.left = Some(id);
        self.store(id, a)?;
        self.store(child, b)?;
        self.refresh(id)?;
        self.refresh(child)?;
        Ok(child)
    }
    fn rotate_right(&mut self, id: usize) -> Result<usize, JsonCheckerError> {
        let mut a = self.member(id)?;
        let child = a.left.ok_or(JsonCheckerError::State)?;
        let mut b = self.member(child)?;
        a.left = b.right;
        b.right = Some(id);
        self.store(id, a)?;
        self.store(child, b)?;
        self.refresh(id)?;
        self.refresh(child)?;
        Ok(child)
    }
    fn balance(&mut self, id: usize) -> Result<usize, JsonCheckerError> {
        self.refresh(id)?;
        let mut node = self.member(id)?;
        let left = self.height(node.left)?;
        let right = self.height(node.right)?;
        if i16::from(left) - i16::from(right) > 1 {
            let child = node.left.ok_or(JsonCheckerError::State)?;
            let branch = self.member(child)?;
            if self.height(branch.left)? < self.height(branch.right)? {
                node.left = Some(self.rotate_left(child)?);
                self.store(id, node)?;
            }
            self.rotate_right(id)
        } else if i16::from(right) - i16::from(left) > 1 {
            let child = node.right.ok_or(JsonCheckerError::State)?;
            let branch = self.member(child)?;
            if self.height(branch.right)? < self.height(branch.left)? {
                node.right = Some(self.rotate_right(child)?);
                self.store(id, node)?;
            }
            self.rotate_left(id)
        } else {
            Ok(id)
        }
    }
    fn compare(&mut self, key: &[u8], range: KeyRange) -> Result<Ordering, JsonCheckerError> {
        let existing = self
            .keys
            .get(range.start..add(range.start, range.len)?)
            .ok_or(JsonCheckerError::State)?;
        for (&a, &b) in key.iter().zip(existing) {
            self.budget.work(1)?;
            if a != b {
                return Ok(a.cmp(&b));
            }
        }
        self.budget.work(1)?;
        Ok(key.len().cmp(&existing.len()))
    }
    // Recursion follows a balanced index, never the untrusted document depth.
    fn insert(&mut self, root: Option<usize>, key: &[u8]) -> Result<(usize, usize, bool), JsonCheckerError> {
        let Some(id) = root else {
            let members = add(self.budget.usage.members, 1)?;
            Budget::check(members, self.budget.limits.max_members, JsonStructureLimit::Members)?;
            let bytes = add(self.keys.len(), key.len())?;
            Budget::check(bytes, self.budget.limits.max_key_bytes, JsonStructureLimit::KeyBytes)?;
            self.budget.reserve_bytes(&mut self.keys, key.len())?;
            self.budget.work(key.len())?;
            let range = KeyRange {
                start: self.keys.len(),
                len: key.len(),
            };
            self.keys.extend_from_slice(key);
            #[cfg(test)]
            measurements::record(Event::KeyCopyBytes, key.len());
            let member = Member {
                key: range,
                left: None,
                right: None,
                height: 1,
                value: Subtree::default(),
            };
            if self.nodes.capacity() == 0 {
                self.budget.reserve_hint(&mut self.nodes, self.hints.nodes)?;
            }
            self.budget.reserve(&mut self.nodes, 1)?;
            self.budget.work(1)?;
            let id = self.nodes.len();
            self.nodes.push(member);
            self.budget.usage.members = members;
            self.budget.usage.peak_members = self.budget.usage.peak_members.max(members);
            self.budget.usage.key_bytes = self.keys.len();
            self.budget.usage.peak_key_bytes = self.budget.usage.peak_key_bytes.max(self.keys.len());
            return Ok((id, id, true));
        };
        let mut node = self.member(id)?;
        let (selected, fresh) = match self.compare(key, node.key)? {
            Ordering::Equal => return Ok((id, id, false)),
            Ordering::Less => {
                let (child, selected, fresh) = self.insert(node.left, key)?;
                node.left = Some(child);
                (selected, fresh)
            }
            Ordering::Greater => {
                let (child, selected, fresh) = self.insert(node.right, key)?;
                node.right = Some(child);
                (selected, fresh)
            }
        };
        self.store(id, node)?;
        Ok((self.balance(id)?, selected, fresh))
    }
    fn join<S: JsonCheckerSink<Receipt = R>>(
        &mut self,
        left: Subtree<R>,
        right: Subtree<R>,
        sink: &mut S,
    ) -> Result<Subtree<R>, JsonCheckerError> {
        self.budget.work(1)?;
        let summary = left.summary.combine(right.summary)?;
        let receipt = match (left.receipt, right.receipt) {
            (None, receipt) | (receipt, None) => receipt,
            (left, right) => sink.concat(left, right).map_err(|_| JsonCheckerError::Sink)?,
        };
        Ok(Subtree { summary, receipt })
    }
    fn drain<S: JsonCheckerSink<Receipt = R>>(
        &mut self,
        root: Option<usize>,
        sink: &mut S,
    ) -> Result<Subtree<R>, JsonCheckerError> {
        let Some(id) = root else {
            return Ok(Subtree::default());
        };
        let node = self.member(id)?;
        let left = self.drain(node.left, sink)?;
        let combined = self.join(left, node.value, sink)?;
        let right = self.drain(node.right, sink)?;
        let result = self.join(combined, right, sink)?;
        self.budget.work(1)?;
        self.budget.usage.members = self
            .budget
            .usage
            .members
            .checked_sub(1)
            .ok_or(JsonCheckerError::State)?;
        Ok(result)
    }
    fn key<S: JsonCheckerSink<Receipt = R>>(&mut self, key: &[u8], sink: &mut S) -> Result<(), JsonCheckerError> {
        let frame = self.frames.last().copied().ok_or(JsonCheckerError::State)?;
        let Container::Object {
            root,
            pending: None,
            key_mark,
            node_mark,
        } = frame.container
        else {
            return Err(JsonCheckerError::State);
        };
        let (root, id, fresh) = self.insert(root, key)?;
        if !fresh {
            let mut node = self.member(id)?;
            if let Some(receipt) = node.value.receipt {
                sink.discard(receipt).map_err(|_| JsonCheckerError::Sink)?;
            }
            node.value = Subtree::default();
            self.store(id, node)?;
        }
        self.frames.last_mut().ok_or(JsonCheckerError::State)?.container = Container::Object {
            root: Some(root),
            pending: Some(id),
            key_mark,
            node_mark,
        };
        Ok(())
    }
    fn begin(&mut self) -> Result<(usize, Subtree<R>), JsonCheckerError> {
        let depth = self.frames.len();
        Budget::check(depth, self.budget.limits.max_depth, JsonStructureLimit::Depth)?;
        let restore = self.path.len();
        if let Some(frame) = self.frames.last().copied() {
            let mut decimal = SmallText::new();
            let range = match frame.container {
                Container::Object { pending: Some(id), .. } => Some(self.member(id)?.key),
                Container::Array { length } => {
                    write!(&mut decimal, "{length}").map_err(|_| JsonCheckerError::Arithmetic)?;
                    self.budget.work(decimal.len)?;
                    None
                }
                _ => return Err(JsonCheckerError::State),
            };
            let len = range.map_or(decimal.len, |range| range.len);
            let separator = usize::from(!(self.empty_root && depth == 1));
            let extra = add(separator, len)?;
            Budget::check(
                add(self.path.len(), extra)?,
                self.budget.limits.max_path_bytes,
                JsonStructureLimit::PathBytes,
            )?;
            self.budget.reserve_bytes(&mut self.path, extra)?;
            self.budget.work(extra)?;
            if separator != 0 {
                self.path.push(b'.');
            }
            if let Some(range) = range {
                self.path.extend_from_slice(
                    self.keys
                        .get(range.start..add(range.start, range.len)?)
                        .ok_or(JsonCheckerError::State)?,
                );
            } else {
                self.path.extend_from_slice(decimal.bytes());
            }
            #[cfg(test)]
            measurements::record(Event::PathCopyBytes, extra);
        } else if self.root.is_some() {
            return Err(JsonCheckerError::State);
        }
        self.budget.work(self.path.len())?;
        Ok((
            restore,
            Subtree {
                summary: JsonSemanticSummary {
                    traversal_work: self.path.len(),
                    max_value_depth: depth,
                    max_checked_name_bytes: if depth == 0 { 0 } else { self.path.len() },
                    ..JsonSemanticSummary::default()
                },
                receipt: None,
            },
        ))
    }
    fn scalar<S: JsonCheckerSink<Receipt = R>>(
        &mut self,
        value: &[u8],
        kind: JsonScalarKind,
        sink: &mut S,
    ) -> Result<Subtree<R>, JsonCheckerError> {
        self.budget.work(1)?;
        let summary = JsonSemanticSummary::scalar(self.path.len(), value.len())?;
        let receipt = sink
            .scalar(&self.path, value, kind)
            .map_err(|_| JsonCheckerError::Sink)?;
        Ok(Subtree { summary, receipt })
    }
    fn finish<S: JsonCheckerSink<Receipt = R>>(
        &mut self,
        value: Subtree<R>,
        restore: usize,
        sink: &mut S,
    ) -> Result<(), JsonCheckerError> {
        self.path.truncate(restore);
        let Some(frame) = self.frames.last().copied() else {
            self.root = Some(value);
            return Ok(());
        };
        match frame.container {
            Container::Object {
                root,
                pending: Some(id),
                key_mark,
                node_mark,
            } => {
                let mut node = self.member(id)?;
                node.value = value;
                self.store(id, node)?;
                self.frames.last_mut().ok_or(JsonCheckerError::State)?.container = Container::Object {
                    root,
                    pending: None,
                    key_mark,
                    node_mark,
                };
            }
            Container::Array { length } => {
                let accumulated = self.join(frame.accumulated, value, sink)?;
                let last = self.frames.last_mut().ok_or(JsonCheckerError::State)?;
                last.accumulated = accumulated;
                last.container = Container::Array {
                    length: add(length, 1)?,
                };
            }
            _ => return Err(JsonCheckerError::State),
        }
        Ok(())
    }
    fn token<S: JsonCheckerSink<Receipt = R>>(
        &mut self,
        token: JsonStreamToken<'_>,
        sink: &mut S,
    ) -> Result<(), JsonCheckerError> {
        self.budget.work(1)?;
        match token {
            JsonStreamToken::Key(key) => self.key(key.as_bytes(), sink),
            JsonStreamToken::ObjectStart | JsonStreamToken::ArrayStart => {
                let (restore, accumulated) = self.begin()?;
                let container = if matches!(token, JsonStreamToken::ObjectStart) {
                    Container::Object {
                        root: None,
                        pending: None,
                        key_mark: self.keys.len(),
                        node_mark: self.nodes.len(),
                    }
                } else {
                    Container::Array { length: 0 }
                };
                self.budget.reserve(&mut self.frames, 1)?;
                self.frames.push(Frame {
                    container,
                    accumulated,
                    restore,
                });
                Ok(())
            }
            JsonStreamToken::ObjectEnd | JsonStreamToken::ArrayEnd => {
                let frame = self.frames.pop().ok_or(JsonCheckerError::State)?;
                let result = match (token, frame.container) {
                    (
                        JsonStreamToken::ObjectEnd,
                        Container::Object {
                            root,
                            pending: None,
                            key_mark,
                            node_mark,
                        },
                    ) => {
                        let members = self.drain(root, sink)?;
                        self.nodes.truncate(node_mark);
                        self.keys.truncate(key_mark);
                        self.budget.usage.key_bytes = self.keys.len();
                        self.join(frame.accumulated, members, sink)?
                    }
                    (JsonStreamToken::ArrayEnd, Container::Array { length }) => {
                        let mut text = SmallText::new();
                        write!(&mut text, "{length}").map_err(|_| JsonCheckerError::Arithmetic)?;
                        self.budget.work(text.len)?;
                        let scalar = self.scalar(text.bytes(), JsonScalarKind::ArrayLength, sink)?;
                        self.join(frame.accumulated, scalar, sink)?
                    }
                    _ => return Err(JsonCheckerError::State),
                };
                self.finish(result, frame.restore, sink)
            }
            token => {
                let (restore, visit) = self.begin()?;
                let mut number = SmallText::new();
                let (value, kind) = match token {
                    JsonStreamToken::String(value) => (value.as_bytes(), JsonScalarKind::String),
                    JsonStreamToken::Number { value, .. } => {
                        value
                            .write_canonical(&mut number)
                            .map_err(|_| JsonCheckerError::Arithmetic)?;
                        self.budget.work(number.len)?;
                        (number.bytes(), JsonScalarKind::Number)
                    }
                    JsonStreamToken::Boolean(value) => (
                        if value { &b"true"[..] } else { &b"false"[..] },
                        JsonScalarKind::Boolean,
                    ),
                    JsonStreamToken::Null => (&b""[..], JsonScalarKind::Null),
                    _ => return Err(JsonCheckerError::State),
                };
                let scalar = self.scalar(value, kind, sink)?;
                let result = self.join(visit, scalar, sink)?;
                self.finish(result, restore, sink)
            }
        }
    }
    fn release(&mut self) {
        self.nodes = Vec::new();
        self.keys = SmallVec::new();
        self.path = SmallVec::new();
        self.frames = Vec::new();
        self.root = None;
        self.budget.usage.retained_bytes = 0;
        self.budget.usage.key_bytes = 0;
        self.budget.usage.members = 0;
    }
}

struct SmallText {
    bytes: [u8; 64],
    len: usize,
}
impl SmallText {
    fn new() -> Self {
        Self { bytes: [0; 64], len: 0 }
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}
impl Write for SmallText {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        let end = self
            .len
            .checked_add(input.len())
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| std::io::Error::other("JSON scalar formatting exceeded its fixed buffer"))?;
        self.bytes[self.len..end].copy_from_slice(input);
        self.len = end;
        Ok(input.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Terminal {
    Active,
    Complete,
    Failed(JsonCheckerError),
}

/// One bounded grammar pass with synchronous scalar inspection. Feed every event
/// to the same logical sink. Successful Complete transfers its receipt to seal;
/// terminal errors call abandon exactly once. Dropping an active checker does
/// not access a separately owned sink: callers must send Abandoned first.
pub struct JsonChecker<R: Copy> {
    lexical: JsonStream,
    reducer: Reducer<R>,
    terminal: Terminal,
    summary: Option<JsonSemanticSummary>,
}

impl<R: Copy> fmt::Debug for JsonChecker<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonChecker")
            .field("usage", &self.usage())
            .finish_non_exhaustive()
    }
}

impl<R: Copy> JsonChecker<R> {
    /// Internal hint for an already owned complete source; no bytes are parsed here.
    pub(crate) fn with_source_len(
        root: &str,
        lexical: JsonStreamLimits,
        limits: JsonStructureLimits,
        source_len: usize,
    ) -> Result<Self, JsonCheckerError> {
        let mut checker = Self::new(root, lexical, limits)?;
        checker.reducer.hints = CapacityHints::for_source::<R>(source_len, root.len(), limits);
        Ok(checker)
    }
    pub fn new(root: &str, lexical: JsonStreamLimits, limits: JsonStructureLimits) -> Result<Self, JsonCheckerError> {
        Budget::check(root.len(), limits.max_path_bytes, JsonStructureLimit::PathBytes)?;
        let mut budget = Budget {
            limits,
            usage: JsonStructureUsage::default(),
        };
        let mut path = SmallVec::new();
        budget.reserve_bytes(&mut path, root.len())?;
        budget.work(root.len())?;
        path.extend_from_slice(root.as_bytes());
        #[cfg(test)]
        measurements::record(Event::PathCopyBytes, root.len());
        Ok(Self {
            lexical: JsonStream::new(lexical),
            reducer: Reducer {
                nodes: Vec::new(),
                hints: CapacityHints::default(),
                keys: SmallVec::new(),
                path,
                frames: Vec::new(),
                root: None,
                empty_root: root.is_empty(),
                budget,
            },
            terminal: Terminal::Active,
            summary: None,
        })
    }
    pub fn usage(&self) -> JsonStructureUsage {
        self.reducer.budget.usage
    }
    pub fn lexical_usage(&self) -> JsonStreamUsage {
        self.lexical.usage()
    }
    pub fn summary(&self) -> Option<JsonSemanticSummary> {
        self.summary
    }

    pub fn feed<S: JsonCheckerSink<Receipt = R>>(
        &mut self,
        input: JsonStreamInput<'_>,
        sink: &mut S,
    ) -> Result<(), JsonCheckerError> {
        match self.terminal {
            Terminal::Complete => return Err(JsonCheckerError::AlreadyComplete),
            Terminal::Failed(error) => return Err(error),
            Terminal::Active => {}
        }
        let mut callback_error = None;
        let result = self
            .lexical
            .feed(input, |token| {
                self.reducer.token(token, sink).map_err(|error| {
                    callback_error = Some(error);
                    JsonStreamSinkError
                })
            })
            .map_err(|error| callback_error.unwrap_or(JsonCheckerError::Lexical(error)));
        let result = result.and_then(|()| {
            if matches!(input, JsonStreamInput::Complete) {
                if !self.reducer.frames.is_empty() {
                    return Err(JsonCheckerError::State);
                }
                let root = self.reducer.root.ok_or(JsonCheckerError::State)?;
                sink.seal(root.receipt, root.summary)
                    .map_err(|_| JsonCheckerError::Sink)?;
                self.summary = Some(root.summary);
                self.terminal = Terminal::Complete;
                self.reducer.release();
            }
            Ok(())
        });
        if let Err(error) = result {
            sink.abandon();
            self.reducer.release();
            self.terminal = Terminal::Failed(error);
            // Completion can fail in the consumer after the lexer has completed;
            // all other lexical failures already release their own buffers.
        }
        result
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_checker_test.rs"]
mod tests;
