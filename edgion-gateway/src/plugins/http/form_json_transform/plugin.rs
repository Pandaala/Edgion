use std::collections::BTreeMap;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};

use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    parse_json_instance, BodyRequirement, FormJsonDirection, FormJsonTransformConfig, StrictJsonError,
    ABSOLUTE_INSTANCE_MAX_BYTES, DEFAULT_INSTANCE_MAX_BYTES,
};
use edgion_resources::EdgionErrorCode;
use serde_json::{Map, Value};

use crate::plugins::http::common::{
    form_request_headers_are_supported, json_request_headers_are_supported, CappedWriter,
};
use crate::plugins::runtime::body_gate::record_body_read_error_status;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::request_body::BodyReadError;

const INVALID_REQUEST_BODY: &str = "Invalid request body";
const UNSUPPORTED_MEDIA_TYPE: &str = "Unsupported media type";
const MAX_PAIRS: usize = 256;
const MAX_KEY_BYTES: usize = 256;
const MAX_VALUE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformFailure {
    BadRequest,
    TooLarge,
    Internal,
}

/// Immutable request-stage form/JSON transformer.
pub struct FormJsonTransform {
    direction: FormJsonDirection,
    body_limit: u64,
}

impl FormJsonTransform {
    pub fn create(
        config: &FormJsonTransformConfig,
        body_requirement: Option<BodyRequirement>,
    ) -> Option<Box<dyn RequestFilter>> {
        let body = body_requirement?;
        if !body.is_fail_close() || body.conditions.is_some() {
            return None;
        }
        let body_limit = body
            .max_body_size_bytes()
            .and_then(Result::ok)
            .unwrap_or(DEFAULT_INSTANCE_MAX_BYTES);
        if body_limit == 0 || body_limit > ABSOLUTE_INSTANCE_MAX_BYTES {
            return None;
        }
        Some(Box::new(Self {
            direction: config.direction,
            body_limit,
        }))
    }

    fn reject(status: u16, body: Option<&'static str>) -> PluginRunningResult {
        PluginRunningResult::ErrResponse {
            status,
            body: body.map(|b| Bytes::from_static(b.as_bytes())),
        }
    }

    fn media_supported(&self, session: &dyn PluginSession) -> bool {
        match self.direction {
            FormJsonDirection::FormToJson => form_request_headers_are_supported(session.request_header_map()),
            FormJsonDirection::JsonToForm => json_request_headers_are_supported(session.request_header_map()),
        }
    }

    fn transform(&self, body: &[u8]) -> Result<Vec<u8>, TransformFailure> {
        match self.direction {
            FormJsonDirection::FormToJson => form_to_json(body, self.body_limit as usize),
            FormJsonDirection::JsonToForm => json_to_form(body, self.body_limit as usize),
        }
    }

    fn output_content_type(&self) -> &'static str {
        match self.direction {
            FormJsonDirection::FormToJson => "application/json",
            FormJsonDirection::JsonToForm => "application/x-www-form-urlencoded",
        }
    }
}

#[async_trait]
impl RequestFilter for FormJsonTransform {
    fn name(&self) -> &str {
        "FormJsonTransform"
    }

    fn requires_materialized_request_body(&self) -> bool {
        false
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if !self.media_supported(session) {
            log.push("deny media type");
            return Self::reject(415, Some(UNSUPPORTED_MEDIA_TYPE));
        }

        let known_bodyless = session.declares_body() == Some(false);
        let body_len = match session.request_body_available_len(known_bodyless) {
            Ok(length) => length,
            Err(error) => {
                record_body_read_error_status(session, &error);
                log.push_err("body length unavailable");
                return Self::reject(500, None);
            }
        };
        if body_len > self.body_limit {
            session.record_error(EdgionErrorCode::BodyReadTooLarge);
            log.push("deny body too large");
            return Self::reject(413, None);
        }

        let original = match session.read_request_body(None).await {
            Ok(body) => body,
            Err(error) => {
                record_body_read_error_status(session, &error);
                return match error {
                    BodyReadError::BufferLimitExceeded { .. } | BodyReadError::TooLarge { .. } => {
                        log.push("deny body too large");
                        Self::reject(413, None)
                    }
                    _ => {
                        log.push_err("body materialization failed");
                        Self::reject(500, None)
                    }
                };
            }
        };
        if original.len() as u64 > self.body_limit {
            session.record_error(EdgionErrorCode::BodyReadTooLarge);
            log.push("deny body too large");
            return Self::reject(413, None);
        }

        let transformed = match self.transform(&original) {
            Ok(body) => Bytes::from(body),
            Err(TransformFailure::BadRequest) => {
                log.push("deny invalid body");
                return Self::reject(400, Some(INVALID_REQUEST_BODY));
            }
            Err(TransformFailure::TooLarge) => {
                session.record_error(EdgionErrorCode::BodyReadTooLarge);
                log.push("deny transform limit");
                return Self::reject(413, None);
            }
            Err(TransformFailure::Internal) => {
                log.push_err("body transform failed");
                return Self::reject(500, None);
            }
        };

        if let Err(error) = session.overwrite_request_body(transformed).await {
            record_body_read_error_status(session, &error);
            return match error {
                BodyReadError::BufferLimitExceeded { .. } | BodyReadError::TooLarge { .. } => {
                    log.push("deny output too large");
                    Self::reject(413, None)
                }
                _ => {
                    log.push_err("body overwrite failed");
                    Self::reject(500, None)
                }
            };
        }
        if session
            .set_request_header("content-type", self.output_content_type())
            .is_err()
        {
            if let Err(error) = session.overwrite_request_body(original).await {
                record_body_read_error_status(session, &error);
                log.push_err("body rollback failed");
            } else {
                log.push_err("content type update failed");
            }
            return Self::reject(500, None);
        }

        log.push("body transformed");
        PluginRunningResult::Nothing
    }
}

