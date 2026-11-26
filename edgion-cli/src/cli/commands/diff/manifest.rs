//! Read scalar mapping keys as strings, matching the Controller's source reader.
//! JSON's default map visitor overwrites duplicate keys; reject them at every depth.
use serde::de::{Error, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::fmt;

pub(super) struct Manifest(pub Value);

impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ManifestVisitor).map(Self)
    }
}

struct ManifestVisitor;

impl<'de> Visitor<'de> for ManifestVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON-compatible manifest")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Value, A::Error> {
        let mut map = Map::new();
        while let Some(key) = entries.next_key::<String>()? {
            if map.contains_key(&key) {
                return Err(A::Error::custom("duplicate manifest key"));
            }
            map.insert(key, entries.next_value::<Manifest>()?.0);
        }
        Ok(Value::Object(map))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut elements: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(Manifest(value)) = elements.next_element()? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E>(self, value: &str) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
}
