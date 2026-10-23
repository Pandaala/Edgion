//! DSL-specific streaming regex and JSON scanners.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::sync::OnceLock;
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use edgion_dsl::json_path::{DefiniteJsonPath, JsonPathSegment};
use regex_automata::dfa::{dense, Automaton};
use regex_automata::util::primitives::StateID;
use regex_automata::Input;
use serde::de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::request_body::BodyReadError;
use crate::request_body::{BodyOperationError, RawBodyScanner, ScanControl, TextBodyScanner};

use edgion_dsl::value::Value;

pub(crate) struct StreamingRegexScanner {
    dfa: Arc<dense::DFA<Vec<u32>>>,
    state: StateID,
    matched: bool,
}

impl StreamingRegexScanner {
    pub(crate) fn new(dfa: Arc<dense::DFA<Vec<u32>>>) -> Result<Self, BodyOperationError> {
        let state = dfa
            .start_state_forward(&Input::new(b""))
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        Ok(Self {
            dfa,
            state,
            matched: false,
        })
    }
}

impl TextBodyScanner for StreamingRegexScanner {
    type Output = bool;

    fn push(&mut self, chunk: &str) -> Result<ScanControl, BodyOperationError> {
        for &byte in chunk.as_bytes() {
            self.state = self.dfa.next_state(self.state, byte);
            if self.dfa.is_match_state(self.state) {
                self.matched = true;
                return Ok(ScanControl::Stop);
            }
        }
        Ok(ScanControl::Continue)
    }

    fn finish(mut self) -> Result<Self::Output, BodyOperationError> {
        self.state = self.dfa.next_eoi_state(self.state);
        Ok(self.matched || self.dfa.is_match_state(self.state))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum JsonScanValue {
    Value(Value),
    Missing,
    TooLong,
}

static JSON_SCAN_LIMITER: OnceLock<Arc<Semaphore>> = OnceLock::new();

async fn acquire_json_scan_permit() -> Result<OwnedSemaphorePermit, BodyOperationError> {
    Arc::clone(JSON_SCAN_LIMITER.get_or_init(|| Arc::new(Semaphore::new(32))))
        .acquire_owned()
        .await
        .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))
}

pub(crate) struct JsonPathScanner {
    sender: Option<mpsc::SyncSender<Vec<u8>>>,
    worker: Option<JoinHandle<JsonScanValue>>,
    disconnected: bool,
}

impl JsonPathScanner {
    pub(crate) async fn new(path: Arc<DefiniteJsonPath>, limit: usize) -> Result<Self, BodyOperationError> {
        let permit = acquire_json_scan_permit().await?;
        let (sender, receiver) = mpsc::sync_channel(2);
        let worker = std::thread::Builder::new()
            .name("edgion-json-path".to_string())
            .spawn(move || parse_json_stream(receiver, path, limit, permit))
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            disconnected: false,
        })
    }
}

impl JsonPathScanner {
    pub(crate) async fn scan_owned(
        input: Arc<str>,
        path: Arc<DefiniteJsonPath>,
        limit: usize,
    ) -> Result<JsonScanValue, BodyOperationError> {
        let permit = acquire_json_scan_permit().await?;
        crate::request_body::run_blocking_scan(move || {
            let _permit = permit;
            Ok(parse_json(
                serde_json::Deserializer::from_slice(input.as_bytes()),
                path.segments(),
                limit,
            ))
        })
        .await
    }
}

#[derive(Debug)]
struct MutableJsonSelectNode {
    capture: Option<usize>,
    members: BTreeMap<String, MutableJsonSelectNode>,
    indices: BTreeMap<u64, MutableJsonSelectNode>,
}

impl MutableJsonSelectNode {
    fn new() -> Self {
        Self {
            capture: None,
            members: BTreeMap::new(),
            indices: BTreeMap::new(),
        }
    }

    fn insert(&mut self, path: &DefiniteJsonPath, next_capture: &mut usize) -> usize {
        let mut node = self;
        for segment in path.segments() {
            node = match segment {
                JsonPathSegment::Member(member) => node.members.entry(member.clone()).or_insert_with(Self::new),
                JsonPathSegment::Index(index) => node.indices.entry(*index).or_insert_with(Self::new),
            };
        }
        *node.capture.get_or_insert_with(|| {
            let capture = *next_capture;
            *next_capture += 1;
            capture
        })
    }

