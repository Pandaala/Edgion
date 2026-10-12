use super::{GraphqlError, MAX_ENVELOPE_BYTES};
use crate::resources::edgion_plugins::plugin_configs::parse_json_instance_with_limits;
use serde_json::{Map, Value};

pub(super) struct Envelope<'a> {
    pub(super) query: &'a str,
    pub(super) operation: Option<&'a str>,
}
pub(super) fn envelope(value: &Value) -> Result<Envelope<'_>, GraphqlError> {
    let object = value.as_object().ok_or(GraphqlError::Invalid)?;
    if object
        .keys()
        .any(|k| !matches!(k.as_str(), "query" | "operationName" | "variables" | "extensions"))
    {
        return Err(GraphqlError::Invalid);
    }
    let query = object
        .get("query")
        .and_then(Value::as_str)
        .filter(|q| !q.trim().is_empty())
        .ok_or(GraphqlError::Invalid)?;
    let operation = match object.get("operationName") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        _ => return Err(GraphqlError::Invalid),
    };
    let _variables = match object.get("variables") {
        None | Some(Value::Null) => None,
        Some(Value::Object(m)) => Some(m),
        _ => return Err(GraphqlError::Invalid),
    };
    match object.get("extensions") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) if m.is_empty() => {}
        _ => return Err(GraphqlError::Invalid),
    }
    Ok(Envelope { query, operation })
}
fn strict_json(bytes: &[u8]) -> Result<Value, GraphqlError> {
    parse_json_instance_with_limits(bytes, 32, 4096)
        .map(|p| p.into_value())
        .map_err(|_| GraphqlError::Invalid)
}
pub(crate) fn decode(source: &str, remaining: &mut usize) -> Result<String, GraphqlError> {
    let source = source.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < source.len() {
        if *remaining == 0 {
            return Err(GraphqlError::TooLarge);
        }
        let byte = match source[i] {
            b'+' => b' ',
            b'%' => {
                fn hex(b: u8) -> Option<u8> {
                    match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        b'A'..=b'F' => Some(b - b'A' + 10),
                        _ => None,
                    }
                }
                let a = source.get(i + 1).and_then(|b| hex(*b)).ok_or(GraphqlError::Invalid)?;
                let b = source.get(i + 2).and_then(|b| hex(*b)).ok_or(GraphqlError::Invalid)?;
                i += 2;
                a * 16 + b
            }
            b => b,
        };
        result.push(byte);
        *remaining -= 1;
        i += 1;
    }
    String::from_utf8(result).map_err(|_| GraphqlError::Invalid)
}

pub(super) fn post(bytes: &[u8], limit: usize) -> Result<Value, GraphqlError> {
    if bytes.len() > limit.min(MAX_ENVELOPE_BYTES) {
        return Err(GraphqlError::TooLarge);
    }
    let value = strict_json(bytes)?;
    Ok(value)
}
pub(super) fn get(query: &str, limit: usize) -> Result<Value, GraphqlError> {
    let limit = limit.min(MAX_ENVELOPE_BYTES);
    if query.len() > limit {
        return Err(GraphqlError::TooLarge);
    }
    let mut remaining = limit;
    let mut params = Map::new();
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or(GraphqlError::Invalid)?;
        let key = decode(key, &mut remaining)?;
        if !matches!(key.as_str(), "query" | "operationName" | "variables" | "extensions") || params.contains_key(&key)
        {
            return Err(GraphqlError::Invalid);
        }
        let value = decode(value, &mut remaining)?;
        let value = if matches!(key.as_str(), "variables" | "extensions") {
            strict_json(value.as_bytes())?
        } else {
            Value::String(value)
        };
        params.insert(key, value);
    }
    let value = Value::Object(params);
    Ok(value)
}
