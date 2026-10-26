//! Static GraphQL request adaptation with canonical local key sources.

use super::proxy_rewrite::compile_pointer;
use super::{parse_json_instance_with_limits, profile_json_value};
use crate::common::KeyGet;
use crate::graphql_request::{envelope::decode, GraphqlError, GraphqlOperationType, ParsedGraphqlRequest, TypeRef};
use crate::resources::edgion_plugins::PluginValidation;
use http::{HeaderName, Uri};
use jsonptr::PointerBuf;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashSet;

pub const REST_GRAPHQL_MAX_VALUE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GraphqlVariableFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlVariableBinding {
    pub name: String,
    #[serde(default)]
    pub from: Option<KeyGet>,
    #[serde(default)]
    pub body_pointer: Option<String>,
    #[serde(default)]
    pub format: GraphqlVariableFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestToGraphqlConfig {
    pub query: String,
    #[serde(default)]
    pub operation_name: Option<String>,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default)]
    pub variables: Vec<GraphqlVariableBinding>,
}

fn default_path() -> String {
    "/graphql".into()
}

#[derive(Debug)]
pub struct CompiledRestToGraphql {
    pub query: String,
    pub operation_name: Option<String>,
    pub path: String,
    pub operation_type: GraphqlOperationType,
    bindings: Vec<CompiledBinding>,
}

#[derive(Debug)]
struct CompiledBinding {
    source: GraphqlVariableBinding,
    pointer: Option<PointerBuf>,
    ty: TypeRef,
    has_default: bool,
}

impl RestToGraphqlConfig {
    pub fn compile(&self) -> Result<CompiledRestToGraphql, String> {
        if self.query.len() > REST_GRAPHQL_MAX_VALUE_BYTES || self.variables.len() > 64 {
            return Err("query exceeds 16KiB or variables exceeds 64 bindings".into());
        }
        let uri: Uri = self.path.parse().map_err(|_| "invalid GraphQL target path")?;
        if self.path.len() > 2048
            || !self.path.starts_with('/')
            || self.path.starts_with("//")
            || uri.scheme().is_some()
            || uri.authority().is_some()
            || uri.query().is_some()
            || self.path.contains('#')
        {
            return Err("path must be an origin-form path without query or fragment".into());
        }
        let parsed = ParsedGraphqlRequest::parse_registered(&self.query, self.operation_name.as_deref())
            .map_err(|_| "invalid static GraphQL document or selector")?;
        let mut names = HashSet::new();
        let mut bindings = Vec::with_capacity(self.variables.len());
        for binding in &self.variables {
            if !names.insert(binding.name.as_str()) {
                return Err("duplicate GraphQL variable mapping".into());
            }
            let definition = parsed
                .vars
                .get(&binding.name)
                .ok_or("mapping references undeclared GraphQL variable")?;
            if binding.from.is_some() == binding.body_pointer.is_some() {
                return Err("each variable requires exactly one of from or bodyPointer".into());
            }
            if let Some(key) = &binding.from {
                if key.name().is_some_and(|name| name.is_empty() || name.len() > 256) {
                    return Err("variable source name must contain 1..256 bytes".into());
                }
                if let KeyGet::Header { name } = key {
                    HeaderName::from_bytes(name.as_bytes()).map_err(|_| "invalid variable source header name")?;
                }
            }
            let pointer = binding
                .body_pointer
                .as_deref()
                .map(|pointer| {
                    if binding.format != GraphqlVariableFormat::Text {
                        return Err("bodyPointer already supplies native JSON; format must be text".into());
                    }
                    compile_pointer(pointer, "bodyPointer")
                })
                .transpose()?;
            let ty =
                TypeRef::parse(definition.ty().ok_or("missing variable type")?).map_err(|_| "invalid variable type")?;
            if ty.nulls.len() > 8 {
                return Err("variable type exceeds eight wrapper levels".into());
            }
            bindings.push(CompiledBinding {
                source: binding.clone(),
                pointer,
                ty,
                has_default: definition.default_value().is_some(),
            });
        }
        for (name, definition) in &parsed.vars {
            let ty =
                TypeRef::parse(definition.ty().ok_or("missing variable type")?).map_err(|_| "invalid variable type")?;
            if ty.nulls.len() > 8 {
                return Err("variable type exceeds eight wrapper levels".into());
            }
            if ty.required() && definition.default_value().is_none() && !names.contains(name.as_str()) {
                return Err("required GraphQL variable has no mapping or default".into());
            }
        }
        Ok(CompiledRestToGraphql {
            query: self.query.clone(),
            operation_name: self.operation_name.clone(),
            path: self.path.clone(),
            operation_type: parsed.operation_type(),
            bindings,
        })
    }
}

