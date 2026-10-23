//! Strict OpenAI Chat Completions to Anthropic Messages request translation.

use std::collections::HashSet;
use std::io::{self, Write};

use serde_json::{json, Map, Value};

const TOP_LEVEL_FIELDS: &[&str] = &[
    "model",
    "messages",
    "stream",
    "stream_options",
    "max_completion_tokens",
    "max_tokens",
    "stop",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "n",
    "temperature",
    "top_p",
];
const MAX_TOOL_CALL_ID_BYTES: usize = 512;
const MAX_TOOL_NAME_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedAnthropicRequest {
    pub(crate) body: Vec<u8>,
    pub(crate) requested_streaming: bool,
    pub(crate) effective_usage_chunk: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnthropicRequestError {
    InvalidRequestBody,
    UnsupportedParameter,
    MissingRequiredParameter,
    OutputTooLarge,
    SerializationFailed,
}

/// Validate and translate one parsed OpenAI Chat Completions request.
///
/// The caller has already resolved the client model against the selected backend catalog and
/// passes its canonical provider model here. Serialization never produces more than
/// `max_output_bytes` bytes.
pub(crate) fn prepare_anthropic_request(
    request: Value,
    canonical_model: &str,
    default_max_output_tokens: Option<u32>,
    inject_usage_tracking: bool,
    max_output_bytes: usize,
) -> Result<PreparedAnthropicRequest, AnthropicRequestError> {
    let request = request.as_object().ok_or(AnthropicRequestError::InvalidRequestBody)?;
    reject_unknown_top_level_fields(request)?;
    validate_client_model(request)?;

    let requested_streaming = optional_bool(request, "stream")?.unwrap_or(false);
    let include_usage = parse_stream_options(request, requested_streaming)?;
    validate_compatibility_fields(request)?;

    let request_max = resolve_request_max_tokens(request)?;
    let max_tokens = match request_max.or(default_max_output_tokens) {
        Some(value) if value > 0 => value,
        Some(_) => return Err(AnthropicRequestError::InvalidRequestBody),
        None => return Err(AnthropicRequestError::MissingRequiredParameter),
    };

    let (tools, declared_tool_names) = parse_tools(request.get("tools"))?;
    let messages_value = request
        .get("messages")
        .ok_or(AnthropicRequestError::InvalidRequestBody)?;
    let (system, messages) = parse_messages(messages_value, &declared_tool_names)?;
    let stop_sequences = parse_stop(request.get("stop"))?;
    let tool_choice = parse_tool_choice(
        request.get("tool_choice"),
        request.get("parallel_tool_calls"),
        tools.is_some(),
        &declared_tool_names,
    )?;

    let mut translated = Map::new();
    translated.insert("model".to_string(), Value::String(canonical_model.to_string()));
    translated.insert("max_tokens".to_string(), Value::from(max_tokens));
    translated.insert("messages".to_string(), Value::Array(messages));
    translated.insert("stream".to_string(), Value::Bool(requested_streaming));
    if !system.is_empty() {
        translated.insert("system".to_string(), Value::Array(system));
    }
    if let Some(stop_sequences) = stop_sequences {
        translated.insert("stop_sequences".to_string(), Value::Array(stop_sequences));
    }
    if let Some(tools) = tools {
        translated.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(tool_choice) = tool_choice {
        translated.insert("tool_choice".to_string(), tool_choice);
    }

    let body = serialize_bounded(&Value::Object(translated), max_output_bytes)?;
    Ok(PreparedAnthropicRequest {
        body,
        requested_streaming,
        effective_usage_chunk: include_usage || inject_usage_tracking,
    })
}

fn reject_unknown_top_level_fields(request: &Map<String, Value>) -> Result<(), AnthropicRequestError> {
    if request.keys().any(|field| !TOP_LEVEL_FIELDS.contains(&field.as_str())) {
        return Err(AnthropicRequestError::UnsupportedParameter);
    }
    Ok(())
}

fn validate_client_model(request: &Map<String, Value>) -> Result<(), AnthropicRequestError> {
    match request.get("model") {
        Some(Value::String(model)) if !model.is_empty() => Ok(()),
        _ => Err(AnthropicRequestError::InvalidRequestBody),
    }
}

fn optional_bool(request: &Map<String, Value>, field: &str) -> Result<Option<bool>, AnthropicRequestError> {
    match request.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(AnthropicRequestError::InvalidRequestBody),
    }
}

fn parse_stream_options(
    request: &Map<String, Value>,
    requested_streaming: bool,
) -> Result<bool, AnthropicRequestError> {
    let Some(value) = request.get("stream_options") else {
        return Ok(false);
    };
    if value.is_null() {
        return Ok(false);
    }
    if !requested_streaming {
        return Err(AnthropicRequestError::UnsupportedParameter);
    }
    let options = value.as_object().ok_or(AnthropicRequestError::InvalidRequestBody)?;
    if options.len() != 1 || !options.contains_key("include_usage") {
        return Err(AnthropicRequestError::InvalidRequestBody);
    }
    options["include_usage"]
        .as_bool()
        .ok_or(AnthropicRequestError::InvalidRequestBody)
}

fn validate_compatibility_fields(request: &Map<String, Value>) -> Result<(), AnthropicRequestError> {
    validate_exact_integer_one(request.get("n"))?;
    validate_exact_numeric_one(request.get("temperature"))?;
    validate_exact_numeric_one(request.get("top_p"))?;
    Ok(())
}

fn validate_exact_integer_one(value: Option<&Value>) -> Result<(), AnthropicRequestError> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Number(number)) => match number.as_u64() {
            Some(1) => Ok(()),
            Some(_) => Err(AnthropicRequestError::UnsupportedParameter),
            None => Err(AnthropicRequestError::InvalidRequestBody),
        },
        Some(_) => Err(AnthropicRequestError::InvalidRequestBody),
    }
}

