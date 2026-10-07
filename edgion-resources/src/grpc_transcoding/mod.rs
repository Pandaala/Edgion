//! Bounded descriptor compilation and ProtoJSON codecs shared by both binaries.
mod codec;
mod descriptor;
mod mapping;
mod wire;

use crate::resources::edgion_plugins::PluginValidation;
use base64::{engine::general_purpose::STANDARD, Engine};
use prost_reflect::{DescriptorPool, MethodDescriptor};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use codec::CappedWriter;
pub use mapping::{BoundRequest, CompiledBinding};
pub const MAX_DESCRIPTOR_BYTES: usize = 512 * 1024;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_PROTO_RESPONSE_BYTES: usize = 512 * 1024;

/// Preserve the descriptor field's existing exact binaryData selector bounds.
pub(crate) fn valid_descriptor_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 253
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum GrpcDescriptorSource {
    Inline { base64: String },
    ConfigMapRef { name: String, key: String },
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GrpcJsonStreamFormat {
    #[default]
    Ndjson,
    Sse,
}
fn request_cap() -> String {
    "1m".into()
}
fn response_cap() -> String {
    "512k".into()
}
fn depth() -> u32 {
    32
}
fn nodes() -> u32 {
    16384
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrpcJsonTranscoderConfig {
    pub descriptor: GrpcDescriptorSource,
    pub services: Vec<String>,
    #[serde(default)]
    pub auto_mapping: bool,
    #[serde(default)]
    pub ignore_unknown_query_parameters: bool,
    #[serde(default)]
    pub stream_format: GrpcJsonStreamFormat,
    #[serde(default = "request_cap")]
    pub max_request_message_size: String,
    #[serde(default = "response_cap")]
    pub max_response_message_size: String,
    #[serde(default = "request_cap")]
    pub max_response_json_size: String,
    #[serde(default = "depth")]
    pub max_message_depth: u32,
    #[serde(default = "nodes")]
    pub max_message_nodes: u32,
    /// Controller-resolved content; serialized only for configuration transport.
    #[schemars(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_descriptor: Option<Vec<u8>>,
    #[schemars(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor_error: Option<String>,
}
#[derive(Debug, Clone, Copy)]
pub struct TranscodingLimits {
    pub request: usize,
    pub response: usize,
    pub json: usize,
    pub depth: usize,
    pub nodes: u64,
}
#[derive(Debug)]
pub struct CompiledGrpcTranscoder {
    bindings: Vec<Arc<CompiledBinding>>,
    pub limits: TranscodingLimits,
    pub format: GrpcJsonStreamFormat,
    pub ignore_unknown_query: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TranscodingError {
    #[error("invalid transcoding input")]
    Invalid,
    #[error("HTTP mapping absent")]
    NotFound,
    #[error("transcoding payload exceeds limit")]
    TooLarge,
}
impl TranscodingError {
    pub fn request_status(self) -> u16 {
        match self {
            Self::Invalid => 400,
            Self::NotFound => 404,
            Self::TooLarge => 413,
        }
    }
}

impl GrpcJsonTranscoderConfig {
    pub fn descriptor_bytes(&self) -> Result<Vec<u8>, String> {
        if self.descriptor_error.is_some() {
            return Err("descriptor resolution failed".into());
        }
        let bytes = match &self.descriptor {
            GrpcDescriptorSource::Inline { base64 } => {
                if base64.len() > MAX_DESCRIPTOR_BYTES.div_ceil(3) * 4 {
                    return Err("descriptor exceeds 512 KiB".into());
                }
                STANDARD.decode(base64).map_err(|_| "descriptor base64 is invalid")?
            }
            GrpcDescriptorSource::ConfigMapRef { .. } => {
                self.resolved_descriptor.clone().ok_or("descriptor is unresolved")?
            }
        };
        if bytes.is_empty() || bytes.len() > MAX_DESCRIPTOR_BYTES {
            return Err("descriptor must contain 1..512 KiB".into());
        }
        Ok(bytes)
    }
    pub fn limits(&self) -> Result<TranscodingLimits, String> {
        fn size(value: &str, cap: usize) -> Result<usize, String> {
            let n = crate::resources::common::parse_byte_size(value).ok_or("invalid message size")?;
            if n == 0 || n > cap as u64 {
                return Err("message size exceeds its hard bound".into());
            }
            Ok(n as usize)
        }
        if !(1..=32).contains(&self.max_message_depth) || !(1..=16384).contains(&self.max_message_nodes) {
            return Err("message complexity exceeds depth 32 or nodes 16384".into());
        }
        if size(&self.max_response_json_size, MAX_JSON_BYTES)? < 128 {
            return Err("response JSON limit must be at least 128 bytes".into());
        }
        Ok(TranscodingLimits {
            request: size(&self.max_request_message_size, MAX_JSON_BYTES)?,
            response: size(&self.max_response_message_size, MAX_PROTO_RESPONSE_BYTES)?,
            json: size(&self.max_response_json_size, MAX_JSON_BYTES)?,
            depth: self.max_message_depth as usize,
            nodes: self.max_message_nodes as u64,
        })
    }
    pub fn compile(&self) -> Result<CompiledGrpcTranscoder, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.compile_inner()))
            .unwrap_or_else(|_| Err("descriptor compilation panicked".into()))
    }
    fn compile_inner(&self) -> Result<CompiledGrpcTranscoder, String> {
        let limits = self.limits()?;
        let bytes = self.descriptor_bytes()?;
        // Admit descriptor structure before the indexed reflection pool expands it.
        let descriptor = descriptor::admission_pool()
            .get_message_by_name("google.protobuf.FileDescriptorSet")
            .ok_or("descriptor admission schema is unavailable")?;
        wire::preflight_descriptor(
            &descriptor,
            &bytes,
            TranscodingLimits {
                depth: 32,
                nodes: 16384,
                ..limits
            },
        )
        .map_err(|_| "descriptor structure exceeds limits or contains unsupported options")?;
        descriptor::admit(&bytes)?;
        let pool = DescriptorPool::decode(bytes.as_slice()).map_err(|_| "invalid descriptor/import closure")?;
        if pool.files().count() > 128
            || pool.all_messages().count() > 1024
            || pool.all_messages().map(|m| m.fields().len()).sum::<usize>() > 8192
            || self.services.is_empty()
            || self.services.len() > 64
        {
            return Err("descriptor inventory exceeds limits".into());
        }
        for m in pool.all_messages() {
            validate_message_shape(&m)?;
            if matches!(m.full_name(), "google.protobuf.Any" | "google.api.HttpBody")
                || m.fields().any(|f| f.is_group())
            {
                return Err("Any, HttpBody and groups are unsupported".into());
            }
            if m.fields().any(|field| {
                field
                    .field_descriptor_proto()
                    .default_value
                    .as_ref()
                    .is_some_and(|value| {
                        !value.is_empty()
                            && matches!(field.kind(), prost_reflect::Kind::String | prost_reflect::Kind::Bytes)
                    })
            }) {
                return Err("explicit string and bytes defaults are unsupported".into());
            }
        }
        let mut bindings = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for name in &self.services {
            if !seen.insert(name) {
                return Err("duplicate allowed service".into());
            }
            let service = pool.get_service_by_name(name).ok_or("allowed service is absent")?;
            for method in service.methods() {
                let mut checked = std::collections::HashSet::new();
                validate_rpc_message(&method.input(), &mut checked)?;
                validate_rpc_message(&method.output(), &mut checked)?;
                if rpc_path(&method).len() > 4096 {
                    return Err("canonical RPC path exceeds limits".into());
                }
                if method.is_client_streaming() {
                    return Err("client streaming is unsupported".into());
                }
                let mut annotated = false;
                if let Some(ext) = pool.get_extension_by_name("google.api.http") {
                    let options = method.options();
                    if options.has_extension(&ext) {
                        annotated = true;
                        let option = options.get_extension(&ext);
                        let rule = option.as_message().ok_or("invalid HTTP annotation")?;
                        mapping::compile_rules(&method, rule, &mut bindings, 0)?;
                    }
                }
                if !annotated && self.auto_mapping {
                    bindings.push(Arc::new(CompiledBinding::compile(
                        method.clone(),
                        "POST",
                        &rpc_path(&method),
                        "*",
                    )?));
                }
                if bindings.len() > 256 {
                    return Err("too many HTTP bindings".into());
                }
            }
        }
        if bindings.is_empty() {
            return Err("no HTTP mappings for allowed services".into());
        }
        let mut paths = std::collections::HashSet::new();
        for b in &bindings {
            if !paths.insert((b.http_method.clone(), b.regex.as_str().to_owned())) {
                return Err("ambiguous HTTP mapping".into());
            }
        }
        Ok(CompiledGrpcTranscoder {
            bindings,
            limits,
            format: self.stream_format,
            ignore_unknown_query: self.ignore_unknown_query_parameters,
        })
    }
}
impl PluginValidation for GrpcJsonTranscoderConfig {
    fn validation_error(&self) -> Option<String> {
        if let Err(e) = self.limits() {
            return Some(e);
        }
        if let GrpcDescriptorSource::ConfigMapRef { name, key } = &self.descriptor {
            if name.is_empty() || name.len() > 253 || !valid_descriptor_key(key) {
                return Some("invalid descriptor ConfigMap reference".into());
            }
            // Ingest validation precedes store resolution. Gateway compilation
            // always requires resolved content; resolution failures are explicit.
            if self.resolved_descriptor.is_none() && self.descriptor_error.is_none() {
                return None;
            }
        }
        self.compile().err()
    }
}
impl CompiledGrpcTranscoder {
    pub fn bind(&self, method: &str, path: &str, query: Option<&str>) -> Result<BoundRequest, TranscodingError> {
        if path.len().saturating_add(query.map_or(0, str::len)) > 65536 {
            return Err(TranscodingError::Invalid);
        }
        let mut matched = self
            .bindings
            .iter()
            .filter(|b| b.http_method == method && b.regex.is_match(path));
        let binding = matched.next().ok_or(TranscodingError::NotFound)?.clone();
        if matched.next().is_some() {
            return Err(TranscodingError::Invalid);
        }
        binding.bind(path, query, self.ignore_unknown_query)
    }
}
fn rpc_path(method: &MethodDescriptor) -> String {
    format!("/{}/{}", method.parent_service().full_name(), method.name())
}