    fn freeze(self) -> Arc<JsonSelectNode> {
        let members: BTreeMap<Arc<str>, Arc<JsonSelectNode>> = self
            .members
            .into_iter()
            .map(|(member, node)| (Arc::from(member), node.freeze()))
            .collect();
        let indices: BTreeMap<u64, Arc<JsonSelectNode>> = self
            .indices
            .into_iter()
            .map(|(index, node)| (index, node.freeze()))
            .collect::<BTreeMap<_, _>>();
        let mut captures = Vec::new();
        if let Some(capture) = self.capture {
            captures.push(capture);
        }
        for node in members.values().chain(indices.values()) {
            captures.extend_from_slice(&node.descendant_captures);
        }
        Arc::new(JsonSelectNode {
            capture: self.capture,
            members,
            indices,
            descendant_captures: captures.into(),
        })
    }
}

#[derive(Debug)]
struct JsonSelectNode {
    capture: Option<usize>,
    members: BTreeMap<Arc<str>, Arc<JsonSelectNode>>,
    indices: BTreeMap<u64, Arc<JsonSelectNode>>,
    descendant_captures: Arc<[usize]>,
}

#[derive(Debug)]
pub(crate) struct JsonSelectPlan {
    root: Arc<JsonSelectNode>,
    outputs: Arc<[(Arc<str>, usize)]>,
    capture_count: usize,
}

impl JsonSelectPlan {
    pub(crate) fn new(entries: impl IntoIterator<Item = (String, DefiniteJsonPath)>) -> Self {
        let mut root = MutableJsonSelectNode::new();
        let mut capture_count = 0usize;
        let outputs = entries
            .into_iter()
            .map(|(name, path)| {
                let capture = root.insert(&path, &mut capture_count);
                (Arc::from(name), capture)
            })
            .collect::<Vec<_>>();
        Self {
            root: root.freeze(),
            outputs: outputs.into(),
            capture_count,
        }
    }

    pub(crate) fn outputs(&self) -> &[(Arc<str>, usize)] {
        &self.outputs
    }
}

pub(crate) struct JsonSelectScanner {
    sender: Option<mpsc::SyncSender<Vec<u8>>>,
    worker: Option<JoinHandle<Vec<JsonScanValue>>>,
    disconnected: bool,
}

impl JsonSelectScanner {
    pub(crate) async fn new(plan: Arc<JsonSelectPlan>, limit: usize) -> Result<Self, BodyOperationError> {
        let permit = acquire_json_scan_permit().await?;
        let (sender, receiver) = mpsc::sync_channel(2);
        let worker = std::thread::Builder::new()
            .name("edgion-json-select".to_string())
            .spawn(move || parse_json_select_stream(receiver, plan, limit, permit))
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            disconnected: false,
        })
    }
}

impl RawBodyScanner for JsonSelectScanner {
    type Output = Vec<JsonScanValue>;

    fn requires_blocking_context(&self) -> bool {
        true
    }

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        if self.disconnected {
            return Ok(ScanControl::Stop);
        }
        let Some(sender) = self.sender.as_ref() else {
            return Err(BodyOperationError::Body(BodyReadError::Incomplete));
        };
        if sender.send(chunk.to_vec()).is_err() {
            self.disconnected = true;
            return Ok(ScanControl::Stop);
        }
        Ok(ScanControl::Continue)
    }

    fn finish(mut self) -> Result<Self::Output, BodyOperationError> {
        self.sender.take();
        self.worker
            .take()
            .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?
            .join()
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))
    }
}

impl RawBodyScanner for JsonPathScanner {
    type Output = JsonScanValue;

    fn requires_blocking_context(&self) -> bool {
        true
    }

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        if self.disconnected {
            return Ok(ScanControl::Stop);
        }
        let Some(sender) = self.sender.as_ref() else {
            return Err(BodyOperationError::Body(BodyReadError::Incomplete));
        };
        if sender.send(chunk.to_vec()).is_err() {
            self.disconnected = true;
            return Ok(ScanControl::Stop);
        }
        Ok(ScanControl::Continue)
    }

    fn finish(mut self) -> Result<Self::Output, BodyOperationError> {
        self.sender.take();
        self.worker
            .take()
            .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?
            .join()
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))
    }
}

