//! Bounded Draft 2020-12 JSON Schema validation shared by Controller and Gateway.

use std::collections::HashSet;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};

use jsonschema::{Draft, PatternOptions, Retrieve, Uri, Validator};
use schemars::JsonSchema;
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::resources::edgion_plugins::{BodyRequirement, PluginValidation};

pub const DEFAULT_INSTANCE_MAX_BYTES: u64 = 64 * 1024;
pub const ABSOLUTE_INSTANCE_MAX_BYTES: u64 = 1024 * 1024;

const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_SCHEMA_NODES: usize = 4_096;
const MAX_REFERENCE_DEPTH: usize = 32;
const MAX_PATTERN_COUNT: usize = 64;
const MAX_PATTERN_SOURCE_BYTES: usize = 16 * 1024;
const REGEX_SIZE_LIMIT: usize = 256 * 1024;
const REGEX_DFA_SIZE_LIMIT: usize = 256 * 1024;
pub const MAX_JSON_INSTANCE_DEPTH: usize = 64;
pub const MAX_JSON_INSTANCE_NODES: u64 = 20_000;
const MAX_LOGICAL_DEPTH: usize = 64;
const MAX_VALIDATION_WORK: u64 = 4_000_000;
const DRAFT_2020_12_URI: &str = "https://json-schema.org/draft/2020-12/schema";

/// Canonical entry-body validation shared by Controller and Gateway.
///
/// Returned fragments include the `body` field name and can be appended to an
/// entry-specific error prefix. Multiple independent errors are preserved.
pub fn bounded_json_body_validation_errors(body: &BodyRequirement) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(error) = body.validation_error() {
        errors.push(format!("body: {error}"));
    }
    if body.conditions.is_some() {
        errors.push("body: conditions are not supported".to_string());
    }
    if body
        .max_body_size_bytes()
        .and_then(Result::ok)
        .is_some_and(|limit| limit > ABSOLUTE_INSTANCE_MAX_BYTES)
    {
        errors.push("body.maxBodySize must not exceed 1MiB".to_string());
    }
    errors
}

/// Configuration for request-body JSON Schema validation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JsonSchemaValidationConfig {
    /// Inline Draft 2020-12 schema.
    pub schema: Value,
}

impl JsonSchemaValidationConfig {
    /// Validate policy and compile an immutable validator.
    pub fn compile(&self) -> Result<CompiledJsonSchema, String> {
        match catch_unwind(AssertUnwindSafe(|| compile_inner(&self.schema))) {
            Ok(result) => result,
            Err(_) => Err("JSON Schema compilation panicked".to_string()),
        }
    }
}

impl PluginValidation for JsonSchemaValidationConfig {
    fn validation_error(&self) -> Option<String> {
        self.compile().err()
    }
}

/// Conservative request-work profile produced while compiling a schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaComplexityProfile {
    pub node_ops: u64,
    pub scan_ops: u64,
    pub logical_depth: usize,
}

/// Immutable validator published in the Gateway runtime.
#[derive(Clone, Debug)]
pub struct CompiledJsonSchema {
    validator: Validator,
    profile: SchemaComplexityProfile,
}

impl CompiledJsonSchema {
    pub fn profile(&self) -> SchemaComplexityProfile {
        self.profile
    }