fn validate_exact_numeric_one(value: Option<&Value>) -> Result<(), AnthropicRequestError> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Number(number)) => match number.as_f64() {
            Some(1.0) => Ok(()),
            Some(_) => Err(AnthropicRequestError::UnsupportedParameter),
            None => Err(AnthropicRequestError::InvalidRequestBody),
        },
        Some(_) => Err(AnthropicRequestError::InvalidRequestBody),
    }
}

fn resolve_request_max_tokens(request: &Map<String, Value>) -> Result<Option<u32>, AnthropicRequestError> {
    let completion = optional_positive_u32(request.get("max_completion_tokens"))?;
    let legacy = optional_positive_u32(request.get("max_tokens"))?;
    match (completion, legacy) {
        (Some(left), Some(right)) if left != right => Err(AnthropicRequestError::InvalidRequestBody),
        (Some(value), _) | (_, Some(value)) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
}

fn optional_positive_u32(value: Option<&Value>) -> Result<Option<u32>, AnthropicRequestError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let value = number
                .as_u64()
                .filter(|value| (1..=u32::MAX as u64).contains(value))
                .ok_or(AnthropicRequestError::InvalidRequestBody)?;
            Ok(Some(value as u32))
        }
        Some(_) => Err(AnthropicRequestError::InvalidRequestBody),
    }
}

fn parse_stop(value: Option<&Value>) -> Result<Option<Vec<Value>>, AnthropicRequestError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(stop)) if !stop.is_empty() => Ok(Some(vec![Value::String(stop.clone())])),
        Some(Value::Array(stops)) if (1..=4).contains(&stops.len()) => {
            let mut translated = Vec::with_capacity(stops.len());
            for stop in stops {
                match stop {
                    Value::String(stop) if !stop.is_empty() => translated.push(Value::String(stop.clone())),
                    _ => return Err(AnthropicRequestError::InvalidRequestBody),
                }
            }
            Ok(Some(translated))
        }
        Some(_) => Err(AnthropicRequestError::InvalidRequestBody),
    }
}

fn parse_tools(value: Option<&Value>) -> Result<(Option<Vec<Value>>, HashSet<&str>), AnthropicRequestError> {
    let Some(value) = value else {
        return Ok((None, HashSet::new()));
    };
    if value.is_null() {
        return Ok((None, HashSet::new()));
    }
    let tools = value
        .as_array()
        .filter(|tools| !tools.is_empty())
        .ok_or(AnthropicRequestError::InvalidRequestBody)?;
    let mut names = HashSet::with_capacity(tools.len());
    let mut translated = Vec::with_capacity(tools.len());
    for tool in tools {
        let tool = tool.as_object().ok_or(AnthropicRequestError::InvalidRequestBody)?;
        require_exact_keys(tool, &["type", "function"])?;
        match tool.get("type") {
            Some(Value::String(kind)) if kind == "function" => {}
            Some(Value::String(_)) => return Err(AnthropicRequestError::UnsupportedParameter),
            _ => return Err(AnthropicRequestError::InvalidRequestBody),
        }
        let function = tool
            .get("function")
            .and_then(Value::as_object)
            .ok_or(AnthropicRequestError::InvalidRequestBody)?;
        if function.contains_key("strict") {
            return Err(AnthropicRequestError::UnsupportedParameter);
        }
        require_allowed_and_required_keys(
            function,
            &["name", "description", "parameters"],
            &["name", "parameters"],
        )?;
        let name = required_tool_name(function.get("name"))?;
        if !names.insert(name) {
            return Err(AnthropicRequestError::InvalidRequestBody);
        }
        let parameters = function
            .get("parameters")
            .filter(|value| value.is_object())
            .ok_or(AnthropicRequestError::InvalidRequestBody)?;
        let mut translated_tool = Map::new();
        translated_tool.insert("name".to_string(), Value::String(name.to_string()));
        if let Some(description) = function.get("description") {
            let description = description.as_str().ok_or(AnthropicRequestError::InvalidRequestBody)?;
            translated_tool.insert("description".to_string(), Value::String(description.to_string()));
        }
        translated_tool.insert("input_schema".to_string(), parameters.clone());
        translated.push(Value::Object(translated_tool));
    }
    Ok((Some(translated), names))
}

