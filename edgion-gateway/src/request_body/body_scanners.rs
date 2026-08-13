use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use edgion_dsl::json_path::{DefiniteJsonPath, JsonPathSegment};
use hmac::{Hmac, Mac};
use md5::Digest as _;
use regex_automata::dfa::{dense, Automaton};
use regex_automata::util::primitives::StateID;
use regex_automata::Input;
use serde::de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::plugins::runtime::traits::session::BodyReadError;
use crate::request_body::{BodyOperationError, RawBodyScanner, ScanControl, TextBodyScanner};

use edgion_dsl::value::Value;

pub(crate) struct Md5Scanner(md5::Md5);

impl Md5Scanner {
    pub(crate) fn new() -> Self {
        Self(md5::Md5::new())
    }
}

impl RawBodyScanner for Md5Scanner {
    type Output = [u8; 16];

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        self.0.update(chunk);
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(self.0.finalize().into())
    }
}

pub(crate) struct HmacSha256Scanner(Hmac<sha2::Sha256>);

impl HmacSha256Scanner {
    pub(crate) fn new(key: &[u8]) -> Result<Self, BodyOperationError> {
        Hmac::<sha2::Sha256>::new_from_slice(key)
            .map(Self)
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))
    }
}

impl RawBodyScanner for HmacSha256Scanner {
    type Output = [u8; 32];

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        self.0.update(chunk);
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(self.0.finalize().into_bytes().into())
    }
}

#[cfg(test)]
mod digest_scanner_tests {
    use super::*;
    use crate::request_body::RequestBodyState;

    #[tokio::test]
    async fn known_empty_body_finishes_digest_scanners_without_scan_bytes() {
        let mut body = RequestBodyState::default();
        body.mark_known_empty();

        let md5 = body.scan_raw(Md5Scanner::new(), 0).await.unwrap();
        assert_eq!(hex::encode(md5.value), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5.bytes_scanned, 0);

        let hmac = body
            .scan_raw(HmacSha256Scanner::new(b"secret").unwrap(), 0)
            .await
            .unwrap();
        assert_eq!(
            hex::encode(hmac.value),
            "f9e66e179b6747ae54108f82f8ade8b3c25d76fd30afde6c395822c530196169"
        );
        assert_eq!(hmac.bytes_scanned, 0);
    }

    #[tokio::test]
    async fn digest_scanners_follow_the_published_mutation_snapshot() {
        use bytes::Bytes;
        use pingora_core::protocols::http::RequestBodyBuffer;

        let (mut storage, view) = crate::request_body::BodyStorage::new(1, 3);
        storage.write(&Bytes::from_static(b"abc")).await.unwrap();
        storage.finish().await.unwrap();
        let mut body = RequestBodyState::default();
        body.install(view);

        let initial_md5 = body.scan_raw(Md5Scanner::new(), 3).await.unwrap();
        assert_eq!(hex::encode(initial_md5.value), "900150983cd24fb0d6963f7d28e17f72");
        let initial_hmac = body
            .scan_raw(HmacSha256Scanner::new(b"secret").unwrap(), 3)
            .await
            .unwrap();
        assert_eq!(
            hex::encode(initial_hmac.value),
            "9946dad4e00e913fc8be8e5d3f7e110a4a9e832f83fb09c345285d78638d8a0e"
        );

        body.replace_literal(Bytes::from_static(b"a"), Bytes::from_static(b"d"), 3)
            .await
            .unwrap();
        let mutated_md5 = body.scan_raw(Md5Scanner::new(), 3).await.unwrap();
        assert_eq!(hex::encode(mutated_md5.value), "4a429e0b8871ec788a3a171545d959e8");
        let mutated_hmac = body
            .scan_raw(HmacSha256Scanner::new(b"secret").unwrap(), 3)
            .await
            .unwrap();
        assert_eq!(
            hex::encode(mutated_hmac.value),
            "1f24ebf1762bcdba36c3dc8fcebb4b0acc409ac6531ddda81136effbdd6bb52a"
        );
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JsonDuplicatePolicy {
    LastWins,
    RejectSelected,
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
    pub(crate) async fn new(path: DefiniteJsonPath, limit: usize) -> Result<Self, BodyOperationError> {
        let permit = acquire_json_scan_permit().await?;
        let (sender, receiver) = mpsc::sync_channel(2);
        let segments: Arc<[JsonPathSegment]> = Arc::from(path.segments().to_vec());
        let worker = std::thread::Builder::new()
            .name("edgion-json-path".to_string())
            .spawn(move || parse_json_stream(receiver, segments, limit, permit))
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            disconnected: false,
        })
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
        let members: BTreeMap<String, Arc<JsonSelectNode>> = self
            .members
            .into_iter()
            .map(|(member, node)| (member, node.freeze()))
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
    members: BTreeMap<String, Arc<JsonSelectNode>>,
    indices: BTreeMap<u64, Arc<JsonSelectNode>>,
    descendant_captures: Arc<[usize]>,
}

#[derive(Debug)]
pub(crate) struct JsonSelectPlan {
    root: Arc<JsonSelectNode>,
    outputs: Arc<[(Arc<str>, usize)]>,
    capture_count: usize,
    duplicate_policy: JsonDuplicatePolicy,
}

impl JsonSelectPlan {
    pub(crate) fn new(entries: impl IntoIterator<Item = (String, DefiniteJsonPath)>) -> Self {
        Self::with_duplicate_policy(entries, JsonDuplicatePolicy::LastWins)
    }