fn validate_rpc_message(
    message: &prost_reflect::MessageDescriptor,
    checked: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    if !checked.insert(message.full_name().to_owned()) {
        return Ok(());
    }
    // Descriptor option extensions remain available for google.api.http. Runtime
    // messages cannot have extension ranges, so the wire preflight is complete.
    if message.extension_ranges().len() != 0 {
        return Err("runtime message extensions are unsupported".into());
    }
    for field in message.fields() {
        if let prost_reflect::Kind::Message(child) = field.kind() {
            validate_rpc_message(&child, checked)?;
        }
    }
    Ok(())
}

fn validate_message_shape(message: &prost_reflect::MessageDescriptor) -> Result<(), String> {
    use prost_reflect::{Cardinality, Kind};
    if message.is_map_entry() {
        let key = message.get_field(1).ok_or("invalid map entry key")?;
        let value = message.get_field(2).ok_or("invalid map entry value")?;
        if message.fields().len() != 2
            || key.cardinality() != Cardinality::Optional
            || value.cardinality() != Cardinality::Optional
            || !matches!(
                key.kind(),
                Kind::Bool
                    | Kind::String
                    | Kind::Int32
                    | Kind::Int64
                    | Kind::Uint32
                    | Kind::Uint64
                    | Kind::Sint32
                    | Kind::Sint64
                    | Kind::Fixed32
                    | Kind::Fixed64
                    | Kind::Sfixed32
                    | Kind::Sfixed64
            )
        {
            return Err("invalid map entry shape".into());
        }
    }
    if matches!(
        message.full_name(),
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
        let pool = DescriptorPool::global();
        let canonical = pool
            .get_message_by_name(message.full_name())
            .ok_or("well-known type unavailable")?;
        if message.fields().len() != canonical.fields().len()
            || message.fields().any(|field| {
                canonical.get_field(field.number()).is_none_or(|expected| {
                    field.name() != expected.name()
                        || field.cardinality() != expected.cardinality()
                        || field.is_map() != expected.is_map()
                        || field.field_descriptor_proto().r#type != expected.field_descriptor_proto().r#type
                        || field.field_descriptor_proto().type_name != expected.field_descriptor_proto().type_name
                })
            })
        {
            return Err("noncanonical well-known type shape".into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/grpc_transcoding/mod_test.rs"]
mod tests;
