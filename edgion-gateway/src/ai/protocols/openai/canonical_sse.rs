//! Single-pass observation and owned handoff for canonical OpenAI SSE records.

use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::StreamingResponseProcessor;
use crate::plugins::runtime::PluginSession;
use crate::response::terminal::{ResponseTerminalCause, TerminalEmission};
use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;
use pingora_proxy::ResponseBodySink;

use crate::ai::protocols::json_span::{field, last_field, Field, Node, Parser, StringToken};
use crate::sse::chain::SliceChain;
use crate::sse::window::{LogicalBucketKey, LogicalField, SemanticFragment, SemanticSource, WindowRecord};
use crate::sse::{ByteRange, SseDecoder, SseEventRef, SseRecordRef};
use serde_json::Value;

pub(crate) const MAX_CANONICAL_BATCH_RECORDS: usize = 1_024;
pub(crate) const MAX_CANONICAL_BATCH_METADATA_BYTES: usize = 128 * 1024;
pub(crate) const MAX_CANONICAL_SOURCE_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const MAX_CANONICAL_BATCH_RAW_BYTES: usize =
    crate::sse::DEFAULT_MAX_PENDING_EVENT_BYTES + MAX_CANONICAL_SOURCE_CHUNK_BYTES;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CanonicalUsageObservation {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

#[derive(Debug, Default)]
pub(crate) struct CanonicalEventObservation {
    pub usage: CanonicalUsageObservation,
    pub output_units: u64,
    pub fragments: Vec<SemanticFragment>,
    pub semantic_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanonicalSourceStatus {
    Available,
    Unavailable,
}

#[derive(Debug)]
pub struct CanonicalRecordBatch {
    records: Vec<WindowRecord>,
    fallback: SliceChain,
    raw_bytes: usize,
    metadata_bytes: usize,
    status: CanonicalSourceStatus,
}

impl CanonicalRecordBatch {
    pub(crate) fn new() -> Self {
        Self {
            records: Vec::new(),
            fallback: SliceChain::default(),
            raw_bytes: 0,
            metadata_bytes: 0,
            status: CanonicalSourceStatus::Available,
        }
    }

    pub(crate) fn push_observed(
        &mut self,
        record: SseRecordRef<'_>,
        observation: Option<CanonicalEventObservation>,
    ) -> Result<(), WindowRecord> {
        let unavailable = matches!(&record, SseRecordRef::Opaque(_));
        let mut owned = WindowRecord::from_sse_record(record);
        if let Some(observation) = observation {
            if observation.semantic_available {
                owned = owned.with_fragments(observation.fragments);
            } else {
                owned = owned.semantic_unavailable();
                self.mark_unavailable();
            }
        }
        if unavailable {
            self.mark_unavailable();
        }
        self.push(owned)
    }

    pub(crate) fn push(&mut self, record: WindowRecord) -> Result<(), WindowRecord> {
        let Some(next_raw) = self.raw_bytes.checked_add(record.raw_len()) else {
            return Err(record);
        };
        if next_raw > MAX_CANONICAL_BATCH_RAW_BYTES {
            return Err(record);
        }
        let record_metadata = record.metadata_bytes();
        let annotated_fits = self.records.len() < MAX_CANONICAL_BATCH_RECORDS
            && self
                .metadata_bytes
                .checked_add(record_metadata)
                .is_some_and(|value| value <= MAX_CANONICAL_BATCH_METADATA_BYTES)
            && self.fallback.is_empty();
        if annotated_fits {
            self.metadata_bytes += record_metadata;
            self.records.push(record);
        } else {
            self.status = CanonicalSourceStatus::Unavailable;
            if self.fallback.is_empty() {
                let mut fallback = SliceChain::default();
                for prior in &self.records {
                    for slice in prior.raw_slices_cloned() {
                        fallback
                            .push(slice, MAX_CANONICAL_BATCH_RAW_BYTES)
                            .map_err(|_| record.clone())?;
                    }
                }
                self.metadata_bytes = 0;
                self.fallback = fallback;
                self.records.clear();
            }
            for slice in record.raw_slices_cloned() {
                self.fallback
                    .push(slice, MAX_CANONICAL_BATCH_RAW_BYTES)
                    .map_err(|_| record.clone())?;
            }
        }
        self.raw_bytes = next_raw;
        Ok(())
    }

    pub(crate) fn mark_unavailable(&mut self) {
        self.status = CanonicalSourceStatus::Unavailable;
    }

    pub(crate) fn status(&self) -> CanonicalSourceStatus {
        self.status
    }

    pub(crate) fn into_records(self) -> Vec<WindowRecord> {
        let mut records = self.records;
        if !self.fallback.is_empty() {
            records.push(WindowRecord::raw(self.fallback.raw_slices()).semantic_unavailable());
        }
        records
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty() && self.fallback.is_empty()
    }

    pub(crate) fn raw_len(&self) -> usize {
        self.raw_bytes
    }

    pub(crate) fn into_raw_bytes(self) -> Result<Bytes, ()> {
        let mut output = Vec::with_capacity(self.raw_len());
        if output.capacity() > MAX_CANONICAL_BATCH_RAW_BYTES {
            return Err(());
        }
        for record in self.records {
            for slice in record.into_raw_slices() {
                output.extend_from_slice(&slice);
            }
        }
        for slice in self.fallback.raw_slices() {
            output.extend_from_slice(&slice);
        }
        Ok(Bytes::from(output))
    }
}

/// Serialize a gateway-constructed canonical event while recording semantic
/// token ranges during the same structural walk. Generated bytes are never
/// decoded or searched for repeated string values.
pub(crate) fn write_canonical_record(value: &Value, limit: usize) -> Result<(Bytes, WindowRecord), ()> {
    let mut writer = ValueWriter {
        bytes: Vec::new(),
        fragments: Vec::new(),
        limit,
    };
    writer.append(b"data: ")?;
    writer.value(value, WriteContext::Root)?;
    writer.append(b"\n\n")?;
    let bytes = Bytes::from(writer.bytes);
    let record = WindowRecord::raw(vec![bytes.clone()]).with_fragments(writer.fragments);
    Ok((bytes, record))
}

#[derive(Clone, Copy)]
enum WriteContext {
    Root,
    Choices,
    Choice(u32),
    Delta(u32),
    ToolCalls(u32),
    Tool(u32, u32),
    Function(u32, u32),
    Other,
}

struct ValueWriter {
    bytes: Vec<u8>,
    fragments: Vec<SemanticFragment>,
    limit: usize,
}

impl ValueWriter {
    fn append(&mut self, value: &[u8]) -> Result<(), ()> {
        let next = self.bytes.len().checked_add(value.len()).ok_or(())?;
        if next > self.limit {
            return Err(());
        }
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn value(&mut self, value: &Value, context: WriteContext) -> Result<(), ()> {
        match value {
            Value::Object(map) => {
                self.append(b"{")?;
                for (position, (key, child)) in map.iter().enumerate() {
                    if position != 0 {
                        self.append(b",")?;
                    }
                    self.append(&serde_json::to_vec(key).map_err(|_| ())?)?;
                    self.append(b":")?;
                    let next = match (context, key.as_str()) {
                        (WriteContext::Root, "choices") => WriteContext::Choices,
                        (WriteContext::Choice(choice), "delta") => WriteContext::Delta(choice),
                        (WriteContext::Delta(choice), "tool_calls") => WriteContext::ToolCalls(choice),
                        (WriteContext::Tool(choice, tool), "function") => WriteContext::Function(choice, tool),
                        _ => WriteContext::Other,
                    };
                    let semantic = match (context, key.as_str()) {
                        (WriteContext::Delta(choice), "content") => Some(LogicalBucketKey {
                            choice_index: choice,
                            field: LogicalField::Content,
                        }),
                        (WriteContext::Delta(choice), "reasoning_content") => Some(LogicalBucketKey {
                            choice_index: choice,
                            field: LogicalField::ReasoningContent,
                        }),
                        (WriteContext::Function(choice, tool), "arguments") => Some(LogicalBucketKey {
                            choice_index: choice,
                            field: LogicalField::ToolArguments { tool_call_index: tool },
                        }),
                        _ => None,
                    };
                    if let (Some(bucket), Value::String(text)) = (semantic, child) {
                        let start = self.bytes.len();
                        self.append(&serde_json::to_vec(text).map_err(|_| ())?)?;
                        let end = self.bytes.len();
                        self.fragments.push(SemanticFragment {
                            bucket,
                            source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(start, end)]),
                        });
                    } else {
                        self.value(child, next)?;
                    }
                }
                self.append(b"}")
            }
            Value::Array(values) => {
                self.append(b"[")?;
                for (position, child) in values.iter().enumerate() {
                    if position != 0 {
                        self.append(b",")?;
                    }
                    let next = match context {
                        WriteContext::Choices => child
                            .get("index")
                            .and_then(Value::as_u64)
                            .and_then(|v| u32::try_from(v).ok())
                            .map(WriteContext::Choice)
                            .unwrap_or(WriteContext::Other),
                        WriteContext::ToolCalls(choice) => child
                            .get("index")
                            .and_then(Value::as_u64)
                            .and_then(|v| u32::try_from(v).ok())
                            .map(|tool| WriteContext::Tool(choice, tool))
                            .unwrap_or(WriteContext::Other),
                        _ => WriteContext::Other,
                    };
                    self.value(child, next)?;
                }
                self.append(b"]")
            }
            _ => self.append(&serde_json::to_vec(value).map_err(|_| ())?),
        }
    }
}

pub(crate) fn observe_canonical_event(event: &SseEventRef<'_>) -> CanonicalEventObservation {
    observe_event(event, true)
}

fn observe_event(event: &SseEventRef<'_>, account_usage: bool) -> CanonicalEventObservation {
    if event.data_is_literal(b"[DONE]") {
        return CanonicalEventObservation {
            semantic_available: true,
            ..Default::default()
        };
    }
    let Ok(data) = event.semantic_data() else {
        return CanonicalEventObservation::default();
    };
    let Ok(root) = Parser::new(data.as_bytes()).parse() else {
        return CanonicalEventObservation::default();
    };
    let mut observation = CanonicalEventObservation {
        semantic_available: true,
        ..Default::default()
    };
    if account_usage {
        observation.usage = observe_usage(&root);
    }
    let choices = match field(&root, "choices") {
        Field::Missing => return observation,
        Field::Unique(value) => match value.array() {
            Some(choices) => choices,
            None => {
                observation.semantic_available = false;
                return observation;
            }
        },
        Field::Ambiguous => {
            observation.semantic_available = false;
            return observation;
        }
    };
    for choice in choices {
        let delta = match field(choice, "delta") {
            Field::Missing => continue,
            Field::Unique(value) => match value.object_marker() {
                Some(delta) => delta,
                None => {
                    observation.semantic_available = false;
                    continue;
                }
            },
            Field::Ambiguous => {
                observation.semantic_available = false;
                continue;
            }
        };
        let choice_index = match field(choice, "index") {
            Field::Unique(value) => value.u32(),
            Field::Missing | Field::Ambiguous => None,
        };
        if choice_index.is_none() {
            observation.semantic_available = false;
        }
        let mut qualifying_output = false;
        for (name, semantic_field) in [
            ("content", LogicalField::Content),
            ("reasoning_content", LogicalField::ReasoningContent),
        ] {
            let token = match field(delta, name) {
                Field::Missing => continue,
                Field::Unique(value) => match value.string_token() {
                    Some(token) => token,
                    None => {
                        observation.semantic_available = false;
                        continue;
                    }
                },
                Field::Ambiguous => {
                    observation.semantic_available = false;
                    continue;
                }
            };
            if name == "content" && !token.value.is_empty() {
                qualifying_output = true;
            }
            {
                if !choice_index.is_some_and(|choice_index| {
                    push_token(event, &mut observation.fragments, choice_index, semantic_field, token)
                }) {
                    observation.semantic_available = false;
                }
            }
        }
        let tool_calls = match field(delta, "tool_calls") {
            Field::Missing => None,
            Field::Unique(value) => match value.array() {
                Some(calls) => Some(calls),
                None => {
                    observation.semantic_available = false;
                    None
                }
            },
            Field::Ambiguous => {
                observation.semantic_available = false;
                None
            }
        };
        if let Some(tool_calls) = tool_calls {
            for tool in tool_calls {
                let function = match field(tool, "function") {
                    Field::Missing => continue,
                    Field::Unique(value) => match value.object_marker() {
                        Some(function) => function,
                        None => {
                            observation.semantic_available = false;
                            continue;
                        }
                    },
                    Field::Ambiguous => {
                        observation.semantic_available = false;
                        continue;
                    }
                };
                let tool_index = match field(tool, "index") {
                    Field::Unique(value) => value.u32(),
                    Field::Missing | Field::Ambiguous => None,
                };
                if tool_index.is_none() {
                    observation.semantic_available = false;
                }
                let token = match field(function, "arguments") {
                    Field::Missing => continue,
                    Field::Unique(value) => match value.string_token() {
                        Some(token) => token,
                        None => {
                            observation.semantic_available = false;
                            continue;
                        }
                    },
                    Field::Ambiguous => {
                        observation.semantic_available = false;
                        continue;
                    }
                };
                if !token.value.is_empty() {
                    qualifying_output = true;
                }
                if !choice_index.zip(tool_index).is_some_and(|(choice_index, tool_index)| {
                    push_token(
                        event,
                        &mut observation.fragments,
                        choice_index,
                        LogicalField::ToolArguments {
                            tool_call_index: tool_index,
                        },
                        token,
                    )
                }) {
                    observation.semantic_available = false;
                }
            }
        }
        if qualifying_output {
            observation.output_units = observation.output_units.saturating_add(1);
        }
    }
    if !observation.semantic_available {
        observation.fragments.clear();
    }
    observation
}

fn push_token(
    event: &SseEventRef<'_>,
    fragments: &mut Vec<SemanticFragment>,
    choice_index: u32,
    field: LogicalField,
    token: &StringToken,
) -> bool {
    let Ok(raw) = event.semantic_range_to_raw(ByteRange::new(token.start, token.end)) else {
        return false;
    };
    fragments.push(SemanticFragment {
        bucket: LogicalBucketKey { choice_index, field },
        source: SemanticSource::JsonStringTokenRanges(vec![raw]),
    });
    true
}

fn observe_usage(root: &Node) -> CanonicalUsageObservation {
    let Some(usage) = last_field(root, "usage").and_then(Node::object_marker) else {
        return CanonicalUsageObservation::default();
    };
    let details = last_field(usage, "prompt_tokens_details").and_then(Node::object_marker);
    CanonicalUsageObservation {
        prompt_tokens: last_field(usage, "prompt_tokens").and_then(Node::u64),
        completion_tokens: last_field(usage, "completion_tokens").and_then(Node::u64),
        cached_tokens: details
            .and_then(|value| last_field(value, "cached_tokens"))
            .and_then(Node::u64),
        cache_read_input_tokens: last_field(usage, "cache_read_input_tokens").and_then(Node::u64),
        cache_creation_input_tokens: last_field(usage, "cache_creation_input_tokens").and_then(Node::u64),
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/protocols/openai/canonical_sse_test.rs"]
mod tests;

/// Canonical semantic source installed only for explicit Guardrail with usage off.
pub(crate) struct SemanticSseSource {
    decoder: SseDecoder,
    identity_encoded: bool,
}

impl SemanticSseSource {
    pub(crate) fn new() -> Self {
        Self {
            decoder: SseDecoder::with_default_limit(),
            identity_encoded: true,
        }
    }
}

#[async_trait]
impl StreamingResponseProcessor for SemanticSseSource {
    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        self.identity_encoded =
            crate::routes::http::proxy_http::content_type::response_is_identity_encoded(&response.headers);
    }

    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        if stream_kind != ResponseStreamKind::Sse || !session.ai().semantic_guard_active() {
            return;
        }
        let mut batch = CanonicalRecordBatch::new();
        let mut overflowed = false;
        if let Some(bytes) = chunk.take() {
            if self.identity_encoded {
                self.decoder.feed_records(bytes, |record| {
                    let observation = match &record {
                        SseRecordRef::Block { event, .. } => Some(observe_event(event, false)),
                        _ => None,
                    };
                    if batch.push_observed(record, observation).is_err() {
                        overflowed = true;
                    }
                });
            } else {
                batch.mark_unavailable();
                if batch.push(WindowRecord::raw(vec![bytes])).is_err() {
                    overflowed = true;
                }
            }
        }
        if end_of_stream && self.identity_encoded {
            self.decoder.finish_records(|record| {
                if batch.push_observed(record, None).is_err() {
                    overflowed = true;
                }
            });
        }
        if self.decoder.degraded() {
            batch.mark_unavailable();
        }
        if (overflowed || session.ai().publish_canonical_records(batch).is_err())
            && session.try_claim_response_terminal(ResponseTerminalCause::ProcessorFailure)
        {
            session.mark_response_terminal_emission(TerminalEmission::NoEvent);
            sink.terminate();
        }
    }
}