impl PluginValidation for RestToGraphqlConfig {
    fn validation_error(&self) -> Option<String> {
        self.compile().err()
    }
}

impl CompiledRestToGraphql {
    pub fn bindings(&self) -> impl Iterator<Item = &GraphqlVariableBinding> {
        self.bindings.iter().map(|binding| &binding.source)
    }

    /// Body pointer lookup is borrowed; clone only after bounded profiling.
    pub fn body_value<'a>(&self, index: usize, body: &'a Value) -> Option<&'a Value> {
        self.bindings.get(index)?.pointer.as_ref()?.resolve(body).ok()
    }

    pub fn bind(&self, index: usize, value: Option<Cow<'_, Value>>) -> Result<Option<Value>, GraphqlError> {
        let binding = self.bindings.get(index).ok_or(GraphqlError::Invalid)?;
        let Some(value) = value else {
            return if binding.ty.required() && !binding.has_default {
                Err(GraphqlError::Invalid)
            } else {
                Ok(None)
            };
        };
        check_value_bounds(&value)?;
        let value = if binding.source.from.is_some() {
            let text = value.as_str().ok_or(GraphqlError::Invalid)?;
            if text.len() > REST_GRAPHQL_MAX_VALUE_BYTES {
                return Err(GraphqlError::TooLarge);
            }
            if binding.source.format == GraphqlVariableFormat::Json
                || binding.ty.nulls.len() > 1
                || matches!(binding.ty.name.as_str(), "Int" | "Float" | "Boolean")
            {
                let parsed = parse_json_instance_with_limits(text.as_bytes(), 32, 4096)
                    .map_err(|_| GraphqlError::Invalid)?
                    .into_value();
                // Conversion creates a new value; unchanged inputs already passed these bounds.
                check_value_bounds(&parsed)?;
                parsed
            } else {
                value.into_owned()
            }
        } else {
            value.into_owned()
        };
        check_type(&binding.ty, &value, 0)?;
        Ok(Some(value))
    }
}

fn check_type(ty: &TypeRef, value: &Value, depth: usize) -> Result<(), GraphqlError> {
    if value.is_null() {
        return if ty.nulls[depth] {
            Err(GraphqlError::Invalid)
        } else {
            Ok(())
        };
    }
    if depth + 1 < ty.nulls.len() {
        let values = value.as_array().ok_or(GraphqlError::Invalid)?;
        for value in values {
            check_type(ty, value, depth + 1)?;
        }
        return Ok(());
    }
    let valid = match ty.name.as_str() {
        "String" => value.is_string(),
        "ID" => value.is_string() || value.as_i64().is_some() || value.as_u64().is_some(),
        "Int" => value.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
        "Float" => value.as_f64().is_some_and(f64::is_finite),
        "Boolean" => value.is_boolean(),
        // SDL-specific enum/input/custom-scalar semantics belong to the upstream.
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(GraphqlError::Invalid)
    }
}

fn check_value_bounds(value: &Value) -> Result<(), GraphqlError> {
    let profile = profile_json_value(value).map_err(|_| GraphqlError::Invalid)?;
    if profile.node_count > 4096 || profile.max_depth > 32 || profile.text_bytes > REST_GRAPHQL_MAX_VALUE_BYTES as u64 {
        return Err(GraphqlError::TooLarge);
    }
    // Scalar roots cannot contain arrays; profiling above still enforces their bounds.
    if !matches!(value, Value::Array(_) | Value::Object(_)) {
        return Ok(());
    }
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Array(values) => {
                if values.len() > 32 {
                    return Err(GraphqlError::TooLarge);
                }
                stack.extend(values);
            }
            Value::Object(values) => stack.extend(values.values()),
            _ => {}
        }
    }
    Ok(())
}

/// Strict preflight retains duplicate information before canonical KeyGet lookup.
pub fn validate_rest_query(query: &str, limit: usize) -> Result<(), GraphqlError> {
    if query.len() > limit {
        return Err(GraphqlError::TooLarge);
    }
    let mut remaining = limit;
    let mut names = HashSet::new();
    for (index, pair) in query.split('&').filter(|pair| !pair.is_empty()).enumerate() {
        if index >= 256 {
            return Err(GraphqlError::TooLarge);
        }
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = decode(name, &mut remaining)?;
        if !names.insert(name) {
            return Err(GraphqlError::Invalid);
        }
        decode(value, &mut remaining)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/rest_to_graphql_test.rs"]
mod tests;