    /// Apply the combined static-schema and parsed-instance work budget, then
    /// perform boolean validation without allocating an attacker-controlled
    /// error list.
    pub fn validate_instance(&self, instance: &ParsedJsonInstance) -> Result<bool, InstanceValidationError> {
        let node_work = self
            .profile
            .node_ops
            .checked_mul(instance.node_count)
            .ok_or(InstanceValidationError::WorkBudgetExceeded)?;
        let scan_work = self
            .profile
            .scan_ops
            .checked_mul(instance.text_bytes)
            .ok_or(InstanceValidationError::WorkBudgetExceeded)?;
        let total = node_work
            .checked_add(scan_work)
            .ok_or(InstanceValidationError::WorkBudgetExceeded)?;
        if total > MAX_VALIDATION_WORK {
            return Err(InstanceValidationError::WorkBudgetExceeded);
        }

        catch_unwind(AssertUnwindSafe(|| self.validator.is_valid(&instance.value)))
            .map_err(|_| InstanceValidationError::ValidatorPanicked)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InstanceValidationError {
    #[error("JSON instance exceeds the validation work budget")]
    WorkBudgetExceeded,
    #[error("JSON Schema validator panicked")]
    ValidatorPanicked,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonValueProfile {
    pub node_count: u64,
    pub text_bytes: u64,
    pub max_depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StrictJsonError {
    #[error("JSON instance exceeds the absolute byte limit")]
    TooLarge,
    #[error("JSON instance is malformed")]
    Malformed,
    #[error("JSON instance contains duplicate object keys")]
    DuplicateKey,
    #[error("JSON instance exceeds the nesting-depth limit")]
    DepthLimit,
    #[error("JSON instance exceeds the node-count limit")]
    NodeLimit,
}

/// Parse one UTF-8 JSON value, rejecting duplicate object keys and collecting
/// node/text accounting in the same pass.
pub fn parse_json_instance(bytes: &[u8]) -> Result<ParsedJsonInstance, StrictJsonError> {
    if bytes.len() as u64 > ABSOLUTE_INSTANCE_MAX_BYTES {
        return Err(StrictJsonError::TooLarge);
    }

    let mut state = ParseState::default();
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

/// Validate and profile an already parsed JSON value using the same depth and
/// node limits as [`parse_json_instance`].
pub fn profile_json_value(value: &Value) -> Result<JsonValueProfile, StrictJsonError> {
    let mut node_count = 0_u64;
    let mut text_bytes = 0_u64;
    let mut max_depth = 0_usize;
    let mut stack = vec![(value, 1_usize)];

    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_JSON_INSTANCE_DEPTH {
            return Err(StrictJsonError::DepthLimit);
        }
        node_count = node_count.saturating_add(1);
        if node_count > MAX_JSON_INSTANCE_NODES {
            return Err(StrictJsonError::NodeLimit);
        }
        max_depth = max_depth.max(depth);
        match value {
            Value::Number(value) => text_bytes = text_bytes.saturating_add(value.to_string().len() as u64),
            Value::String(value) => {
                text_bytes = text_bytes.saturating_add(value.len() as u64);
            }
            Value::Array(values) => {
                stack.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                for (key, value) in values {
                    text_bytes = text_bytes.saturating_add(key.len() as u64);
                    stack.push((value, depth + 1));
                }
            }
            _ => {}
        }
    }

    Ok(JsonValueProfile {
        node_count,
        text_bytes,
        max_depth,
    })
}

#[derive(Default)]
struct ParseState {
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
        if self.depth > MAX_JSON_INSTANCE_DEPTH {
            self.state.failure = Some(StrictJsonError::DepthLimit);
            return Err(D::Error::custom("JSON nesting limit exceeded"));
        }
        self.state.node_count = self.state.node_count.saturating_add(1);
        if self.state.node_count > MAX_JSON_INSTANCE_NODES {
            self.state.failure = Some(StrictJsonError::NodeLimit);
            return Err(D::Error::custom("JSON node limit exceeded"));
        }
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
        let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX_JSON_INSTANCE_NODES as usize));
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
        let capacity = map.size_hint().unwrap_or(0).min(MAX_JSON_INSTANCE_NODES as usize);
        let mut values = Map::with_capacity(capacity);
        let mut keys = HashSet::with_capacity(capacity);
        while let Some(key) = map.next_key::<String>()? {
            self.state.text_bytes = self.state.text_bytes.saturating_add(key.len() as u64);
            if !keys.insert(key.clone()) {
                self.state.failure = Some(StrictJsonError::DuplicateKey);
                return Err(A::Error::custom("duplicate JSON object key"));
            }
            let value = map.next_value_seed(ValueSeed {
                state: self.state,
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

#[derive(Debug)]
struct DenyAllRetriever;

impl Retrieve for DenyAllRetriever {
    fn retrieve(&self, _uri: &Uri<String>) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external JSON Schema retrieval is disabled".into())
    }
}

fn compile_inner(schema: &Value) -> Result<CompiledJsonSchema, String> {
    let serialized = serde_json::to_vec(schema).map_err(|_| "JSON Schema cannot be serialized".to_string())?;
    if serialized.len() > MAX_SCHEMA_BYTES {
        return Err(format!("JSON Schema exceeds the {MAX_SCHEMA_BYTES}-byte limit"));
    }
    validate_json_shape(schema)?;
    validate_schema_policy(schema)?;

    let mut expansion_steps = 0_u64;
    let profile = profile_schema(schema, schema, 1, 0, &mut Vec::new(), &mut expansion_steps)?;
    if profile.logical_depth > MAX_LOGICAL_DEPTH {
        return Err(format!(
            "JSON Schema evaluation depth exceeds the {MAX_LOGICAL_DEPTH}-level limit"
        ));
    }

    let validator = jsonschema::options()
        .with_draft(Draft::Draft202012)
        .should_validate_formats(true)
        .with_pattern_options(
            PatternOptions::regex()
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_DFA_SIZE_LIMIT),
        )
        .with_retriever(DenyAllRetriever)
        .build(schema)
        .map_err(|_| "JSON Schema is invalid for the restricted Draft 2020-12 profile".to_string())?;

    Ok(CompiledJsonSchema { validator, profile })
}

fn validate_json_shape(root: &Value) -> Result<(), String> {
    let mut stack = vec![(root, 1_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_SCHEMA_DEPTH {
            return Err(format!(
                "JSON Schema nesting exceeds the {MAX_SCHEMA_DEPTH}-level limit"
            ));
        }
        nodes = nodes.saturating_add(1);
        if nodes > MAX_SCHEMA_NODES {
            return Err(format!("JSON Schema exceeds the {MAX_SCHEMA_NODES}-node limit"));
        }
        match value {
            Value::Array(items) => stack.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(object) => stack.extend(object.values().map(|item| (item, depth + 1))),
            _ => {}
        }
    }
    Ok(())
}

fn validate_schema_policy(root: &Value) -> Result<(), String> {
    let mut stack = vec![root];
    let mut visited = HashSet::new();
    let mut pattern_count = 0_usize;
    let mut pattern_bytes = 0_usize;
    while let Some(schema) = stack.pop() {
        // A schema node may be reached through both its ordinary location
        // (for example `$defs`) and one or more `$ref` values. Inspect each
        // actual node once so references cannot hide restricted keywords while
        // repeated references do not multiply the configured pattern quota.
        if !visited.insert(std::ptr::from_ref(schema)) {
            continue;
        }
        let Value::Object(object) = schema else {
            if schema.is_boolean() {
                continue;
            }
            return Err("JSON Schema positions must contain an object or boolean".to_string());
        };

        for keyword in [
            "$id",
            "$anchor",
            "$dynamicAnchor",
            "$dynamicRef",
            "$recursiveRef",
            "unevaluatedProperties",
        ] {
            if object.contains_key(keyword) {
                return Err(format!("JSON Schema keyword '{keyword}' is not supported"));
            }
        }
        if let Some(draft) = object.get("$schema") {
            if draft.as_str() != Some(DRAFT_2020_12_URI) {
                return Err("JSON Schema '$schema' must identify Draft 2020-12".to_string());
            }
        }
        if let Some(reference) = object.get("$ref") {
            let Some(reference) = reference.as_str() else {
                return Err("JSON Schema '$ref' must be a string".to_string());
            };
            let Some(pointer) = reference.strip_prefix('#') else {
                return Err("JSON Schema '$ref' must be a root-document JSON Pointer".to_string());
            };
            if !pointer.starts_with('/') {
                return Err("JSON Schema '$ref' must use '#/...' JSON Pointer form".to_string());
            }
            validate_pointer_encoding(pointer)?;
            let target = root
                .pointer(pointer)
                .ok_or_else(|| "JSON Schema '$ref' target does not exist".to_string())?;
            stack.push(target);
        }
        // jsonschema 0.40 compiles the attacker-controlled instance string as a
        // fancy-regex during validation. PatternOptions does not bound that
        // request-time compilation, so this built-in format is excluded.
        if object.get("format").and_then(Value::as_str) == Some("regex") {
            return Err("JSON Schema format 'regex' is not supported".to_string());
        }

        if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
            add_pattern(pattern, &mut pattern_count, &mut pattern_bytes)?;
        }
        if let Some(patterns) = object.get("patternProperties").and_then(Value::as_object) {
            for pattern in patterns.keys() {
                add_pattern(pattern, &mut pattern_count, &mut pattern_bytes)?;
            }
        }
        stack.extend(schema_children(object, true));
    }
    Ok(())
}

fn add_pattern(pattern: &str, count: &mut usize, bytes: &mut usize) -> Result<(), String> {
    *count = count.saturating_add(1);
    *bytes = bytes.saturating_add(pattern.len());
    if *count > MAX_PATTERN_COUNT {
        return Err(format!("JSON Schema exceeds the {MAX_PATTERN_COUNT}-pattern limit"));
    }
    if *bytes > MAX_PATTERN_SOURCE_BYTES {
        return Err(format!(
            "JSON Schema pattern source exceeds the {MAX_PATTERN_SOURCE_BYTES}-byte aggregate limit"
        ));
    }
    Ok(())
}

fn validate_pointer_encoding(pointer: &str) -> Result<(), String> {
    // The validator URI-decodes fragments before applying JSON Pointer
    // escapes. Keep the v1 subset unambiguous by rejecting percent-encoded
    // fragments instead of maintaining a second URI-decoder whose target
    // selection could drift from the engine.
    if pointer.contains('%') {
        return Err("JSON Schema '$ref' percent-encoded fragments are not supported".to_string());
    }

    let bytes = pointer.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'~' {
            if i + 1 >= bytes.len() || !matches!(bytes[i + 1], b'0' | b'1') {
                return Err("JSON Schema '$ref' contains an invalid JSON Pointer escape".to_string());
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    Ok(())
}

fn profile_schema(
    schema: &Value,
    root: &Value,
    logical_depth: usize,
    reference_depth: usize,
    reference_stack: &mut Vec<String>,
    expansion_steps: &mut u64,
) -> Result<SchemaComplexityProfile, String> {
    if logical_depth > MAX_LOGICAL_DEPTH {
        return Err(format!(
            "JSON Schema evaluation depth exceeds the {MAX_LOGICAL_DEPTH}-level limit"
        ));
    }
    *expansion_steps = expansion_steps.saturating_add(1);
    if *expansion_steps > MAX_VALIDATION_WORK {
        return Err("JSON Schema expanded complexity exceeds the validation work budget".to_string());
    }

    let mut profile = SchemaComplexityProfile {
        node_ops: 1,
        scan_ops: 0,
        logical_depth,
    };
    let Value::Object(object) = schema else {
        return Ok(profile);
    };

    // Count validator keywords, not only schema objects. Annotation-only
    // keywords do not execute against the instance; every other keyword gets
    // at least one operation before its schema-bearing children are expanded.
    profile.node_ops = profile.node_ops.saturating_add(
        object
            .keys()
            .filter(|keyword| !is_annotation_only_keyword(keyword))
            .count() as u64,
    );
    for keyword in ["required", "dependentRequired"] {
        if let Some(value) = object.get(keyword) {
            let members = array_member_count(value);
            profile.node_ops = profile.node_ops.saturating_add(members);
            profile.scan_ops = profile.scan_ops.saturating_add(members);
        }
    }
    for keyword in ["properties", "patternProperties", "dependentSchemas"] {
        if let Some(values) = object.get(keyword).and_then(Value::as_object) {
            profile.node_ops = profile.node_ops.saturating_add(values.len() as u64);
            profile.scan_ops = profile.scan_ops.saturating_add(values.len() as u64);
        }
    }
    // jsonschema uses pairwise deep comparisons for arrays of up to 15 items
    // before switching to hashing. Charge the worst fixed node and text-scan
    // fanout so long strings cannot bypass the request work budget.
    if object.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
        profile.node_ops = profile.node_ops.saturating_add(15);
        profile.scan_ops = profile.scan_ops.saturating_add(15);
    }
    // `const` and `enum` perform deep equality checks. Charge every candidate
    // JSON node and every candidate string scan so large literals cannot hide
    // behind one schema-object operation.
    for keyword in ["const", "enum"] {
        if let Some(value) = object.get(keyword) {
            let (literal_nodes, literal_strings) = literal_comparison_cost(value);
            profile.node_ops = profile.node_ops.saturating_add(literal_nodes);
            profile.scan_ops = profile.scan_ops.saturating_add(literal_strings);
        }
    }

    for keyword in ["format", "maxLength", "minLength", "pattern"] {
        if object.contains_key(keyword) {
            profile.scan_ops = profile.scan_ops.saturating_add(1);
        }
    }

    for child in schema_children(object, false) {
        merge_profile(
            &mut profile,
            profile_schema(
                child,
                root,
                logical_depth + 1,
                reference_depth,
                reference_stack,
                expansion_steps,
            )?,
        )?;
    }

    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let next_reference_depth = reference_depth.saturating_add(1);
        if next_reference_depth > MAX_REFERENCE_DEPTH {
            return Err(format!(
                "JSON Schema reference chain exceeds the {MAX_REFERENCE_DEPTH}-reference limit"
            ));
        }
        if reference_stack.iter().any(|seen| seen == reference) {
            return Err("JSON Schema local reference cycles are not supported".to_string());
        }
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| "JSON Schema '$ref' must be local".to_string())?;
        let target = root
            .pointer(pointer)
            .ok_or_else(|| "JSON Schema '$ref' target does not exist".to_string())?;
        if !target.is_object() && !target.is_boolean() {
            return Err("JSON Schema '$ref' target must be an object or boolean schema".to_string());
        }
        reference_stack.push(reference.to_string());
        let target_profile = profile_schema(
            target,
            root,
            logical_depth + 1,
            next_reference_depth,
            reference_stack,
            expansion_steps,
        );
        reference_stack.pop();
        merge_profile(&mut profile, target_profile?)?;
    }
    Ok(profile)
}

fn is_annotation_only_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "$comment"
            | "$defs"
            | "$schema"
            | "default"
            | "deprecated"
            | "description"
            | "examples"
            | "readOnly"
            | "title"
            | "writeOnly"
    )
}

fn array_member_count(value: &Value) -> u64 {
    match value {
        Value::Array(values) => values.len() as u64,
        Value::Object(values) => {
            values.len() as u64
                + values
                    .values()
                    .filter_map(Value::as_array)
                    .map(|values| values.len() as u64)
                    .sum::<u64>()
        }
        _ => 0,
    }
}

fn literal_comparison_cost(root: &Value) -> (u64, u64) {
    let mut nodes = 0_u64;
    let mut strings = 0_u64;
    let mut stack = vec![root];
    while let Some(value) = stack.pop() {
        nodes = nodes.saturating_add(1);
        match value {
            Value::String(_) => strings = strings.saturating_add(1),
            Value::Array(values) => stack.extend(values),
            Value::Object(values) => {
                strings = strings.saturating_add(values.len() as u64);
                stack.extend(values.values());
            }
            _ => {}
        }
    }
    (nodes, strings)
}

fn merge_profile(target: &mut SchemaComplexityProfile, child: SchemaComplexityProfile) -> Result<(), String> {
    target.node_ops = target.node_ops.saturating_add(child.node_ops);
    target.scan_ops = target.scan_ops.saturating_add(child.scan_ops);
    target.logical_depth = target.logical_depth.max(child.logical_depth);
    if target.node_ops > MAX_VALIDATION_WORK || target.scan_ops > MAX_VALIDATION_WORK {
        return Err("JSON Schema expanded complexity exceeds the validation work budget".to_string());
    }
    Ok(())
}

fn schema_children(object: &Map<String, Value>, include_definitions: bool) -> Vec<&Value> {
    let mut children = Vec::new();
    for keyword in [
        "additionalProperties",
        "contains",
        "contentSchema",
        "else",
        "if",
        "items",
        "not",
        "propertyNames",
        "then",
        "unevaluatedItems",
    ] {
        if let Some(value) = object.get(keyword) {
            children.push(value);
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(values) = object.get(keyword).and_then(Value::as_array) {
            children.extend(values);
        }
    }
    for keyword in ["properties", "patternProperties", "dependentSchemas"] {
        if let Some(values) = object.get(keyword).and_then(Value::as_object) {
            children.extend(values.values());
        }
    }
    if include_definitions {
        if let Some(values) = object.get("$defs").and_then(Value::as_object) {
            children.extend(values.values());
        }
    }
    children
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(schema: Value) -> JsonSchemaValidationConfig {
        JsonSchemaValidationConfig { schema }
    }

    #[test]
    fn compiles_and_validates_local_pointer_schema() {
        let compiled = config(json!({
            "$schema": DRAFT_2020_12_URI,
            "$defs": {"name": {"type": "string", "minLength": 1}},
            "type": "object",
            "properties": {"name": {"$ref": "#/$defs/name"}},
            "required": ["name"]
        }))
        .compile()
        .unwrap();

        let valid = parse_json_instance(br#"{"name":"Ada"}"#).unwrap();
        let invalid = parse_json_instance(br#"{"name":""}"#).unwrap();
        assert_eq!(compiled.validate_instance(&valid), Ok(true));
        assert_eq!(compiled.validate_instance(&invalid), Ok(false));
    }

    #[test]
    fn rejects_wrong_draft_and_restricted_keywords() {
        assert!(
            config(json!({"$schema": "http://json-schema.org/draft-07/schema#", "type": "string"}))
                .compile()
                .unwrap_err()
                .contains("Draft 2020-12")
        );
        for keyword in [
            "$id",
            "$anchor",
            "$dynamicAnchor",
            "$dynamicRef",
            "$recursiveRef",
            "unevaluatedProperties",
        ] {
            let mut schema = Map::new();
            schema.insert(keyword.to_string(), Value::String("#x".to_string()));
            assert!(config(Value::Object(schema)).compile().unwrap_err().contains(keyword));
        }
    }

    #[test]
    fn rejects_external_unresolved_cyclic_and_bad_pointer_refs() {
        for schema in [
            json!({"$ref": "https://example.com/schema"}),
            json!({"$ref": "#/$defs/missing"}),
            json!({"$ref": "#/$defs/a", "$defs": {"a": {"$ref": "#/$defs/a"}}}),
            json!({"$ref": "#/$defs/~2bad", "$defs": {"~2bad": true}}),
        ] {
            assert!(config(schema).compile().is_err());
        }
    }

    #[test]
    fn rejects_schema_shape_and_reference_depth_limits() {
        let mut deep = Value::Bool(true);
        for _ in 0..MAX_SCHEMA_DEPTH {
            deep = json!({"allOf": [deep]});
        }
        assert!(config(deep).compile().unwrap_err().contains("nesting"));

        let too_many_nodes = json!({"examples": vec![Value::Null; MAX_SCHEMA_NODES]});
        assert!(config(too_many_nodes).compile().unwrap_err().contains("node"));

        let mut definitions = Map::new();
        for index in 0..=MAX_REFERENCE_DEPTH {
            let target = if index == MAX_REFERENCE_DEPTH {
                Value::Bool(true)
            } else {
                json!({"$ref": format!("#/$defs/n{}", index + 1)})
            };
            definitions.insert(format!("n{index}"), target);
        }
        assert!(config(json!({
            "$ref": "#/$defs/n0",
            "$defs": definitions
        }))
        .compile()
        .unwrap_err()
        .contains("reference chain"));
    }

    #[test]
    fn referenced_annotation_data_cannot_bypass_schema_policy() {
        for schema in [
            json!({
                "$ref": "#/default",
                "default": {"format": "regex"}
            }),
            json!({
                "$ref": "#/x-internal",
                "x-internal": {"unevaluatedProperties": false}
            }),
            json!({
                "$ref": "#/examples/0",
                "examples": [{"$id": "https://example.com/hidden"}]
            }),
        ] {
            assert!(config(schema).compile().is_err());
        }
    }

    #[test]
    fn percent_encoded_ref_cannot_select_a_different_validator_target() {
        let error = config(json!({
            "$ref": "#/$defs/safe%2Fname",
            "$defs": {
                "safe%2Fname": true,
                "safe": {
                    "name": {"format": "regex"}
                }
            }
        }))
        .compile()
        .unwrap_err();
        assert!(error.contains("percent-encoded"), "{error}");
    }

    #[test]
    fn referenced_annotation_patterns_are_bounded_and_counted_once() {
        let patterns: Map<String, Value> = (0..=MAX_PATTERN_COUNT)
            .map(|i| (format!("p{i}"), Value::Bool(true)))
            .collect();
        assert!(config(json!({
            "$ref": "#/default",
            "default": {"patternProperties": patterns}
        }))
        .compile()
        .unwrap_err()
        .contains("pattern"));

        config(json!({
            "allOf": [
                {"$ref": "#/$defs/shared"},
                {"$ref": "#/$defs/shared"}
            ],
            "$defs": {
                "shared": {"pattern": "^shared$"}
            }
        }))
        .compile()
        .unwrap();
    }

    #[test]
    fn schema_walker_ignores_keyword_like_annotation_data() {
        config(json!({
            "type": "object",
            "const": {"$id": "not-a-schema", "unevaluatedProperties": false},
            "default": {"$dynamicRef": "not-a-schema"},
            "examples": [{"$anchor": "not-a-schema"}]
        }))
        .compile()
        .unwrap();
    }

    #[test]
    fn strict_parser_rejects_duplicates_depth_and_trailing_data() {
        assert_eq!(
            parse_json_instance(br#"{"a":1,"a":2}"#).unwrap_err(),
            StrictJsonError::DuplicateKey
        );
        assert_eq!(
            parse_json_instance(br#"{} {}"#).unwrap_err(),
            StrictJsonError::Malformed
        );

        let deep = format!(
            "{}0{}",
            "[".repeat(MAX_JSON_INSTANCE_DEPTH),
            "]".repeat(MAX_JSON_INSTANCE_DEPTH)
        );
        assert_eq!(
            parse_json_instance(deep.as_bytes()).unwrap_err(),
            StrictJsonError::DepthLimit
        );
    }

    #[test]
    fn strict_parser_counts_nodes_and_text() {
        let parsed = parse_json_instance(br#"{"name":"Ada","tags":["x","yz"]}"#).unwrap();
        assert_eq!(parsed.node_count(), 5);
        assert_eq!(parsed.text_bytes(), 14);
    }

    #[test]
    fn strict_parser_rejects_instance_node_overflow() {
        let encoded = serde_json::to_vec(&Value::Array(vec![Value::Null; MAX_JSON_INSTANCE_NODES as usize])).unwrap();
        assert_eq!(parse_json_instance(&encoded).unwrap_err(), StrictJsonError::NodeLimit);
    }

    #[test]
    fn strict_parser_preserves_private_token_object_keys() {
        let parsed = parse_json_instance(br#"{"$serde_json::private::Number":"1","other":true}"#)
            .unwrap()
            .into_value();
        assert_eq!(
            parsed.pointer("/$serde_json::private::Number"),
            Some(&Value::String("1".to_string()))
        );
        assert_eq!(parsed.pointer("/other"), Some(&Value::Bool(true)));
    }

    #[test]
    fn format_assertions_are_enabled_and_unknown_formats_are_annotations() {
        let email = config(json!({"type": "string", "format": "email"})).compile().unwrap();
        assert!(!email
            .validate_instance(&parse_json_instance(br#""not-an-email""#).unwrap())
            .unwrap());
        let unknown = config(json!({"type": "string", "format": "edgion-unknown"}))
            .compile()
            .unwrap();
        assert!(unknown
            .validate_instance(&parse_json_instance(br#""anything""#).unwrap())
            .unwrap());
    }

    #[test]
    fn rejects_regex_format_that_compiles_instance_text_at_request_time() {
        assert!(config(json!({"type": "string", "format": "regex"}))
            .compile()
            .unwrap_err()
            .contains("format 'regex'"));
    }

    #[test]
    fn rejects_pattern_and_schema_limits() {
        let patterns: Map<String, Value> = (0..=MAX_PATTERN_COUNT)
            .map(|i| (format!("p{i}"), Value::Bool(true)))
            .collect();
        assert!(config(json!({"patternProperties": patterns}))
            .compile()
            .unwrap_err()
            .contains("pattern"));

        let half_plus_one = MAX_PATTERN_SOURCE_BYTES / 2 + 1;
        let aggregate_patterns = Map::from_iter([
            ("a".repeat(half_plus_one), Value::Bool(true)),
            ("b".repeat(half_plus_one), Value::Bool(true)),
        ]);
        assert!(config(json!({"patternProperties": aggregate_patterns}))
            .compile()
            .unwrap_err()
            .contains("pattern source"));

        assert!(config(json!({"pattern": "a{1000000}"})).compile().is_err());

        let huge = "x".repeat(MAX_SCHEMA_BYTES);
        assert!(config(json!({"description": huge})).compile().is_err());
    }

    #[test]
    fn rejects_absolute_instance_byte_overflow() {
        let bytes = vec![b' '; ABSOLUTE_INSTANCE_MAX_BYTES as usize + 1];
        assert_eq!(parse_json_instance(&bytes).unwrap_err(), StrictJsonError::TooLarge);
    }

    #[test]
    fn profiles_deep_literal_and_property_costs() {
        let compiled = config(json!({
            "type": "object",
            "required": ["kind", "payload"],
            "properties": {
                "kind": {"enum": ["one", "two", "three"]},
                "payload": {"const": {"nested": ["a", "b", "c"]}}
            }
        }))
        .compile()
        .unwrap();
        let profile = compiled.profile();
        assert!(profile.node_ops >= 20, "{profile:?}");
        assert!(profile.scan_ops >= 7, "{profile:?}");
    }

    #[test]
    fn profiles_unique_items_pairwise_comparison_fanout() {
        let compiled = config(json!({"type": "array", "uniqueItems": true})).compile().unwrap();
        let profile = compiled.profile();
        assert!(profile.node_ops >= 18, "{profile:?}");
        assert!(profile.scan_ops >= 15, "{profile:?}");

        let instance = Value::Array(
            (0..15)
                .map(|index| Value::String(format!("{}{:02}", "x".repeat(20_000), index)))
                .collect(),
        );
        let encoded = serde_json::to_vec(&instance).unwrap();
        let parsed = parse_json_instance(&encoded).unwrap();
        assert_eq!(
            compiled.validate_instance(&parsed),
            Err(InstanceValidationError::WorkBudgetExceeded)
        );
    }
}