struct ChannelReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    current: Cursor<Vec<u8>>,
    finished: bool,
}

impl ChannelReader {
    fn new(receiver: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            receiver,
            current: Cursor::new(Vec::new()),
            finished: false,
        }
    }
}

impl Read for ChannelReader {
    fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = self.current.read(target)?;
            if read != 0 {
                return Ok(read);
            }
            if self.finished {
                return Ok(0);
            }
            match self.receiver.recv() {
                Ok(chunk) => self.current = Cursor::new(chunk),
                Err(_) => self.finished = true,
            }
        }
    }
}

fn parse_json_stream(
    receiver: mpsc::Receiver<Vec<u8>>,
    path: Arc<DefiniteJsonPath>,
    limit: usize,
    _permit: OwnedSemaphorePermit,
) -> JsonScanValue {
    let reader = ChannelReader::new(receiver);
    parse_json(serde_json::Deserializer::from_reader(reader), path.segments(), limit)
}

fn parse_json<'de, R>(
    mut deserializer: serde_json::Deserializer<R>,
    segments: &[JsonPathSegment],
    limit: usize,
) -> JsonScanValue
where
    R: serde_json::de::Read<'de>,
{
    let result = SelectSeed {
        segments,
        index: 0,
        limit,
    }
    .deserialize(&mut deserializer);
    match result {
        Ok(value) if deserializer.end().is_ok() => value,
        _ => JsonScanValue::Missing,
    }
}

#[derive(Clone, Copy)]
struct SelectSeed<'a> {
    segments: &'a [JsonPathSegment],
    index: usize,
    limit: usize,
}

impl<'de> DeserializeSeed<'de> for SelectSeed<'_> {
    type Value = JsonScanValue;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if self.index == self.segments.len() {
            return CaptureSeed { limit: self.limit }.deserialize(deserializer);
        }
        deserializer.deserialize_any(SelectVisitor { seed: self })
    }
}

struct SelectVisitor<'a> {
    seed: SelectSeed<'a>,
}

impl<'de> Visitor<'de> for SelectVisitor<'_> {
    type Value = JsonScanValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a valid JSON value")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let JsonPathSegment::Member(expected) = &self.seed.segments[self.seed.index] else {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            return Ok(JsonScanValue::Missing);
        };
        let mut selected = JsonScanValue::Missing;
        while let Some(matches) = map.next_key_seed(ExpectedKeySeed { expected })? {
            if matches {
                selected = map.next_value_seed(SelectSeed {
                    index: self.seed.index + 1,
                    ..self.seed
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(selected)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let JsonPathSegment::Index(expected) = self.seed.segments[self.seed.index] else {
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            return Ok(JsonScanValue::Missing);
        };
        let mut index = 0u64;
        while index < expected {
            if seq.next_element::<IgnoredAny>()?.is_none() {
                return Ok(JsonScanValue::Missing);
            }
            index += 1;
        }
        let Some(selected) = seq.next_element_seed(SelectSeed {
            index: self.seed.index + 1,
            ..self.seed
        })?
        else {
            return Ok(JsonScanValue::Missing);
        };
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(selected)
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Missing)
    }
    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Missing)
    }
    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Missing)
    }
    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Missing)
    }
    fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Missing)
    }
    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Missing)
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Missing)
    }
}

struct ExpectedKeySeed<'a> {
    expected: &'a str,
}

impl<'de> DeserializeSeed<'de> for ExpectedKeySeed<'_> {
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(ExpectedKeyVisitor {
            expected: self.expected,
        })
    }
}

struct ExpectedKeyVisitor<'a> {
    expected: &'a str,
}

struct SelectedMapKey<'a> {
    selected: Option<&'a JsonSelectNode>,
    retained: Option<Arc<str>>,
    overlong_for_parent: bool,
}

struct SelectedMapKeySeed<'a> {
    node: &'a JsonSelectNode,
    retain_for_parent: bool,
    limit: usize,
}

