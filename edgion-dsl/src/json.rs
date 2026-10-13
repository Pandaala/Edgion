//! Canonical strict JSON policy with DOM and source-preserving representations.

use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde::Deserializer;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictJsonError {
    TooLarge,
    Malformed,
    DuplicateKey,
    DepthLimit,
    NodeLimit,
}
impl fmt::Display for StrictJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StrictJsonError {}

#[derive(Clone, Copy)]
pub struct JsonLimits {
    pub bytes: usize,
    pub depth: usize,
    pub nodes: u64,
}

pub fn parse_dom(bytes: &[u8], limits: JsonLimits) -> Result<ParsedJsonInstance, StrictJsonError> {
    if bytes.len() > limits.bytes {
        return Err(StrictJsonError::TooLarge);
    }
    let mut state = ParseState {
        max_depth: limits.depth,
        max_nodes: limits.nodes,
        ..ParseState::default()
    };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = ValueSeed {
        state: &mut state,
        depth: 1,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| state.failure.unwrap_or(StrictJsonError::Malformed))?;
    deserializer
        .end()
        .map_err(|_| state.failure.unwrap_or(StrictJsonError::Malformed))?;
    Ok(ParsedJsonInstance {
        value,
        node_count: state.node_count,
        text_bytes: state.text_bytes,
    })
}
/// Strictly parsed JSON plus the accounting needed by the work budget.
#[derive(Debug)]
pub struct ParsedJsonInstance {
    value: Value,
    node_count: u64,
    text_bytes: u64,
}

impl ParsedJsonInstance {
    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn node_count(&self) -> u64 {
        self.node_count
    }

    pub fn text_bytes(&self) -> u64 {
        self.text_bytes
    }

    pub fn into_value(self) -> Value {
        self.value
    }
}

#[derive(Default)]
struct ParseState {
    max_depth: usize,
    max_nodes: u64,
    node_count: u64,
    text_bytes: u64,
    failure: Option<StrictJsonError>,
}

struct ValueSeed<'a> {
    state: &'a mut ParseState,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for ValueSeed<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        self.state.enter(self.depth).map_err(D::Error::custom)?;
        deserializer.deserialize_any(ValueVisitor {
            state: self.state,
            depth: self.depth,
        })
    }
}

struct ValueVisitor<'a> {
    state: &'a mut ParseState,
    depth: usize,
}

