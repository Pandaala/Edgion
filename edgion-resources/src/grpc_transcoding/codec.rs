use super::{mapping::BoundRequest, wire, CompiledBinding, GrpcJsonStreamFormat, TranscodingError, TranscodingLimits};
use crate::resources::edgion_plugins::plugin_configs::{parse_json_instance_with_limits, profile_json_value};
use prost::Message;
use prost_reflect::{DynamicMessage, Kind, MessageDescriptor};
use std::io::{self, Write};
use std::sync::Arc;

/// A writer that refuses allocation beyond the encoded output bound.
pub struct CappedWriter {
    bytes: Vec<u8>,
    cap: usize,
}
impl CappedWriter {
    pub fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(cap),
            cap,
        }
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes.into_boxed_slice().into_vec()
    }
}
impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let len = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("output limit"))?;
        if len > self.cap {
            return Err(io::Error::other("output limit"));
        }
        // Fixed-capacity admission avoids geometric growth exceeding the cap.
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl BoundRequest {
    pub fn encode(
        self,
        bytes: &[u8],
        limits: TranscodingLimits,
    ) -> Result<(Arc<CompiledBinding>, Vec<u8>), TranscodingError> {
        let parsed = if bytes.is_empty() {
            None
        } else {
            Some(
                parse_json_instance_with_limits(bytes, limits.depth, limits.nodes)
                    .map_err(|_| TranscodingError::Invalid)?
                    .into_value(),
            )
        };
        let (binding, value) = self.merge(parsed)?;
        let profile = profile_json_value(&value).map_err(|_| TranscodingError::Invalid)?;
        if profile.max_depth > limits.depth
            || profile.node_count > limits.nodes
            || profile.text_bytes > super::MAX_JSON_BYTES as u64
        {
            return Err(TranscodingError::Invalid);
        }
        validate_field_names(&binding.method.input(), &value)?;
        let mut writer = CappedWriter::new(super::MAX_JSON_BYTES);
        serde_json::to_writer(&mut writer, &value).map_err(|_| TranscodingError::Invalid)?;
        drop(value);
        let input = writer.into_bytes();
        let mut de = serde_json::Deserializer::from_slice(&input);
        let message =
            DynamicMessage::deserialize(binding.method.input(), &mut de).map_err(|_| TranscodingError::Invalid)?;
        de.end().map_err(|_| TranscodingError::Invalid)?;
        let len = message.encoded_len();
        if len > limits.request {
            return Err(TranscodingError::TooLarge);
        }
        let mut frame = Vec::with_capacity(len + 5);
        frame.push(0);
        frame.extend_from_slice(&(len as u32).to_be_bytes());
        message.encode(&mut frame).map_err(|_| TranscodingError::Invalid)?;
        Ok((binding, frame))
    }
}

// Reflection accepts both JSON and proto field spellings, but silently replaces
// aliases of an ordinary field. Detect those collisions before deserialization.
fn validate_field_names(desc: &MessageDescriptor, value: &serde_json::Value) -> Result<(), TranscodingError> {
    if matches!(
        desc.full_name(),
        "google.protobuf.Timestamp"
            | "google.protobuf.Duration"
            | "google.protobuf.FieldMask"
            | "google.protobuf.Struct"
            | "google.protobuf.Value"
            | "google.protobuf.ListValue"
            | "google.protobuf.DoubleValue"
            | "google.protobuf.FloatValue"
            | "google.protobuf.Int64Value"
            | "google.protobuf.UInt64Value"
            | "google.protobuf.Int32Value"
            | "google.protobuf.UInt32Value"
            | "google.protobuf.BoolValue"
            | "google.protobuf.StringValue"
            | "google.protobuf.BytesValue"
    ) {
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    let mut fields = std::collections::BTreeSet::new();
    for (name, value) in object {
        let field = desc
            .get_field_by_json_name(name)
            .or_else(|| desc.get_field_by_name(name))
            .ok_or(TranscodingError::Invalid)?;
        if !fields.insert(field.number()) {
            return Err(TranscodingError::Invalid);
        }
        if let Kind::Message(child) = field.kind() {
            if field.is_map() {
                if let Kind::Message(element) = child.map_entry_value_field().kind() {
                    if let Some(map) = value.as_object() {
                        for value in map.values() {
                            validate_field_names(&element, value)?;
                        }
                    }
                }
            } else if field.is_list() {
                if let Some(list) = value.as_array() {
                    for value in list {
                        validate_field_names(&child, value)?;
                    }
                }
            } else {
                validate_field_names(&child, value)?;
            }
        }
    }
    Ok(())
}
impl CompiledBinding {
    pub fn decode(
        &self,
        bytes: &[u8],
        limits: TranscodingLimits,
        format: Option<GrpcJsonStreamFormat>,
    ) -> Result<Vec<u8>, TranscodingError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.decode_inner(bytes, limits, format)
        }))
        .map_err(|_| TranscodingError::Invalid)?
    }
    fn decode_inner(
        &self,
        bytes: &[u8],
        limits: TranscodingLimits,
        format: Option<GrpcJsonStreamFormat>,
    ) -> Result<Vec<u8>, TranscodingError> {
        if bytes.len() > limits.response {
            return Err(TranscodingError::Invalid);
        }
        wire::preflight(&self.method.output(), bytes, limits)?;
        let message = DynamicMessage::decode(self.method.output(), bytes).map_err(|_| TranscodingError::Invalid)?;
        let mut writer = CappedWriter::new(limits.json);
        let (prefix, suffix): (&[u8], &[u8]) = match format {
            None => (b"", b""),
            Some(GrpcJsonStreamFormat::Ndjson) => (b"{\"type\":\"message\",\"data\":", b"}\n"),
            Some(GrpcJsonStreamFormat::Sse) => (b"event: rpc_message\ndata: ", b"\n\n"),
        };
        writer.write_all(prefix).map_err(|_| TranscodingError::Invalid)?;
        serde_json::to_writer(&mut writer, &message).map_err(|_| TranscodingError::Invalid)?;
        writer.write_all(suffix).map_err(|_| TranscodingError::Invalid)?;
        Ok(writer.into_bytes())
    }
}