impl<'de, 'a> DeserializeSeed<'de> for SelectedMapKeySeed<'a> {
    type Value = SelectedMapKey<'a>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(SelectedMapKeyVisitor {
            node: self.node,
            retain_for_parent: self.retain_for_parent,
            limit: self.limit,
        })
    }
}

struct SelectedMapKeyVisitor<'a> {
    node: &'a JsonSelectNode,
    retain_for_parent: bool,
    limit: usize,
}

impl<'a> Visitor<'_> for SelectedMapKeyVisitor<'a> {
    type Value = SelectedMapKey<'a>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON object key")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        let selected_entry = self.node.members.get_key_value(value);
        let selected = selected_entry.map(|(_, node)| node.as_ref());
        let (retained, overlong_for_parent) = if self.retain_for_parent {
            if !encoded_string_fits(value, self.limit).map_err(E::custom)? {
                (None, true)
            } else {
                let key = selected_entry.map_or_else(|| Arc::from(value), |(key, _)| Arc::clone(key));
                (Some(key), false)
            }
        } else {
            (None, false)
        };
        Ok(SelectedMapKey {
            selected,
            retained,
            overlong_for_parent,
        })
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.visit_str(&value)
    }
}

impl Visitor<'_> for ExpectedKeyVisitor<'_> {
    type Value = bool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON object key")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(value == self.expected)
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.visit_str(&value)
    }
}

struct JsonSelectState {
    values: Vec<JsonScanValue>,
    retained_bytes: usize,
    retained_by_capture: Vec<usize>,
    limit: usize,
}

impl JsonSelectState {
    fn new(capture_count: usize, limit: usize) -> Self {
        Self {
            values: vec![JsonScanValue::Missing; capture_count],
            retained_bytes: 0,
            retained_by_capture: vec![0; capture_count],
            limit,
        }
    }

    fn reset(&mut self, captures: &[usize]) {
        for &capture in captures {
            if let Some(value) = self.values.get_mut(capture) {
                self.retained_bytes = self.retained_bytes.saturating_sub(self.retained_by_capture[capture]);
                self.retained_by_capture[capture] = 0;
                *value = JsonScanValue::Missing;
            }
        }
    }

    fn set(&mut self, capture: usize, value: JsonScanValue) {
        let Some(slot) = self.values.get_mut(capture) else {
            return;
        };
        self.retained_bytes = self.retained_bytes.saturating_sub(self.retained_by_capture[capture]);
        self.retained_by_capture[capture] = 0;

        let bytes = match &value {
            JsonScanValue::Value(Value::Str(value)) => value.len(),
            _ => 0,
        };
        if matches!(value, JsonScanValue::TooLong)
            || self
                .retained_bytes
                .checked_add(bytes)
                .is_none_or(|total| total > self.limit)
        {
            *slot = JsonScanValue::TooLong;
        } else {
            self.retained_bytes += bytes;
            self.retained_by_capture[capture] = bytes;
            *slot = value;
        }
    }
}

fn parse_json_select_stream(
    receiver: mpsc::Receiver<Vec<u8>>,
    plan: Arc<JsonSelectPlan>,
    limit: usize,
    _permit: OwnedSemaphorePermit,
) -> Vec<JsonScanValue> {
    let reader = ChannelReader::new(receiver);
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let state = RefCell::new(JsonSelectState::new(plan.capture_count, limit));
    let result = JsonSelectSeed {
        node: &plan.root,
        state: &state,
        emit_to_parent: false,
        limit,
    }
    .deserialize(&mut deserializer);
    if result.is_err() || deserializer.end().is_err() {
        return vec![JsonScanValue::Missing; plan.capture_count];
    }
    state.into_inner().values
}

#[derive(Clone)]
struct JsonSelectSeed<'a> {
    node: &'a JsonSelectNode,
    state: &'a RefCell<JsonSelectState>,
    emit_to_parent: bool,
    limit: usize,
}