impl<'de> Visitor<'de> for ValueVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("one JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        self.state.text_bytes = self.state.text_bytes.saturating_add(value.len() as u64);
        Ok(Value::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        self.state.text_bytes = self.state.text_bytes.saturating_add(value.len() as u64);
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(self.state.max_nodes as usize));
        while let Some(value) = seq.next_element_seed(ValueSeed {
            state: self.state,
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let capacity = map.size_hint().unwrap_or(0).min(self.state.max_nodes as usize);
        let mut values = Map::with_capacity(capacity);
        while let Some(key) = map.next_key::<String>()? {
            self.state.text_bytes = self.state.text_bytes.saturating_add(key.len() as u64);
            let entry = values.entry(key);
            self.state
                .admit_key(matches!(&entry, serde_json::map::Entry::Vacant(_)))
                .map_err(A::Error::custom)?;
            if let serde_json::map::Entry::Vacant(entry) = entry {
                let value = map.next_value_seed(ValueSeed {
                    state: self.state,
                    depth: self.depth + 1,
                })?;
                entry.insert(value);
            }
        }
        Ok(Value::Object(values))
    }
}

impl ParseState {
    fn fail(&mut self, error: StrictJsonError) -> StrictJsonError {
        self.failure = Some(error);
        error
    }
    fn enter(&mut self, depth: usize) -> Result<(), StrictJsonError> {
        if depth > self.max_depth {
            return Err(self.fail(StrictJsonError::DepthLimit));
        }
        self.node_count = self.node_count.saturating_add(1);
        if self.node_count > self.max_nodes {
            return Err(self.fail(StrictJsonError::NodeLimit));
        }
        Ok(())
    }
    fn admit_key(&mut self, is_new: bool) -> Result<(), StrictJsonError> {
        if !is_new {
            return Err(self.fail(StrictJsonError::DuplicateKey));
        }
        Ok(())
    }
}

use crate::error::RuntimeError;
use crate::value::ValueWorkBudget;
use serde_json::value::RawValue;
use std::io::{self, Write};
use std::sync::Arc;

pub const MAX_DSL_JSON_BYTES: usize = 64 * 1024;
pub const DSL_JSON_DEPTH: usize = 32;
pub const DSL_JSON_NODES: u64 = 4096;

#[derive(Debug)]
pub enum JsonError {
    Strict(StrictJsonError),
    Work(RuntimeError),
    Allocation,
    OutputLimit { limit: usize },
    InvalidOperation,
}

impl From<StrictJsonError> for JsonError {
    fn from(value: StrictJsonError) -> Self {
        Self::Strict(value)
    }
}
impl From<RuntimeError> for JsonError {
    fn from(value: RuntimeError) -> Self {
        Self::Work(value)
    }
}

/// The source stays caller-owned. Only decoded top-level member names are owned.
pub struct RawDocument<'a> {
    raw: &'a RawValue,
    members: Option<Vec<(String, &'a RawValue)>>,
}
impl<'a> RawDocument<'a> {
    pub fn source(&self) -> &'a str {
        self.raw.get()
    }
    pub fn members(&self) -> Option<&[(String, &'a RawValue)]> {
        self.members.as_deref()
    }
    pub fn get(&self, key: &str) -> Option<&'a str> {
        self.members
            .as_ref()?
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.get())
    }
}

/// The charge precedes serde traversal and its escaped-token scratch allocation.
fn reserve_parse(source: &str, work: &mut ValueWorkBudget) -> Result<(), JsonError> {
    work.charge((source.len() as u64).saturating_mul(8))?;
    Ok(())
}

fn reserve_vec<T>(values: &mut Vec<T>, work: &mut ValueWorkBudget) -> Result<(), JsonError> {
    if values.len() == values.capacity() {
        let target = values.capacity().saturating_mul(2).max(4);
        work.charge(
            (target as u64)
                .saturating_mul(std::mem::size_of::<T>() as u64)
                .saturating_mul(2),
        )?;
        values
            .try_reserve_exact(target - values.len())
            .map_err(|_| JsonError::Allocation)?;
    }
    Ok(())
}

struct RawState<'w> {
    policy: ParseState,
    work: &'w mut ValueWorkBudget,
    error: Option<JsonError>,
}
impl RawState<'_> {
    fn charge(&mut self, amount: u64) -> Result<(), JsonError> {
        self.work.charge(amount).map_err(JsonError::Work)
    }
    fn serde_error<E: serde::de::Error>(&mut self, error: JsonError) -> E {
        self.error = Some(error);
        E::custom("bounded JSON operation failed")
    }
    fn admit_key(&mut self, keys: &mut HashSet<String>, key: &str) -> Result<(), JsonError> {
        self.charge(128 + (key.len() as u64).saturating_mul(2))?;
        if keys.len() == keys.capacity() {
            let target = keys.capacity().saturating_mul(2).max(4);
            self.charge(
                (target as u64)
                    .saturating_mul((std::mem::size_of::<String>() + 32) as u64)
                    .saturating_mul(2),
            )?;
            keys.try_reserve(1).map_err(|_| JsonError::Allocation)?;
        }
        self.policy
            .admit_key(keys.insert(key.to_owned()))
            .map_err(JsonError::Strict)
    }
}