fn parse_tool_choice(
    choice: Option<&Value>,
    parallel: Option<&Value>,
    has_tools: bool,
    declared_tool_names: &HashSet<&str>,
) -> Result<Option<Value>, AnthropicRequestError> {
    let parallel = match parallel {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err(AnthropicRequestError::InvalidRequestBody),
    };
    let choice_is_present = choice.is_some_and(|value| !value.is_null());
    if !has_tools && (choice_is_present || parallel.is_some()) {
        return Err(AnthropicRequestError::UnsupportedParameter);
    }

    let mut translated = match choice {
        None | Some(Value::Null) => None,
        Some(Value::String(choice)) => {
            let kind = match choice.as_str() {
                "auto" => "auto",
                "required" => "any",
                "none" => "none",
                _ => return Err(AnthropicRequestError::UnsupportedParameter),
            };
            Some(json!({ "type": kind }))
        }
        Some(Value::Object(choice)) => {
            require_exact_keys(choice, &["type", "function"])?;
            match choice.get("type") {
                Some(Value::String(kind)) if kind == "function" => {}
                Some(Value::String(_)) => return Err(AnthropicRequestError::UnsupportedParameter),
                _ => return Err(AnthropicRequestError::InvalidRequestBody),
            }
            let function = choice
                .get("function")
                .and_then(Value::as_object)
                .ok_or(AnthropicRequestError::InvalidRequestBody)?;
            require_exact_keys(function, &["name"])?;
            let name = required_tool_name(function.get("name"))?;
            if !declared_tool_names.contains(name) {
                return Err(AnthropicRequestError::InvalidRequestBody);
            }
            Some(json!({ "type": "tool", "name": name }))
        }
        Some(_) => return Err(AnthropicRequestError::InvalidRequestBody),
    };

    if parallel == Some(false) {
        if translated.is_none() {
            translated = Some(json!({ "type": "auto" }));
        }
        let choice = translated
            .as_mut()
            .and_then(Value::as_object_mut)
            .ok_or(AnthropicRequestError::SerializationFailed)?;
        // Anthropic's `none` variant accepts only `type`; parallel tool use is
        // already impossible, so the OpenAI false flag is a no-op there.
        if choice.get("type").and_then(Value::as_str) != Some("none") {
            choice.insert("disable_parallel_tool_use".to_string(), Value::Bool(true));
        }
    }
    Ok(translated)
}