impl<'de> DeserializeSeed<'de> for JsonSelectSeed<'_> {
    type Value = BoundedJson;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let capture = self.node.capture;
        let value = deserializer.deserialize_any(JsonSelectVisitor {
            node: self.node,
            state: self.state,
            capture_current: capture.is_some(),
            emit_to_parent: self.emit_to_parent,
            limit: self.limit,
        })?;
        if let Some(capture) = capture {
            self.state.borrow_mut().set(capture, value.captured);
        }
        if self.emit_to_parent {
            Ok(value.encoded)
        } else {
            Ok(BoundedJson::new(self.limit))
        }
    }
}

struct JsonSelectValue {
    encoded: BoundedJson,
    captured: JsonScanValue,
}

struct JsonSelectVisitor<'a> {
    node: &'a JsonSelectNode,
    state: &'a RefCell<JsonSelectState>,
    capture_current: bool,
    emit_to_parent: bool,
    limit: usize,
}

impl JsonSelectVisitor<'_> {
    fn scalar(self, captured: JsonScanValue, encoded: &[u8]) -> JsonSelectValue {
        JsonSelectValue {
            encoded: if self.emit_to_parent {
                BoundedJson::from_bytes(self.limit, encoded)
            } else {
                BoundedJson::new(self.limit)
            },
            captured,
        }
    }

    fn finish_container(self, mut encoded: BoundedJson) -> JsonSelectValue {
        let captured = if self.capture_current {
            if self.emit_to_parent {
                encoded.to_scan_value()
            } else {
                std::mem::replace(&mut encoded, BoundedJson::new(self.limit)).into_scan_value()
            }
        } else {
            JsonScanValue::Missing
        };
        JsonSelectValue { encoded, captured }
    }
}

impl<'de> Visitor<'de> for JsonSelectVisitor<'_> {
    type Value = JsonSelectValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a valid JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        let encoded = if value { b"true" as &[u8] } else { b"false" };
        Ok(self.scalar(JsonScanValue::Value(Value::Bool(value)), encoded))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        if !self.emit_to_parent {
            return Ok(self.scalar(JsonScanValue::Value(Value::Int(value)), b""));
        }
        let encoded = value.to_string();
        Ok(self.scalar(JsonScanValue::Value(Value::Int(value)), encoded.as_bytes()))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
        if let Ok(value) = i64::try_from(value) {
            return self.visit_i64(value);
        }
        let encoded = value.to_string();
        let captured = match i64::try_from(value) {
            Ok(value) => JsonScanValue::Value(Value::Int(value)),
            Err(_) if encoded.len() > self.limit => JsonScanValue::TooLong,
            Err(_) => JsonScanValue::Value(Value::Str(Arc::from(encoded.as_str()))),
        };
        Ok(self.scalar(captured, encoded.as_bytes()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        let encoded = json_float_string(value);
        let captured = if encoded.len() > self.limit {
            JsonScanValue::TooLong
        } else {
            JsonScanValue::Value(Value::Str(Arc::from(encoded.as_str())))
        };
        Ok(self.scalar(captured, encoded.as_bytes()))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        let captured = if !self.capture_current {
            JsonScanValue::Missing
        } else if value.len() > self.limit {
            JsonScanValue::TooLong
        } else {
            JsonScanValue::Value(Value::Str(Arc::from(value)))
        };
        let mut encoded = BoundedJson::new(self.limit);
        if self.emit_to_parent {
            serde_json::to_writer(&mut encoded, &value).map_err(E::custom)?;
        }
        Ok(JsonSelectValue { encoded, captured })
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.visit_str(&value)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(self.scalar(JsonScanValue::Value(Value::Nil), b"null"))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(self.scalar(JsonScanValue::Value(Value::Nil), b"null"))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut output = BoundedJson::new(self.limit);
        let encode_current = self.emit_to_parent || self.capture_current;
        if encode_current {
            output.push(b"[");
        }
        let mut index = 0u64;
        let mut first = true;
        loop {
            let selected = self.node.indices.get(&index).map(AsRef::as_ref);
            let retain_for_parent = encode_current && !output.oversized;
            let value = match selected {
                Some(node) => {
                    self.state.borrow_mut().reset(&node.descendant_captures);
                    seq.next_element_seed(JsonSelectSeed {
                        node,
                        state: self.state,
                        emit_to_parent: retain_for_parent,
                        limit: self.limit,
                    })?
                }
                None if retain_for_parent => seq.next_element_seed(BoundedSeed { limit: self.limit })?,
                None => {
                    if seq.next_element::<IgnoredAny>()?.is_some() {
                        Some(BoundedJson::new(self.limit))
                    } else {
                        None
                    }
                }
            };
            let Some(value) = value else {
                break;
            };
            if retain_for_parent {
                if !first {
                    output.push(b",");
                }
                first = false;
                output.push_value(value);
            }
            index = index.saturating_add(1);
        }
        if encode_current {
            output.push(b"]");
        }
        Ok(self.finish_container(output))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let encode_current = self.emit_to_parent || self.capture_current;
        let mut output = BoundedMapAccumulator::new(self.limit);
        while let Some(key) = map.next_key_seed(SelectedMapKeySeed {
            node: self.node,
            retain_for_parent: encode_current && output.accepts_candidates(),
            limit: self.limit,
        })? {
            if key.overlong_for_parent {
                output.mark_irreversible();
            }
            let retain_for_parent = key.retained.is_some();
            let value = match key.selected {
                Some(node) => {
                    self.state.borrow_mut().reset(&node.descendant_captures);
                    map.next_value_seed(JsonSelectSeed {
                        node,
                        state: self.state,
                        emit_to_parent: retain_for_parent,
                        limit: self.limit,
                    })?
                }
                None if retain_for_parent => map.next_value_seed(BoundedSeed { limit: self.limit })?,
                None => {
                    map.next_value::<IgnoredAny>()?;
                    BoundedJson::new(self.limit)
                }
            };
            if let Some(key) = key.retained {
                output.insert(key, value).map_err(A::Error::custom)?;
            }
        }

        let encoded = if encode_current {
            output.finish().map_err(A::Error::custom)?
        } else {
            BoundedJson::new(self.limit)
        };
        Ok(self.finish_container(encoded))
    }
}