struct KeySeed<'s, 'w>(&'s mut RawState<'w>);
impl<'de> DeserializeSeed<'de> for KeySeed<'_, '_> {
    type Value = String;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<String, D::Error> {
        deserializer.deserialize_str(KeyVisitor(self.0))
    }
}
struct KeyVisitor<'s, 'w>(&'s mut RawState<'w>);
impl<'de> Visitor<'de> for KeyVisitor<'_, '_> {
    type Value = String;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON member name")
    }
    fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<String, E> {
        self.0
            .charge((key.len() as u64).saturating_mul(2))
            .map_err(|error| self.0.serde_error(error))?;
        Ok(key.to_owned())
    }
}

struct RawVisitor<'s, 'w> {
    state: &'s mut RawState<'w>,
    depth: usize,
    retain: bool,
}
impl<'de> Visitor<'de> for RawVisitor<'_, '_> {
    type Value = Option<Vec<(String, &'de RawValue)>>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON container")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut keys = HashSet::new();
        let mut members = Vec::new();
        while let Some(key) = map.next_key_seed(KeySeed(self.state))? {
            self.state
                .admit_key(&mut keys, &key)
                .map_err(|error| self.state.serde_error(error))?;
            let value: &'de RawValue = map.next_value()?;
            raw_node(value, self.depth + 1, false, self.state).map_err(|error| self.state.serde_error(error))?;
            if self.retain {
                reserve_vec(&mut members, self.state.work).map_err(|error| self.state.serde_error(error))?;
                members.push((key, value));
            }
        }
        Ok(self.retain.then_some(members))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while let Some(value) = seq.next_element::<&'de RawValue>()? {
            raw_node(value, self.depth + 1, false, self.state).map_err(|error| self.state.serde_error(error))?;
        }
        Ok(None)
    }
}

struct ValidString;
impl<'de> Visitor<'de> for ValidString {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a valid JSON string")
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
}

fn raw_node<'de>(
    raw: &'de RawValue,
    depth: usize,
    retain: bool,
    state: &mut RawState<'_>,
) -> Result<Option<Vec<(String, &'de RawValue)>>, JsonError> {
    state.policy.enter(depth)?;
    state.charge(128)?;
    let source = raw.get();
    match source.as_bytes().first() {
        Some(b'"') => {
            // RawValue preserves spelling but does not validate escaped Unicode.
            // Strict string deserialization checks surrogate pairs without owning
            // a decoded string; scratch is reserved before serde runs.
            reserve_parse(source, state.work)?;
            let mut de = serde_json::Deserializer::from_str(source);
            de.deserialize_str(ValidString)
                .map_err(|_| JsonError::Strict(StrictJsonError::Malformed))?;
            Ok(None)
        }
        Some(b'{') | Some(b'[') => {
            reserve_parse(source, state.work)?;
            let mut de = serde_json::Deserializer::from_str(source);
            let visitor = RawVisitor { state, depth, retain };
            let result = if source.starts_with('{') {
                de.deserialize_map(visitor)
            } else {
                de.deserialize_seq(visitor)
            };
            result.map_err(|_| {
                state
                    .error
                    .take()
                    .unwrap_or(JsonError::Strict(StrictJsonError::Malformed))
            })
        }
        _ => Ok(None),
    }
}

pub fn parse_raw<'a>(source: &'a str, limit: usize, work: &mut ValueWorkBudget) -> Result<RawDocument<'a>, JsonError> {
    let limit = limit.min(MAX_DSL_JSON_BYTES);
    if source.len() > limit {
        return Err(JsonError::OutputLimit { limit });
    }
    reserve_parse(source, work)?;
    let raw: &'a RawValue = serde_json::from_str(source).map_err(|_| JsonError::Strict(StrictJsonError::Malformed))?;
    let mut state = RawState {
        policy: ParseState {
            max_depth: DSL_JSON_DEPTH,
            max_nodes: DSL_JSON_NODES,
            ..ParseState::default()
        },
        work,
        error: None,
    };
    let members = raw_node(raw, 1, true, &mut state)?;
    Ok(RawDocument { raw, members })
}

