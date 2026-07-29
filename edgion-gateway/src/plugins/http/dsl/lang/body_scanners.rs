use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::OnceLock;
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use edgion_dsl::json_path::{DefiniteJsonPath, JsonPathSegment};
use regex_automata::dfa::{dense, Automaton};
use regex_automata::util::primitives::StateID;
use regex_automata::Input;
use serde::de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::plugins::runtime::traits::session::BodyReadError;
use crate::request_body::{BodyOperationError, RawBodyScanner, ScanControl, TextBodyScanner};

use super::value::Value;

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

#[derive(Debug)]
pub(crate) enum JsonScanValue {
    Value(Value),
    Missing,
    TooLong,
}

pub(crate) struct JsonPathScanner {
    sender: Option<mpsc::SyncSender<Vec<u8>>>,
    worker: Option<JoinHandle<JsonScanValue>>,
    disconnected: bool,
}

impl JsonPathScanner {
    pub(crate) async fn new(path: DefiniteJsonPath, limit: usize) -> Result<Self, BodyOperationError> {
        static JSON_SCAN_LIMITER: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let permit = Arc::clone(JSON_SCAN_LIMITER.get_or_init(|| Arc::new(Semaphore::new(32))))
            .acquire_owned()
            .await
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        let (sender, receiver) = mpsc::sync_channel(2);
        let segments: Arc<[JsonPathSegment]> = Arc::from(path.segments().to_vec());
        let worker = std::thread::Builder::new()
            .name("edgion-dsl-json".to_string())
            .spawn(move || parse_json_stream(receiver, segments, limit, permit))
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            disconnected: false,
        })
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
    let mut entries = BTreeMap::<String, (Vec<u8>, BoundedJson)>::new();
    let mut minimum_len = 2usize;
    let mut irreversible_oversize = minimum_len > limit;
    while let Some(key) = map.next_key::<String>()? {
        if irreversible_oversize {
            map.next_value::<IgnoredAny>()?;
            continue;
        }
        let encoded_key = if let Some((encoded, _)) = entries.get(&key) {
            encoded.clone()
        } else {
            if key.len() > limit {
                irreversible_oversize = true;
                entries.clear();
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            let encoded = serde_json::to_vec(&key).map_err(A::Error::custom)?;
            let separator = usize::from(!entries.is_empty());
            let added_minimum = encoded.len().checked_add(2 + separator).unwrap_or(usize::MAX);
            minimum_len = minimum_len.checked_add(added_minimum).unwrap_or(usize::MAX);
            if minimum_len > limit {
                irreversible_oversize = true;
                entries.clear();
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            encoded
        };
        let value = map.next_value_seed(BoundedSeed { limit })?;
        entries.insert(key, (encoded_key, value));
    }
    if irreversible_oversize {
        return Ok(BoundedJson::oversized(limit));
    }
    let mut output = BoundedJson::new(limit);
    output.push(b"{");
    for (index, (_, (encoded_key, value))) in entries.into_iter().enumerate() {
        if index != 0 {
            output.push(b",");
        }
        output.push(&encoded_key);
        output.push(b":");
        output.push_value(value);
    }
    output.push(b"}");
    Ok(output)
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
    async fn invalid_json_returns_missing() {
        assert!(matches!(
            scan_json(&[br#"{"a":1} trailing"#], "$.a", 16).await,
            JsonScanValue::Missing
        ));
    }
}