struct CaptureSeed {
    limit: usize,
}

impl<'de> DeserializeSeed<'de> for CaptureSeed {
    type Value = JsonScanValue;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(CaptureVisitor { limit: self.limit })
    }
}

struct CaptureVisitor {
    limit: usize,
}

fn json_float_string(value: f64) -> String {
    serde_json::Number::from_f64(value).map_or_else(|| value.to_string(), |number| number.to_string())
}

impl<'de> Visitor<'de> for CaptureVisitor {
    type Value = JsonScanValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a valid JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Value(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Value(Value::Int(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(match i64::try_from(value) {
            Ok(value) => JsonScanValue::Value(Value::Int(value)),
            Err(_) => {
                let value = value.to_string();
                if value.len() > self.limit {
                    JsonScanValue::TooLong
                } else {
                    JsonScanValue::Value(Value::Str(value.into()))
                }
            }
        })
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        let value = json_float_string(value);
        Ok(if value.len() > self.limit {
            JsonScanValue::TooLong
        } else {
            JsonScanValue::Value(Value::Str(value.into()))
        })
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(if value.len() > self.limit {
            JsonScanValue::TooLong
        } else {
            JsonScanValue::Value(Value::Str(value.into()))
        })
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.visit_str(&value)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Value(Value::Nil))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(JsonScanValue::Value(Value::Nil))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut output = BoundedJson::new(self.limit);
        output.push(b"[");
        let mut first = true;
        loop {
            if output.oversized {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                break;
            }
            let Some(value) = seq.next_element_seed(BoundedSeed { limit: self.limit })? else {
                break;
            };
            if !first {
                output.push(b",");
            }
            first = false;
            output.push_value(value);
        }
        output.push(b"]");
        Ok(output.into_scan_value())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        read_bounded_map(&mut map, self.limit).map(BoundedJson::into_scan_value)
    }
}

struct BoundedSeed {
    limit: usize,
}

impl<'de> DeserializeSeed<'de> for BoundedSeed {
    type Value = BoundedJson;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(BoundedVisitor { limit: self.limit })
    }
}

struct BoundedVisitor {
    limit: usize,
}