pub fn copy_raw(source: &str, work: &mut ValueWorkBudget) -> Result<Arc<str>, JsonError> {
    work.charge((source.len() as u64).saturating_mul(2))?;
    Ok(Arc::from(source))
}

pub fn decode_string(source: &str, work: &mut ValueWorkBudget) -> Result<Option<Arc<str>>, JsonError> {
    if !source.starts_with('"') {
        return Ok(None);
    }
    reserve_parse(source, work)?;
    let text: String = serde_json::from_str(source).map_err(|_| JsonError::InvalidOperation)?;
    copy_raw(&text, work).map(Some)
}

struct JsonWriter<'w> {
    bytes: Vec<u8>,
    limit: usize,
    work: &'w mut ValueWorkBudget,
    failure: Option<JsonError>,
}
impl Write for JsonWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = (|| {
            let next = self
                .bytes
                .len()
                .checked_add(bytes.len())
                .ok_or(JsonError::OutputLimit { limit: self.limit })?;
            if next > self.limit {
                return Err(JsonError::OutputLimit { limit: self.limit });
            }
            if next > self.bytes.capacity() {
                let target = next.max(self.bytes.capacity().saturating_mul(2)).min(self.limit);
                self.work.charge((target - self.bytes.capacity()) as u64 * 2)?;
                self.bytes
                    .try_reserve_exact(target - self.bytes.len())
                    .map_err(|_| JsonError::Allocation)?;
            }
            self.work.charge(bytes.len() as u64 * 2)?;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        })();
        result.map_err(|error| {
            self.failure = Some(error);
            io::Error::other("bounded JSON output failed")
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Explicitly encoded raw JSON, or an ordinary string to escape as a JSON value.
pub enum MemberValue<'a> {
    Raw(&'a str),
    String(&'a str),
}

pub fn set_member(
    source: &str,
    key: &str,
    value: MemberValue<'_>,
    limit: usize,
    work: &mut ValueWorkBudget,
) -> Result<Arc<str>, JsonError> {
    let document = parse_raw(source, limit, work)?;
    let members = document.members().ok_or(JsonError::InvalidOperation)?;
    let raw_value = match value {
        MemberValue::Raw(raw) => {
            parse_raw(raw, limit, work)?;
            Some(raw)
        }
        MemberValue::String(_) => None,
    };
    let mut writer = JsonWriter {
        bytes: Vec::new(),
        limit: limit.min(MAX_DSL_JSON_BYTES),
        work,
        failure: None,
    };
    let result = (|| -> io::Result<()> {
        writer.write_all(b"{")?;
        let mut first = true;
        for (name, raw) in members {
            if name == key {
                continue;
            }
            if !first {
                writer.write_all(b",")?;
            }
            first = false;
            serde_json::to_writer(&mut writer, name)?;
            writer.write_all(b":")?;
            writer.write_all(raw.get().as_bytes())?;
        }
        if !first {
            writer.write_all(b",")?;
        }
        serde_json::to_writer(&mut writer, key)?;
        writer.write_all(b":")?;
        match value {
            MemberValue::Raw(_) => writer.write_all(raw_value.unwrap_or("").as_bytes())?,
            MemberValue::String(text) => serde_json::to_writer(&mut writer, text)?,
        }
        writer.write_all(b"}")
    })();
    if result.is_err() {
        return Err(writer.failure.take().unwrap_or(JsonError::Allocation));
    }
    // Construction can increase combined depth/nodes; validate before publication.
    let output = std::str::from_utf8(&writer.bytes).map_err(|_| JsonError::InvalidOperation)?;
    parse_raw(output, writer.limit, writer.work)?;
    copy_raw(output, writer.work)
}

#[cfg(test)]
#[path = "../tests/unit/json_test.rs"]
mod tests;