fn form_to_json(input: &[u8], output_limit: usize) -> Result<Vec<u8>, TransformFailure> {
    let mut values = BTreeMap::<String, Vec<String>>::new();
    let mut pair_count = 0usize;
    for segment in input.split(|byte| *byte == b'&').filter(|segment| !segment.is_empty()) {
        pair_count = pair_count.checked_add(1).ok_or(TransformFailure::TooLarge)?;
        if pair_count > MAX_PAIRS {
            return Err(TransformFailure::TooLarge);
        }
        let split = segment.iter().position(|byte| *byte == b'=').unwrap_or(segment.len());
        let key = decode_component(&segment[..split], MAX_KEY_BYTES)?;
        let value_start = split.saturating_add(1).min(segment.len());
        let value = decode_component(&segment[value_start..], MAX_VALUE_BYTES)?;
        values.entry(key).or_default().push(value);
    }

    let object = values
        .into_iter()
        .map(|(key, mut values)| {
            let value = if values.len() == 1 {
                Value::String(values.pop().expect("one value"))
            } else {
                Value::Array(values.into_iter().map(Value::String).collect())
            };
            (key, value)
        })
        .collect::<Map<String, Value>>();
    serialize_json(&Value::Object(object), output_limit)
}

fn decode_component(input: &[u8], limit: usize) -> Result<String, TransformFailure> {
    let mut output = Vec::with_capacity(input.len().min(limit));
    let mut index = 0usize;
    while index < input.len() {
        let byte = match input[index] {
            b'+' => {
                index += 1;
                b' '
            }
            b'%' => {
                let high = input.get(index + 1).and_then(|byte| hex_value(*byte));
                let low = input.get(index + 2).and_then(|byte| hex_value(*byte));
                let (Some(high), Some(low)) = (high, low) else {
                    return Err(TransformFailure::BadRequest);
                };
                index += 3;
                (high << 4) | low
            }
            byte => {
                index += 1;
                byte
            }
        };
        if output.len() >= limit {
            return Err(TransformFailure::TooLarge);
        }
        output.push(byte);
    }
    String::from_utf8(output).map_err(|_| TransformFailure::BadRequest)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn serialize_json(value: &Value, limit: usize) -> Result<Vec<u8>, TransformFailure> {
    let mut writer = CappedWriter::new(limit);
    match catch_unwind(AssertUnwindSafe(|| serde_json::to_writer(&mut writer, value))) {
        Ok(Ok(())) => Ok(writer.into_inner()),
        Ok(Err(_)) if writer.overflowed() => Err(TransformFailure::TooLarge),
        Ok(Err(_)) | Err(_) => Err(TransformFailure::Internal),
    }
}

fn json_to_form(input: &[u8], output_limit: usize) -> Result<Vec<u8>, TransformFailure> {
    let parsed = parse_json_instance(input).map_err(|error| match error {
        StrictJsonError::TooLarge => TransformFailure::TooLarge,
        StrictJsonError::Malformed
        | StrictJsonError::DuplicateKey
        | StrictJsonError::DepthLimit
        | StrictJsonError::NodeLimit => TransformFailure::BadRequest,
    })?;
    let Value::Object(object) = parsed.into_value() else {
        return Err(TransformFailure::BadRequest);
    };

    let mut members = object.into_iter().collect::<Vec<_>>();
    members.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    let mut writer = CappedWriter::new(output_limit);
    let mut pair_count = 0usize;
    for (key, value) in members {
        if key.len() > MAX_KEY_BYTES {
            return Err(TransformFailure::TooLarge);
        }
        match value {
            Value::Array(values) if values.is_empty() => return Err(TransformFailure::BadRequest),
            Value::Array(values) => {
                for value in values {
                    write_json_pair(&mut writer, &key, value, &mut pair_count)?;
                }
            }
            value => write_json_pair(&mut writer, &key, value, &mut pair_count)?,
        }
    }
    Ok(writer.into_inner())
}

fn write_json_pair(
    writer: &mut CappedWriter,
    key: &str,
    value: Value,
    pair_count: &mut usize,
) -> Result<(), TransformFailure> {
    let value = match value {
        Value::String(value) => value,
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) => return Err(TransformFailure::BadRequest),
    };
    if value.len() > MAX_VALUE_BYTES {
        return Err(TransformFailure::TooLarge);
    }
    *pair_count = pair_count.checked_add(1).ok_or(TransformFailure::TooLarge)?;
    if *pair_count > MAX_PAIRS {
        return Err(TransformFailure::TooLarge);
    }
    if *pair_count > 1 {
        writer.write_all(b"&").map_err(writer_failure)?;
    }
    write_form_component(writer, key.as_bytes())?;
    writer.write_all(b"=").map_err(writer_failure)?;
    write_form_component(writer, value.as_bytes())
}

fn write_form_component(writer: &mut CappedWriter, input: &[u8]) -> Result<(), TransformFailure> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in input {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                writer.write_all(&[*byte]).map_err(writer_failure)?;
            }
            b' ' => writer.write_all(b"+").map_err(writer_failure)?,
            byte => writer
                .write_all(&[b'%', HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]])
                .map_err(writer_failure)?,
        }
    }
    Ok(())
}

fn writer_failure(_: std::io::Error) -> TransformFailure {
    TransformFailure::TooLarge
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/form_json_transform/plugin_test.rs"]
mod tests;