impl<'de> Visitor<'de> for BoundedVisitor {
    type Value = BoundedJson;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a valid JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(BoundedJson::from_bytes(
            self.limit,
            if value { b"true" } else { b"false" },
        ))
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
        let mut encoded = BoundedJson::new(self.limit);
        serde_json::to_writer(&mut encoded, &value).map_err(E::custom)?;
        Ok(encoded)
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
        let mut encoded = BoundedJson::new(self.limit);
        serde_json::to_writer(&mut encoded, &value).map_err(E::custom)?;
        Ok(encoded)
    }
    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(BoundedJson::from_bytes(self.limit, json_float_string(value).as_bytes()))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        if value.len() > self.limit {
            return Ok(BoundedJson::oversized(self.limit));
        }
        let mut encoded = BoundedJson::new(self.limit);
        serde_json::to_writer(&mut encoded, &value).map_err(E::custom)?;
        Ok(encoded)
    }
    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(BoundedJson::from_bytes(self.limit, b"null"))
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(BoundedJson::from_bytes(self.limit, b"null"))
    }
    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut output = BoundedJson::new(self.limit);
        output.push(b"[");
        let mut first = true;
        loop {
            if output.oversized {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                break;
            }
            let Some(value) = seq.next_element_seed(BoundedSeed { limit: self.limit })? else {
                break;
            };
            if !first {
                output.push(b",");
            }
            first = false;
            output.push_value(value);
        }
        output.push(b"]");
        Ok(output)
    }
    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        read_bounded_map(&mut map, self.limit)
    }
}

fn read_bounded_map<'de, A>(map: &mut A, limit: usize) -> Result<BoundedJson, A::Error>
where
    A: MapAccess<'de>,
{
    let mut output = BoundedMapAccumulator::new(limit);
    loop {
        if !output.accepts_candidates() {
            if map.next_key::<IgnoredAny>()?.is_none() {
                break;
            }
            map.next_value::<IgnoredAny>()?;
            continue;
        }
        let Some(key) = map.next_key_seed(BoundedMapKeySeed { limit })? else {
            break;
        };
        let Some(key) = key else {
            output.mark_irreversible();
            map.next_value::<IgnoredAny>()?;
            continue;
        };
        let value = map.next_value_seed(BoundedSeed { limit })?;
        output.insert(key, value).map_err(A::Error::custom)?;
    }
    output.finish().map_err(A::Error::custom)
}

struct BoundedMapKeySeed {
    limit: usize,
}

impl<'de> DeserializeSeed<'de> for BoundedMapKeySeed {
    type Value = Option<Arc<str>>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(BoundedMapKeyVisitor { limit: self.limit })
    }
}

struct BoundedMapKeyVisitor {
    limit: usize,
}

impl Visitor<'_> for BoundedMapKeyVisitor {
    type Value = Option<Arc<str>>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON object key")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        if !encoded_string_fits(value, self.limit).map_err(E::custom)? {
            Ok(None)
        } else {
            Ok(Some(Arc::from(value)))
        }
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.visit_str(&value)
    }
}

struct BoundedMapAccumulator {
    entries: BTreeMap<Arc<str>, BoundedJson>,
    rendered_len: usize,
    limit: usize,
    overflow_key: Option<Arc<str>>,
    irreversible_oversize: bool,
}