    pub(crate) fn with_duplicate_policy(
        entries: impl IntoIterator<Item = (String, DefiniteJsonPath)>,
        duplicate_policy: JsonDuplicatePolicy,
    ) -> Self {
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
            duplicate_policy,
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
    segments: Arc<[JsonPathSegment]>,
    limit: usize,
    _permit: OwnedSemaphorePermit,
) -> JsonScanValue {
    let reader = ChannelReader::new(receiver);
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let result = SelectSeed {
        segments: &segments,
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
        while let Some(key) = map.next_key::<String>()? {
            if key == *expected {
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
        let mut selected = JsonScanValue::Missing;
        let mut index = 0u64;
        while index <= expected {
            if index == expected {
                selected = match seq.next_element_seed(SelectSeed {
                    index: self.seed.index + 1,
                    ..self.seed
                })? {
                    Some(value) => value,
                    None => return Ok(JsonScanValue::Missing),
                };
            } else if seq.next_element::<IgnoredAny>()?.is_none() {
                return Ok(JsonScanValue::Missing);
            }
            index += 1;
        }
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

struct JsonSelectState {
    values: Vec<JsonScanValue>,
    retained_bytes: usize,
    retained_by_capture: Vec<usize>,
    limit: usize,
    rejected: Vec<bool>,
    duplicate_policy: JsonDuplicatePolicy,
}

impl JsonSelectState {
    fn new(capture_count: usize, limit: usize, duplicate_policy: JsonDuplicatePolicy) -> Self {
        Self {
            values: vec![JsonScanValue::Missing; capture_count],
            retained_bytes: 0,
            retained_by_capture: vec![0; capture_count],
            limit,
            rejected: vec![false; capture_count],
            duplicate_policy,
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
        if self.rejected.get(capture).copied().unwrap_or(true) {
            return;
        }
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

    fn reject(&mut self, captures: &[usize]) {
        for &capture in captures {
            if capture < self.values.len() {
                self.rejected[capture] = true;
                self.values[capture] = JsonScanValue::Missing;
                self.retained_bytes = self.retained_bytes.saturating_sub(self.retained_by_capture[capture]);
                self.retained_by_capture[capture] = 0;
            }
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
    let state = Rc::new(RefCell::new(JsonSelectState::new(
        plan.capture_count,
        limit,
        plan.duplicate_policy,
    )));
    let result = JsonSelectSeed {
        node: Arc::clone(&plan.root),
        state: Rc::clone(&state),
        emit_to_parent: false,
        limit,
    }
    .deserialize(&mut deserializer);
    if result.is_err() || deserializer.end().is_err() {
        return vec![JsonScanValue::Missing; plan.capture_count];
    }
    Rc::try_unwrap(state)
        .map(|state| state.into_inner().values)
        .unwrap_or_else(|_| vec![JsonScanValue::Missing; plan.capture_count])
}

#[derive(Clone)]
struct JsonSelectSeed {
    node: Arc<JsonSelectNode>,
    state: Rc<RefCell<JsonSelectState>>,
    emit_to_parent: bool,
    limit: usize,
}

impl<'de> DeserializeSeed<'de> for JsonSelectSeed {
    type Value = BoundedJson;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encode_current = self.emit_to_parent || self.node.capture.is_some();
        let capture = self.node.capture;
        let value = deserializer.deserialize_any(JsonSelectVisitor {
            node: self.node,
            state: Rc::clone(&self.state),
            encode_current,
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

struct JsonSelectVisitor {
    node: Arc<JsonSelectNode>,
    state: Rc<RefCell<JsonSelectState>>,
    encode_current: bool,
    limit: usize,
}

impl JsonSelectVisitor {
    fn scalar(self, captured: JsonScanValue, encoded: &[u8]) -> JsonSelectValue {
        JsonSelectValue {
            encoded: if self.encode_current {
                BoundedJson::from_bytes(self.limit, encoded)
            } else {
                BoundedJson::new(self.limit)
            },
            captured,
        }
    }

    fn finish_container(self, encoded: BoundedJson) -> JsonSelectValue {
        let captured = if self.node.capture.is_some() {
            encoded.to_scan_value()
        } else {
            JsonScanValue::Missing
        };
        JsonSelectValue { encoded, captured }
    }
}

impl<'de> Visitor<'de> for JsonSelectVisitor {
    type Value = JsonSelectValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a valid JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        let encoded = if value { b"true" as &[u8] } else { b"false" };
        Ok(self.scalar(JsonScanValue::Value(Value::Bool(value)), encoded))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        let encoded = value.to_string();
        Ok(self.scalar(JsonScanValue::Value(Value::Int(value)), encoded.as_bytes()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
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
        let captured = if value.len() > self.limit {
            JsonScanValue::TooLong
        } else {
            JsonScanValue::Value(Value::Str(Arc::from(value)))
        };
        let encoded = if self.encode_current {
            serde_json::to_vec(value).map_err(E::custom)?
        } else {
            Vec::new()
        };
        Ok(JsonSelectValue {
            encoded: BoundedJson::from_bytes(self.limit, &encoded),
            captured,
        })
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
        if self.encode_current {
            output.push(b"[");
        }
        let mut index = 0u64;
        let mut first = true;
        loop {
            let selected = self.node.indices.get(&index).cloned();
            let value = match selected {
                Some(node) => {
                    self.state.borrow_mut().reset(&node.descendant_captures);
                    seq.next_element_seed(JsonSelectSeed {
                        node,
                        state: Rc::clone(&self.state),
                        emit_to_parent: self.encode_current,
                        limit: self.limit,
                    })?
                }
                None if self.encode_current => seq.next_element_seed(BoundedSeed { limit: self.limit })?,
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
            if self.encode_current {
                if !first {
                    output.push(b",");
                }
                first = false;
                output.push_value(value);
            }
            index = index.saturating_add(1);
        }
        if self.encode_current {
            output.push(b"]");
        }
        Ok(self.finish_container(output))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut output = BoundedMapAccumulator::new(self.limit);
        let reject_duplicates = self.state.borrow().duplicate_policy == JsonDuplicatePolicy::RejectSelected;
        let mut selected_keys = reject_duplicates.then(std::collections::HashSet::new);
        while let Some(key) = map.next_key::<String>()? {
            let selected = self.node.members.get(&key).cloned();
            let retain_for_parent = self.encode_current && output.accepts_candidates();
            let value = match selected {
                Some(node) => {
                    if selected_keys
                        .as_mut()
                        .is_some_and(|selected_keys| !selected_keys.insert(key.clone()))
                    {
                        self.state.borrow_mut().reject(&node.descendant_captures);
                        map.next_value::<IgnoredAny>()?;
                        BoundedJson::new(self.limit)
                    } else {
                        self.state.borrow_mut().reset(&node.descendant_captures);
                        map.next_value_seed(JsonSelectSeed {
                            node,
                            state: Rc::clone(&self.state),
                            emit_to_parent: retain_for_parent,
                            limit: self.limit,
                        })?
                    }
                }
                None if retain_for_parent => map.next_value_seed(BoundedSeed { limit: self.limit })?,
                None => {
                    map.next_value::<IgnoredAny>()?;
                    BoundedJson::new(self.limit)
                }
            };
            if retain_for_parent {
                output.insert(key, value).map_err(A::Error::custom)?;
            }
        }

        let encoded = if self.encode_current {
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
        while let Some(value) = seq.next_element_seed(BoundedSeed { limit: self.limit })? {
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
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(BoundedJson::from_bytes(self.limit, value.to_string().as_bytes()))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(BoundedJson::from_bytes(self.limit, value.to_string().as_bytes()))
    }
    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(BoundedJson::from_bytes(self.limit, json_float_string(value).as_bytes()))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        if value.len() > self.limit {
            return Ok(BoundedJson::oversized(self.limit));
        }
        Ok(BoundedJson::from_bytes(
            self.limit,
            &serde_json::to_vec(value).map_err(E::custom)?,
        ))
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
        while let Some(value) = seq.next_element_seed(BoundedSeed { limit: self.limit })? {
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
    while let Some(key) = map.next_key::<String>()? {
        if !output.accepts_candidates() {
            map.next_value::<IgnoredAny>()?;
            continue;
        }
        let value = map.next_value_seed(BoundedSeed { limit })?;
        output.insert(key, value).map_err(A::Error::custom)?;
    }
    output.finish().map_err(A::Error::custom)
}

struct BoundedMapAccumulator {
    entries: BTreeMap<String, BoundedJson>,
    rendered_len: usize,
    limit: usize,
    overflow_key: Option<String>,
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

    fn insert(&mut self, key: String, value: BoundedJson) -> Result<(), serde_json::Error> {
        if self.irreversible_oversize {
            return Ok(());
        }

        let encoded_key_len = serde_json::to_vec(&key)?.len();
        if self.overflow_key.as_deref() == Some(key.as_str()) {
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

    fn record_overflow(&mut self, key: String, encoded_key_len: usize) {
        if encoded_key_len > self.limit
            || self
                .overflow_key
                .as_deref()
                .is_some_and(|overflow_key| overflow_key != key)
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
            output.push(&serde_json::to_vec(&key)?);
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

    fn push_value(&mut self, value: Self) {
        if value.oversized {
            self.bytes.clear();
            self.oversized = true;
        } else {
            self.push(&value.bytes);
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
            match String::from_utf8(self.bytes.clone()) {
                Ok(value) => JsonScanValue::Value(Value::Str(value.into())),
                Err(_) => JsonScanValue::Missing,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dfa(pattern: &str) -> Arc<dense::DFA<Vec<u32>>> {
        Arc::new(dense::Builder::new().build(pattern).unwrap())
    }

    #[test]
    fn regex_stops_after_unanchored_match() {
        let mut scanner = StreamingRegexScanner::new(dfa("needle")).unwrap();
        assert_eq!(scanner.push("prefix nee").unwrap(), ScanControl::Continue);
        assert_eq!(scanner.push("dle suffix").unwrap(), ScanControl::Stop);
        assert!(scanner.finish().unwrap());
    }

    #[test]
    fn regex_eoi_resolves_end_anchor() {
        let mut scanner = StreamingRegexScanner::new(dfa("needle$")).unwrap();
        assert_eq!(scanner.push("needle").unwrap(), ScanControl::Continue);
        assert!(scanner.finish().unwrap());

        let mut scanner = StreamingRegexScanner::new(dfa("needle$")).unwrap();
        assert_eq!(scanner.push("needle!").unwrap(), ScanControl::Continue);
        assert!(!scanner.finish().unwrap());
    }

    #[test]
    fn regex_empty_pattern_matches() {
        let scanner = StreamingRegexScanner::new(dfa("")).unwrap();
        assert!(scanner.finish().unwrap());
    }

    async fn scan_json(input: &[&[u8]], path: &str, limit: usize) -> JsonScanValue {
        let path = DefiniteJsonPath::parse(path).unwrap();
        let mut scanner = JsonPathScanner::new(path, limit).await.unwrap();
        for chunk in input {
            if scanner.push(chunk).unwrap() == ScanControl::Stop {
                break;
            }
        }
        scanner.finish().unwrap()
    }

    async fn scan_json_select(
        input: &[&[u8]],
        entries: &[(&str, &str)],
        limit: usize,
    ) -> (Arc<JsonSelectPlan>, Vec<JsonScanValue>) {
        let plan = Arc::new(JsonSelectPlan::new(entries.iter().map(|(name, path)| {
            (
                (*name).to_string(),
                DefiniteJsonPath::parse(path).expect("test path should be valid"),
            )
        })));
        let mut scanner = JsonSelectScanner::new(Arc::clone(&plan), limit).await.unwrap();
        for chunk in input {
            if scanner.push(chunk).unwrap() == ScanControl::Stop {
                break;
            }
        }
        let values = scanner.finish().unwrap();
        (plan, values)
    }

    fn selected<'a>(plan: &JsonSelectPlan, values: &'a [JsonScanValue], name: &str) -> &'a JsonScanValue {
        let (_, capture) = plan
            .outputs()
            .iter()
            .find(|(output, _)| output.as_ref() == name)
            .expect("output should exist");
        &values[*capture]
    }

    #[tokio::test]
    async fn json_selects_across_chunks_and_last_duplicate_wins() {
        let result = scan_json(
            &[br#"{"a":{"b":"old"},"a":{"b":"ne"# as &[u8], br#"w"},"tail":true}"#],
            "$.a.b",
            32,
        )
        .await;
        assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == "new"));
    }

    #[tokio::test]
    async fn oversized_earlier_duplicate_does_not_mask_later_value() {
        let result = scan_json(&[br#"{"a":"far too large","a":"ok"}"#], "$.a", 2).await;
        assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == "ok"));
    }

    #[tokio::test]
    async fn json_float_scalar_preserves_json_number_format() {
        for (input, expected) in [(br#"{"n":1.0}"# as &[u8], "1.0"), (br#"{"n":-0.0}"# as &[u8], "-0.0")] {
            let result = scan_json(&[input], "$.n", 16).await;
            assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == expected));
        }
    }

    #[tokio::test]
    async fn selected_json_preserves_float_number_format() {
        let result = scan_json(&[br#"{"selected":{"n":1.0,"z":-0.0}}"#], "$.selected", 32).await;
        assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == r#"{"n":1.0,"z":-0.0}"#));
    }

    #[tokio::test]
    async fn nested_duplicate_path_uses_last_value() {
        let result = scan_json(&[br#"{"a":{"b":"far too large","b":"ok"}}"#], "$.a.b", 2).await;
        assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == "ok"));
    }

    #[tokio::test]
    async fn selected_object_collapses_duplicates_last_wins() {
        let result = scan_json(
            &[br#"{"selected":{"k":"far too large","k":"ok","a":1}}"#],
            "$.selected",
            20,
        )
        .await;
        assert!(matches!(result, JsonScanValue::Value(Value::Str(value)) if &*value == r#"{"a":1,"k":"ok"}"#));
    }

    #[tokio::test]
    async fn selected_object_does_not_replay_two_distinct_overflowed_keys() {
        let result = scan_json(
            &[br#"{"selected":{"a":"far too large for limit","b":"also far too large","a":"x","b":"y"}}"#],
            "$.selected",
            20,
        )
        .await;
        assert!(matches!(result, JsonScanValue::TooLong));
    }

    #[tokio::test]
    async fn invalid_json_returns_missing() {
        assert!(matches!(
            scan_json(&[br#"{"a":1} trailing"#], "$.a", 16).await,
            JsonScanValue::Missing
        ));
    }

    #[tokio::test]
    async fn json_select_shares_prefixes_and_duplicate_paths_across_chunks() {
        let (plan, values) = scan_json_select(
            &[
                br#"{"user":{"id":7,"na"# as &[u8],
                br#"me":"alice"},"items":[true,false]}"#,
            ],
            &[
                ("user", "$.user"),
                ("id", "$.user.id"),
                ("id_alias", "$.user.id"),
                ("second", "$.items[1]"),
            ],
            128,
        )
        .await;

        assert!(matches!(
            selected(&plan, &values, "user"),
            JsonScanValue::Value(Value::Str(value)) if &**value == r#"{"id":7,"name":"alice"}"#
        ));
        assert!(matches!(
            selected(&plan, &values, "id"),
            JsonScanValue::Value(Value::Int(7))
        ));
        assert!(matches!(
            selected(&plan, &values, "id_alias"),
            JsonScanValue::Value(Value::Int(7))
        ));
        assert!(matches!(
            selected(&plan, &values, "second"),
            JsonScanValue::Value(Value::Bool(false))
        ));
    }

    #[tokio::test]
    async fn json_select_root_and_descendant_share_one_parse() {
        let (plan, values) =
            scan_json_select(&[br#"{"id":7,"name":"alice"}"#], &[("root", "$"), ("id", "$.id")], 64).await;

        assert!(matches!(
            selected(&plan, &values, "root"),
            JsonScanValue::Value(Value::Str(value)) if &**value == r#"{"id":7,"name":"alice"}"#
        ));
        assert!(matches!(
            selected(&plan, &values, "id"),
            JsonScanValue::Value(Value::Int(7))
        ));
    }

    #[tokio::test]
    async fn json_select_preserves_existing_non_i64_number_conversion() {
        let (plan, values) = scan_json_select(
            &[br#"{"float":1.5,"big":18446744073709551615}"#],
            &[("float", "$.float"), ("big", "$.big")],
            64,
        )
        .await;

        assert!(matches!(
            selected(&plan, &values, "float"),
            JsonScanValue::Value(Value::Str(value)) if &**value == "1.5"
        ));
        assert!(matches!(
            selected(&plan, &values, "big"),
            JsonScanValue::Value(Value::Str(value)) if &**value == "18446744073709551615"
        ));
    }

    #[tokio::test]
    async fn oversized_parent_still_scans_selected_descendants() {
        let (plan, values) = scan_json_select(
            &[br#"{"large":{"padding":"too long","target":7}}"#],
            &[("large", "$.large"), ("target", "$.large.target")],
            8,
        )
        .await;

        assert!(matches!(selected(&plan, &values, "large"), JsonScanValue::TooLong));
        assert!(matches!(
            selected(&plan, &values, "target"),
            JsonScanValue::Value(Value::Int(7))
        ));
    }

    #[test]
    fn bounded_map_caps_retained_entries_when_the_aggregate_overflows() {
        let mut output = BoundedMapAccumulator::new(24);
        output
            .insert("first".to_string(), BoundedJson::from_bytes(24, br#""12345678""#))
            .unwrap();
        assert!(!output.entries.is_empty());

        output
            .insert("second".to_string(), BoundedJson::from_bytes(24, br#""12345678""#))
            .unwrap();
        assert!(output.is_oversized());
        assert!(output.rendered_len <= output.limit);
        assert!(output.entries.values().map(|value| value.bytes.len()).sum::<usize>() <= output.limit);
        assert!(output.finish().unwrap().oversized);
    }

    #[test]
    fn bounded_map_replacement_updates_rendered_length_and_commas() {
        let mut output = BoundedMapAccumulator::new(64);
        output
            .insert("a".to_string(), BoundedJson::from_bytes(64, b"1"))
            .unwrap();
        output
            .insert("b".to_string(), BoundedJson::from_bytes(64, b"2"))
            .unwrap();
        output
            .insert("a".to_string(), BoundedJson::from_bytes(64, b"333"))
            .unwrap();

        let output = output.finish().unwrap();
        assert_eq!(String::from_utf8(output.bytes).unwrap(), r#"{"a":333,"b":2}"#);
    }

    #[test]
    fn bounded_map_overflow_marker_recovers_only_for_the_same_key() {
        let mut recovered = BoundedMapAccumulator::new(20);
        recovered
            .insert("key".to_string(), BoundedJson::from_bytes(20, &[b'x'; 21]))
            .unwrap();
        assert!(recovered.is_oversized());
        recovered
            .insert("key".to_string(), BoundedJson::from_bytes(20, br#""ok""#))
            .unwrap();
        assert!(!recovered.is_oversized());
        assert_eq!(
            String::from_utf8(recovered.finish().unwrap().bytes).unwrap(),
            r#"{"key":"ok"}"#
        );

        let mut irreversible = BoundedMapAccumulator::new(20);
        irreversible
            .insert("first".to_string(), BoundedJson::from_bytes(20, &[b'x'; 21]))
            .unwrap();
        irreversible
            .insert("second".to_string(), BoundedJson::from_bytes(20, &[b'y'; 21]))
            .unwrap();
        assert!(irreversible.irreversible_oversize);
        irreversible
            .insert("first".to_string(), BoundedJson::from_bytes(20, b"1"))
            .unwrap();
        assert!(irreversible.entries.is_empty());
    }

    #[test]
    fn bounded_map_overlong_key_is_irreversible() {
        let mut output = BoundedMapAccumulator::new(8);
        output
            .insert("key-too-long".to_string(), BoundedJson::from_bytes(8, b"1"))
            .unwrap();
        assert!(output.irreversible_oversize);
        assert!(output.entries.is_empty());
    }

    #[tokio::test]
    async fn json_select_duplicate_parent_replaces_the_complete_subtree() {
        let (plan, values) = scan_json_select(
            &[br#"{"a":{"b":"old"},"a":{}}"#],
            &[("parent", "$.a"), ("child", "$.a.b")],
            64,
        )
        .await;

        assert!(matches!(
            selected(&plan, &values, "parent"),
            JsonScanValue::Value(Value::Str(value)) if &**value == "{}"
        ));
        assert!(matches!(selected(&plan, &values, "child"), JsonScanValue::Missing));
    }

    #[tokio::test]
    async fn json_select_later_duplicate_can_replace_oversized_same_capture() {
        let (plan, values) = scan_json_select(&[br#"{"a":"far too large","a":"ok"}"#], &[("value", "$.a")], 2).await;
        assert!(matches!(
            selected(&plan, &values, "value"),
            JsonScanValue::Value(Value::Str(value)) if &**value == "ok"
        ));
    }

    #[tokio::test]
    async fn json_select_online_aggregate_overflow_is_not_replayed() {
        let (plan, values) = scan_json_select(
            &[br#"{"a":"1234","b":"1234","a":""}"#],
            &[("a", "$.a"), ("b", "$.b")],
            5,
        )
        .await;

        assert!(matches!(
            selected(&plan, &values, "a"),
            JsonScanValue::Value(Value::Str(value)) if value.is_empty()
        ));
        assert!(matches!(selected(&plan, &values, "b"), JsonScanValue::TooLong));
    }

    #[tokio::test]
    async fn invalid_json_discards_every_json_select_capture() {
        let (plan, values) = scan_json_select(&[br#"{"a":1,"b":2} trailing"#], &[("a", "$.a"), ("b", "$.b")], 64).await;
        assert!(matches!(selected(&plan, &values, "a"), JsonScanValue::Missing));
        assert!(matches!(selected(&plan, &values, "b"), JsonScanValue::Missing));
    }
}