fn parse_messages(
    value: &Value,
    declared_tool_names: &HashSet<&str>,
) -> Result<(Vec<Value>, Vec<Value>), AnthropicRequestError> {
    let messages = value
        .as_array()
        .filter(|messages| !messages.is_empty())
        .ok_or(AnthropicRequestError::InvalidRequestBody)?;
    let mut system = Vec::new();
    let mut translated = Vec::with_capacity(messages.len());
    let mut seen_non_system = false;
    let mut pending_tool_ids = HashSet::new();
    let mut all_tool_ids = HashSet::new();
    let mut tool_results = Vec::new();

    for message in messages {
        let message = message.as_object().ok_or(AnthropicRequestError::InvalidRequestBody)?;
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or(AnthropicRequestError::InvalidRequestBody)?;

        if !pending_tool_ids.is_empty() && role != "tool" {
            return Err(AnthropicRequestError::InvalidRequestBody);
        }

        match role {
            "system" => {
                require_exact_keys(message, &["role", "content"])?;
                if seen_non_system {
                    return Err(AnthropicRequestError::UnsupportedParameter);
                }
                system.extend(parse_required_text_content(message.get("content"))?);
            }
            "user" => {
                require_exact_keys(message, &["role", "content"])?;
                seen_non_system = true;
                let content = parse_required_text_content(message.get("content"))?;
                translated.push(translated_message("user", content));
            }
            "assistant" => {
                require_allowed_and_required_keys(message, &["role", "content", "tool_calls"], &["role"])?;
                seen_non_system = true;
                let (mut content, has_meaningful_text) = parse_assistant_text_content(message.get("content"))?;
                let tool_calls = parse_assistant_tool_calls(
                    message.get("tool_calls"),
                    declared_tool_names,
                    &mut pending_tool_ids,
                    &mut all_tool_ids,
                )?;
                if !has_meaningful_text && tool_calls.is_empty() {
                    return Err(AnthropicRequestError::InvalidRequestBody);
                }
                content.extend(tool_calls);
                translated.push(translated_message("assistant", content));
            }
            "tool" => {
                require_exact_keys(message, &["role", "tool_call_id", "content"])?;
                seen_non_system = true;
                if pending_tool_ids.is_empty() {
                    return Err(AnthropicRequestError::InvalidRequestBody);
                }
                let tool_call_id = required_bounded_id(message.get("tool_call_id"))?;
                if !pending_tool_ids.remove(tool_call_id) {
                    return Err(AnthropicRequestError::InvalidRequestBody);
                }
                let content = parse_required_text_content(message.get("content"))?;
                let mut result = Map::new();
                result.insert("type".to_string(), Value::String("tool_result".to_string()));
                result.insert("tool_use_id".to_string(), Value::String(tool_call_id.to_string()));
                result.insert("content".to_string(), Value::Array(content));
                tool_results.push(Value::Object(result));
                if pending_tool_ids.is_empty() {
                    translated.push(translated_message("user", std::mem::take(&mut tool_results)));
                }
            }
            "developer" => return Err(AnthropicRequestError::UnsupportedParameter),
            _ => return Err(AnthropicRequestError::UnsupportedParameter),
        }
    }

    if !pending_tool_ids.is_empty() || !tool_results.is_empty() {
        return Err(AnthropicRequestError::InvalidRequestBody);
    }
    let ends_in_user_turn = translated
        .last()
        .and_then(Value::as_object)
        .and_then(|message| message.get("role").and_then(Value::as_str))
        == Some("user");
    if !ends_in_user_turn {
        return Err(AnthropicRequestError::UnsupportedParameter);
    }
    Ok((system, translated))
}

fn translated_message(role: &str, content: Vec<Value>) -> Value {
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String(role.to_string()));
    message.insert("content".to_string(), Value::Array(content));
    Value::Object(message)
}

fn parse_required_text_content(value: Option<&Value>) -> Result<Vec<Value>, AnthropicRequestError> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Ok(vec![json!({ "type": "text", "text": text })]),
        Some(Value::Array(parts)) if !parts.is_empty() => parse_text_parts(parts),
        _ => Err(AnthropicRequestError::InvalidRequestBody),
    }
}

fn parse_assistant_text_content(value: Option<&Value>) -> Result<(Vec<Value>, bool), AnthropicRequestError> {
    match value {
        None | Some(Value::Null) => Ok((Vec::new(), false)),
        Some(Value::String(text)) if text.is_empty() => Ok((Vec::new(), false)),
        Some(Value::String(text)) => Ok((vec![json!({ "type": "text", "text": text })], true)),
        Some(Value::Array(parts)) if !parts.is_empty() => Ok((parse_text_parts(parts)?, true)),
        Some(_) => Err(AnthropicRequestError::InvalidRequestBody),
    }
}

fn parse_text_parts(parts: &[Value]) -> Result<Vec<Value>, AnthropicRequestError> {
    let mut translated = Vec::with_capacity(parts.len());
    for part in parts {
        let part = part.as_object().ok_or(AnthropicRequestError::InvalidRequestBody)?;
        match part.get("type") {
            Some(Value::String(kind)) if kind == "text" => {}
            Some(Value::String(_)) => return Err(AnthropicRequestError::UnsupportedParameter),
            _ => return Err(AnthropicRequestError::InvalidRequestBody),
        }
        require_exact_keys(part, &["type", "text"])?;
        let text = part
            .get("text")
            .and_then(Value::as_str)
            .ok_or(AnthropicRequestError::InvalidRequestBody)?;
        translated.push(json!({ "type": "text", "text": text }));
    }
    Ok(translated)
}