impl BoundedMapAccumulator {
    fn new(limit: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            rendered_len: 2,
            limit,
            overflow_key: None,
            irreversible_oversize: limit < 2,
        }
    }

    fn is_oversized(&self) -> bool {
        self.irreversible_oversize || self.overflow_key.is_some()
    }

    fn accepts_candidates(&self) -> bool {
        !self.irreversible_oversize
    }

    fn insert(&mut self, key: impl Into<Arc<str>>, value: BoundedJson) -> Result<(), serde_json::Error> {
        let key = key.into();
        if self.irreversible_oversize {
            return Ok(());
        }

        let mut encoded_key_len = CountingWriter::default();
        serde_json::to_writer(&mut encoded_key_len, &key.as_ref())?;
        let encoded_key_len = encoded_key_len.len;
        if self.overflow_key.as_deref() == Some(key.as_ref()) {
            self.overflow_key = None;
        }
        if let Some(old) = self.entries.remove(&key) {
            let old_entry_len = encoded_key_len.saturating_add(1).saturating_add(old.bytes.len());
            self.rendered_len = self.rendered_len.saturating_sub(old_entry_len);
            if !self.entries.is_empty() {
                self.rendered_len = self.rendered_len.saturating_sub(1);
            }
        }
        if value.oversized {
            self.record_overflow(key, encoded_key_len);
            return Ok(());
        }

        let new_entry_len = encoded_key_len.saturating_add(1).saturating_add(value.bytes.len());
        let candidate_len = self
            .rendered_len
            .saturating_add(usize::from(!self.entries.is_empty()))
            .saturating_add(new_entry_len);
        if candidate_len > self.limit {
            self.record_overflow(key, encoded_key_len);
            return Ok(());
        }

        self.rendered_len = candidate_len;
        self.entries.insert(key, value);
        Ok(())
    }

    fn record_overflow(&mut self, key: Arc<str>, encoded_key_len: usize) {
        if encoded_key_len > self.limit
            || self
                .overflow_key
                .as_deref()
                .is_some_and(|overflow_key| overflow_key != key.as_ref())
        {
            self.mark_irreversible();
        } else {
            self.overflow_key = Some(key);
        }
    }

    fn mark_irreversible(&mut self) {
        self.entries.clear();
        self.rendered_len = 0;
        self.overflow_key = None;
        self.irreversible_oversize = true;
    }

    fn finish(self) -> Result<BoundedJson, serde_json::Error> {
        if self.is_oversized() {
            return Ok(BoundedJson::oversized(self.limit));
        }
        let mut output = BoundedJson::new(self.limit);
        output.push(b"{");
        for (index, (key, value)) in self.entries.into_iter().enumerate() {
            if index != 0 {
                output.push(b",");
            }
            serde_json::to_writer(&mut output, &key.as_ref())?;
            output.push(b":");
            output.push_value(value);
        }
        output.push(b"}");
        debug_assert_eq!(output.bytes.len(), self.rendered_len);
        Ok(output)
    }
}

struct BoundedJson {
    bytes: Vec<u8>,
    limit: usize,
    oversized: bool,
}

#[derive(Default)]
struct CountingWriter {
    len: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.len = self.len.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encoded_string_len(value: &str) -> Result<usize, serde_json::Error> {
    let mut count = CountingWriter::default();
    serde_json::to_writer(&mut count, &value)?;
    Ok(count.len)
}

fn encoded_string_fits(value: &str, limit: usize) -> Result<bool, serde_json::Error> {
    if value.len().checked_add(2).is_none_or(|minimum| minimum > limit) {
        return Ok(false);
    }
    Ok(encoded_string_len(value)? <= limit)
}

impl BoundedJson {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            oversized: false,
        }
    }

    fn oversized(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            oversized: true,
        }
    }

    fn from_bytes(limit: usize, bytes: &[u8]) -> Self {
        let mut value = Self::new(limit);
        value.push(bytes);
        value
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.oversized {
            return;
        }
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > self.limit)
        {
            self.bytes.clear();
            self.oversized = true;
        } else {
            self.bytes.extend_from_slice(bytes);
        }
    }

    fn push_value(&mut self, mut value: Self) {
        if self.oversized {
            return;
        }
        if value.oversized {
            self.bytes.clear();
            self.oversized = true;
            return;
        }
        if self
            .bytes
            .len()
            .checked_add(value.bytes.len())
            .is_none_or(|len| len > self.limit)
        {
            self.bytes.clear();
            self.oversized = true;
        } else {
            self.bytes.append(&mut value.bytes);
        }
    }

    fn into_scan_value(self) -> JsonScanValue {
        if self.oversized {
            JsonScanValue::TooLong
        } else {
            match String::from_utf8(self.bytes) {
                Ok(value) => JsonScanValue::Value(Value::Str(value.into())),
                Err(_) => JsonScanValue::Missing,
            }
        }
    }

    fn to_scan_value(&self) -> JsonScanValue {
        if self.oversized {
            JsonScanValue::TooLong
        } else {
            match std::str::from_utf8(&self.bytes) {
                Ok(value) => JsonScanValue::Value(Value::Str(value.into())),
                Err(_) => JsonScanValue::Missing,
            }
        }
    }
}

impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.push(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/body_scanners_test.rs"]
mod tests;
