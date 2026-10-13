use super::{rpc_path, TranscodingError};
use prost_reflect::{DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, MethodDescriptor, Value};
use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value as Json};
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Debug)]
pub struct CompiledBinding {
    pub(crate) http_method: String,
    pub(crate) regex: Regex,
    pub method: MethodDescriptor,
    pub rpc_path: String,
    path_fields: Vec<Vec<FieldDescriptor>>,
    path_multi_segment: Vec<bool>,
    body: BodyBinding,
}
#[derive(Debug)]
enum BodyBinding {
    None,
    All,
    Field(Vec<FieldDescriptor>),
}
pub struct BoundRequest {
    pub binding: Arc<CompiledBinding>,
    fields: Vec<(Vec<FieldDescriptor>, Json)>,
}
impl CompiledBinding {
    pub(crate) fn compile(method: MethodDescriptor, verb: &str, template: &str, body: &str) -> Result<Self, String> {
        if template.len() > 4096 || !template.starts_with('/') {
            return Err("invalid HTTP path template".into());
        }
        if let Some(index) = template.find("**") {
            let rest = &template[index + 2..];
            let rest = rest.strip_prefix('}').unwrap_or(rest);
            if !rest.is_empty() && !rest.starts_with(':') || rest.contains('/') || rest.contains(['{', '}', '*']) {
                return Err("multi-segment wildcard must be last before the custom verb".into());
            }
        }
        let input = method.input();
        let mut regex = String::from("^");
        let mut fields: Vec<Vec<FieldDescriptor>> = Vec::new();
        let mut path_multi_segment = Vec::new();
        let mut pos = 0;
        while pos < template.len() {
            let rest = &template[pos..];
            if rest.starts_with('{') {
                let end = rest.find('}').ok_or("unclosed HTTP variable")?;
                let variable = &rest[1..end];
                let (name, pattern) = variable.split_once('=').unwrap_or((variable, "*"));
                let selector = resolve_field(&input, name).map_err(|_| "invalid path field selector")?;
                if selector
                    .last()
                    .is_none_or(|f| f.is_list() || f.is_map() || matches!(f.kind(), Kind::Message(_)))
                {
                    return Err("path variables require scalar fields".into());
                }
                if fields.iter().any(|f| overlaps(f, &selector)) {
                    return Err("overlapping path fields".into());
                }
                if pos == 0
                    || template.as_bytes()[pos - 1] != b'/'
                    || !matches!(rest.as_bytes().get(end + 1), None | Some(b'/') | Some(b':'))
                {
                    return Err("variables must occupy complete path segments".into());
                }
                path_multi_segment.push(pattern.contains('/') || pattern == "**");
                fields.push(selector);
                let optional_separator = pattern == "**" && regex.ends_with('/');
                if optional_separator {
                    regex.pop();
                    regex.push_str("(?:/");
                }
                regex.push('(');
                regex.push_str(&path_pattern(pattern)?);
                regex.push(')');
                if optional_separator {
                    regex.push_str(")?");
                }
                pos += end + 1;
            } else {
                let end = rest.find('{').unwrap_or(rest.len());
                let literal = &rest[..end];
                if let Some((path, suffix)) = literal.split_once(':') {
                    if suffix.is_empty() || suffix.contains(['/', ':', '*', '{', '}']) {
                        return Err("invalid custom verb".into());
                    }
                    regex.push_str(&path_pattern(path)?);
                    regex.push_str(&regex::escape(&format!(":{suffix}")));
                } else {
                    regex.push_str(&path_pattern(literal)?);
                }
                pos += end;
            }
        }
        if fields.len() > 64 {
            return Err("too many path bindings".into());
        }
        regex.push('$');
        let regex = RegexBuilder::new(&regex)
            .size_limit(256 * 1024)
            .dfa_size_limit(256 * 1024)
            .build()
            .map_err(|_| "HTTP path regex exceeds limits")?;
        let body = match body {
            "" => BodyBinding::None,
            "*" => BodyBinding::All,
            field => {
                let selector = resolve_field(&input, field).map_err(|_| "invalid body field selector")?;
                if fields.iter().any(|f| overlaps(f, &selector)) {
                    return Err("body overlaps path variable".into());
                }
                BodyBinding::Field(selector)
            }
        };
        Ok(Self {
            http_method: verb.to_owned(),
            regex,
            rpc_path: rpc_path(&method),
            method,
            path_fields: fields,
            path_multi_segment,
            body,
        })
    }
    pub(super) fn bind(
        self: &Arc<Self>,
        path: &str,
        query: Option<&str>,
        ignore_unknown: bool,
    ) -> Result<BoundRequest, TranscodingError> {
        let captures = self.regex.captures(path).ok_or(TranscodingError::Invalid)?;
        let mut fields = Vec::with_capacity(self.path_fields.len());
        for (index, selector) in self.path_fields.iter().enumerate() {
            let value = decode_path(
                captures.get(index + 1).map_or("", |c| c.as_str()),
                self.path_multi_segment[index],
            )?;
            fields.push((
                selector.clone(),
                scalar(selector.last().ok_or(TranscodingError::Invalid)?, &value)?,
            ));
        }
        if let Some(query) = query {
            decode_path(query, false)?;
        }
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
            if fields.len() >= 64 {
                return Err(TranscodingError::Invalid);
            }
            let selector = match resolve_field(&self.method.input(), &key) {
                Ok(f) => f,
                Err(_) if ignore_unknown => continue,
                Err(e) => return Err(e),
            };
            if matches!(self.body, BodyBinding::All)
                || self.path_fields.iter().any(|f| overlaps(f, &selector))
                || matches!(&self.body, BodyBinding::Field(f) if overlaps(f, &selector))
            {
                return Err(TranscodingError::Invalid);
            }
            let field = selector.last().ok_or(TranscodingError::Invalid)?;
            let val = scalar(field, &value)?;
            if field.is_list() {
                if let Some((_, Json::Array(values))) = fields.iter_mut().find(|(f, _)| f == &selector) {
                    values.push(val);
                } else {
                    fields.push((selector, Json::Array(vec![val])));
                }
            } else {
                if fields.iter().any(|(f, _)| overlaps(f, &selector)) {
                    return Err(TranscodingError::Invalid);
                }
                fields.push((selector, val));
            }
        }
        Ok(BoundRequest {
            binding: self.clone(),
            fields,
        })
    }
}
impl BoundRequest {
    pub(super) fn merge(self, body: Option<Json>) -> Result<(Arc<CompiledBinding>, Json), TranscodingError> {
        let mut root = match &self.binding.body {
            BodyBinding::None => {
                if body.is_some() {
                    return Err(TranscodingError::Invalid);
                }
                Json::Object(Map::new())
            }
            BodyBinding::All => body.unwrap_or_else(|| Json::Object(Map::new())),
            BodyBinding::Field(field) => {
                let mut root = Json::Object(Map::new());
                insert(&mut root, field, body.ok_or(TranscodingError::Invalid)?)?;
                root
            }
        };
        for (field, value) in self.fields {
            insert(&mut root, &field, value)?;
        }
        Ok((self.binding, root))
    }
}
fn insert(root: &mut Json, fields: &[FieldDescriptor], value: Json) -> Result<(), TranscodingError> {
    let (field, rest) = fields.split_first().ok_or(TranscodingError::Invalid)?;
    let object = root.as_object_mut().ok_or(TranscodingError::Invalid)?;
    // Accept either ProtoJSON spelling, but never two spellings of one field.
    if field.name() != field.json_name() && object.contains_key(field.name()) && object.contains_key(field.json_name())
    {
        return Err(TranscodingError::Invalid);
    }
    let key = if object.contains_key(field.name()) {
        field.name()
    } else {
        field.json_name()
    };
    if rest.is_empty() {
        if object.contains_key(key) {
            return Err(TranscodingError::Invalid);
        }
        object.insert(field.json_name().to_owned(), value);
        Ok(())
    } else {
        insert(
            object.entry(key).or_insert_with(|| Json::Object(Map::new())),
            rest,
            value,
        )
    }
}
fn resolve_field(input: &MessageDescriptor, name: &str) -> Result<Vec<FieldDescriptor>, TranscodingError> {
    if name.len() > 4096 {
        return Err(TranscodingError::Invalid);
    }
    let mut message = input.clone();
    let mut result = Vec::new();
    let mut parts = name.split('.').peekable();
    while let Some(part) = parts.next() {
        if result.len() >= 32 {
            return Err(TranscodingError::Invalid);
        }
        let f = message
            .get_field_by_name(part)
            .or_else(|| message.get_field_by_json_name(part))
            .ok_or(TranscodingError::Invalid)?;
        if parts.peek().is_some() {
            if f.is_list() || f.is_map() {
                return Err(TranscodingError::Invalid);
            }
            let Kind::Message(next) = f.kind() else {
                return Err(TranscodingError::Invalid);
            };
            message = next;
        }
        result.push(f);
    }
    if result.is_empty() {
        return Err(TranscodingError::Invalid);
    }
    Ok(result)
}
fn overlaps(a: &[FieldDescriptor], b: &[FieldDescriptor]) -> bool {
    a.iter().zip(b).all(|(x, y)| x == y)
}
fn scalar(field: &FieldDescriptor, value: &str) -> Result<Json, TranscodingError> {
    Ok(match field.kind() {
        Kind::Message(_) => return Err(TranscodingError::Invalid),
        Kind::Bool => match value {
            "true" => Json::Bool(true),
            "false" => Json::Bool(false),
            _ => return Err(TranscodingError::Invalid),
        },
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
            Json::from(value.parse::<i32>().map_err(|_| TranscodingError::Invalid)?)
        }
        Kind::Uint32 | Kind::Fixed32 => Json::from(value.parse::<u32>().map_err(|_| TranscodingError::Invalid)?),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
            value.parse::<i64>().map_err(|_| TranscodingError::Invalid)?;
            Json::String(value.into())
        }
        Kind::Uint64 | Kind::Fixed64 => {
            value.parse::<u64>().map_err(|_| TranscodingError::Invalid)?;
            Json::String(value.into())
        }
        Kind::Float | Kind::Double => {
            let n = value.parse::<f64>().map_err(|_| TranscodingError::Invalid)?;
            if !n.is_finite() {
                return Err(TranscodingError::Invalid);
            }
            Json::from(n)
        }
        Kind::Enum(_) | Kind::String | Kind::Bytes => Json::String(value.into()),
    })
}
fn path_pattern(pattern: &str) -> Result<String, String> {
    let mut result = String::new();
    for (i, segment) in pattern.split('/').enumerate() {
        if segment == "**" && i != 0 {
            result.push_str("(?:/.*)?");
            continue;
        }
        if i != 0 {
            result.push('/');
        }
        result.push_str(&match segment {
            "*" => "[^/]+".into(),
            "**" => ".*".into(),
            literal if !literal.contains(['*', '{', '}']) => regex::escape(literal),
            _ => return Err("invalid path wildcard".into()),
        });
    }
    Ok(result)
}
fn decode_path(value: &str, preserve_slash: bool) -> Result<Cow<'_, str>, TranscodingError> {
    if !value.as_bytes().contains(&b'%') {
        return Ok(Cow::Borrowed(value));
    }
    let mut output = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = bytes.get(i + 1..i + 3).ok_or(TranscodingError::Invalid)?;
            let h = |c: u8| {
                (c as char)
                    .to_digit(16)
                    .map(|v| v as u8)
                    .ok_or(TranscodingError::Invalid)
            };
            let decoded = h(pair[0])? * 16 + h(pair[1])?;
            if preserve_slash && decoded == b'/' {
                output.extend_from_slice(&bytes[i..i + 3]);
            } else {
                output.push(decoded);
            }
            i += 3;
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(output)
        .map(Cow::Owned)
        .map_err(|_| TranscodingError::Invalid)
}
pub(super) fn compile_rules(
    method: &MethodDescriptor,
    rule: &DynamicMessage,
    output: &mut Vec<Arc<CompiledBinding>>,
    depth: usize,
) -> Result<(), String> {
    if depth > 1 || output.len() >= 256 {
        return Err("nested or excessive additional bindings".into());
    }
    let text = |name: &str| {
        rule.get_field_by_name(name)
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    if !text("response_body").is_empty() {
        return Err("response_body is unsupported".into());
    }
    let mut pattern = None;
    for (name, verb) in [
        ("get", "GET"),
        ("put", "PUT"),
        ("post", "POST"),
        ("delete", "DELETE"),
        ("patch", "PATCH"),
    ] {
        let path = text(name);
        if !path.is_empty() {
            if pattern.is_some() {
                return Err("multiple HTTP patterns".into());
            }
            pattern = Some((verb, path));
        }
    }
    if rule
        .get_field_by_name("custom")
        .is_some_and(|v| v.as_message().is_some_and(|m| m.fields().next().is_some()))
    {
        return Err("custom HTTP methods are unsupported".into());
    }
    let (verb, path) = pattern.ok_or("HTTP annotation has no supported pattern")?;
    output.push(Arc::new(CompiledBinding::compile(
        method.clone(),
        verb,
        &path,
        &text("body"),
    )?));
    if let Some(bindings) = rule.get_field_by_name("additional_bindings") {
        if let Value::List(bindings) = bindings.as_ref() {
            for binding in bindings {
                let Value::Message(binding) = binding else {
                    return Err("invalid additional binding".into());
                };
                compile_rules(method, binding, output, depth + 1)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/grpc_transcoding/mapping_test.rs"]
mod tests;