fn parse_assistant_tool_calls<'a>(
    value: Option<&'a Value>,
    declared_tool_names: &HashSet<&str>,
    pending_tool_ids: &mut HashSet<&'a str>,
    all_tool_ids: &mut HashSet<&'a str>,
) -> Result<Vec<Value>, AnthropicRequestError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let calls = value
        .as_array()
        .filter(|calls| !calls.is_empty())
        .ok_or(AnthropicRequestError::InvalidRequestBody)?;
    let mut translated = Vec::with_capacity(calls.len());
    for call in calls {
        let call = call.as_object().ok_or(AnthropicRequestError::InvalidRequestBody)?;
        require_exact_keys(call, &["id", "type", "function"])?;
        let id = required_bounded_id(call.get("id"))?;
        if !all_tool_ids.insert(id) {
            return Err(AnthropicRequestError::InvalidRequestBody);
        }
        match call.get("type") {
            Some(Value::String(kind)) if kind == "function" => {}
            Some(Value::String(_)) => return Err(AnthropicRequestError::UnsupportedParameter),
            _ => return Err(AnthropicRequestError::InvalidRequestBody),
        }
        let function = call
            .get("function")
            .and_then(Value::as_object)
            .ok_or(AnthropicRequestError::InvalidRequestBody)?;
        require_exact_keys(function, &["name", "arguments"])?;
        let name = required_tool_name(function.get("name"))?;
        if !declared_tool_names.contains(name) {
            return Err(AnthropicRequestError::InvalidRequestBody);
        }
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .ok_or(AnthropicRequestError::InvalidRequestBody)?;
        let arguments: Value =
            serde_json::from_str(arguments).map_err(|_| AnthropicRequestError::InvalidRequestBody)?;
        if !arguments.is_object() {
            return Err(AnthropicRequestError::InvalidRequestBody);
        }
        pending_tool_ids.insert(id);
        let mut tool_use = Map::new();
        tool_use.insert("type".to_string(), Value::String("tool_use".to_string()));
        tool_use.insert("id".to_string(), Value::String(id.to_string()));
        tool_use.insert("name".to_string(), Value::String(name.to_string()));
        tool_use.insert("input".to_string(), arguments);
        translated.push(Value::Object(tool_use));
    }
    Ok(translated)
}

fn required_bounded_id(value: Option<&Value>) -> Result<&str, AnthropicRequestError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= MAX_TOOL_CALL_ID_BYTES)
        .ok_or(AnthropicRequestError::InvalidRequestBody)
}

fn required_tool_name(value: Option<&Value>) -> Result<&str, AnthropicRequestError> {
    value
        .and_then(Value::as_str)
        .filter(|name| {
            !name.is_empty()
                && name.len() <= MAX_TOOL_NAME_BYTES
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
        .ok_or(AnthropicRequestError::InvalidRequestBody)
}

fn require_exact_keys(object: &Map<String, Value>, expected: &[&str]) -> Result<(), AnthropicRequestError> {
    if object.len() != expected.len()
        || expected.iter().any(|field| !object.contains_key(*field))
        || object.keys().any(|field| !expected.contains(&field.as_str()))
    {
        return Err(AnthropicRequestError::InvalidRequestBody);
    }
    Ok(())
}

fn require_allowed_and_required_keys(
    object: &Map<String, Value>,
    allowed: &[&str],
    required: &[&str],
) -> Result<(), AnthropicRequestError> {
    if required.iter().any(|field| !object.contains_key(*field))
        || object.keys().any(|field| !allowed.contains(&field.as_str()))
    {
        return Err(AnthropicRequestError::InvalidRequestBody);
    }
    Ok(())
}

fn serialize_bounded(value: &Value, max_output_bytes: usize) -> Result<Vec<u8>, AnthropicRequestError> {
    let mut writer = BoundedWriter::new(max_output_bytes);
    if serde_json::to_writer(&mut writer, value).is_err() {
        return if writer.overflowed {
            Err(AnthropicRequestError::OutputTooLarge)
        } else {
            Err(AnthropicRequestError::SerializationFailed)
        };
    }
    Ok(writer.bytes)
}

struct BoundedWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    overflowed: bool,
}

impl BoundedWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(max_bytes.min(8 * 1024)),
            max_bytes,
            overflowed: false,
        }
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() > self.max_bytes.saturating_sub(self.bytes.len()) {
            self.overflowed = true;
            return Err(io::Error::other("translated request exceeds the output limit"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/conversion/chat_to_anthropic/request_test.rs"]
mod tests;
