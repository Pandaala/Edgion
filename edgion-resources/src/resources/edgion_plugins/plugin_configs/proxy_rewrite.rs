//! Proxy Rewrite plugin configuration
//!
//! Rewrites requests before forwarding to upstream services.
//! Supports URI, Host, Method, and Headers modification.

use jsonptr::PointerBuf;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::ctx_set::{template_has_secret_ref, CompiledTemplate, TemplateSegment, TemplateVarType};
use super::json_schema_validation::profile_json_value;

pub const MAX_JSON_PATCH_OPERATIONS: usize = 64;
pub const MAX_JSON_PATCH_PLAN_BYTES: usize = 64 * 1024;
pub const MAX_JSON_POINTER_BYTES: usize = 4_096;
pub const MAX_JSON_POINTER_TOKENS: usize = 64;

/// Proxy Rewrite plugin configuration
///
/// Rewrites requests before forwarding to upstream services.
/// All fields are optional - configure only what you need.
///
/// # Example
///
/// ```yaml
/// type: ProxyRewrite
/// config:
///   # URI rewrite (choose one: uri or regexUri)
///   uri: "/internal/api/v2"
///   # Or use regex:
///   # regexUri:
///   #   pattern: "^/api/v1/users/([0-9]+)"
///   #   replacement: "/user-service/$1"
///   
///   host: "backend.internal.svc"
///   
///   headers:
///     set:
///       - name: X-Api-Version
///         value: "v2"
///     remove:
///       - X-Debug
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyRewriteConfig {
    /// New upstream URI path.
    ///
    /// Supports variable substitution:
    /// - `$uri` - original request path
    /// - `$arg_<name>` - query parameter value
    ///
    /// Query string from original request is preserved automatically.
    /// When both `uri` and `regexUri` are configured, `uri` takes priority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,

    /// Regex-based URI rewrite.
    ///
    /// Uses regex to match the original path and generates a new path using the replacement template.
    /// The replacement supports `$1`, `$2`, etc. for capture group references.
    ///
    /// Query string from original request is preserved automatically.
    ///
    /// # Example
    ///
    /// ```yaml
    /// regexUri:
    ///   pattern: "^/api/v1/users/([0-9]+)/profile"
    ///   replacement: "/user-service/$1"
    /// ```
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regex_uri: Option<RegexUri>,

    /// Set Host request header.
    ///
    /// Note: Do not configure Host in `headers.set` - use this field instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,

    /// Rewrite HTTP method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<HttpMethod>,

    /// Request header operations.
    ///
    /// Execution order: add -> set -> remove.
    /// Header values support variable substitution: `$uri`, `$arg_<name>`, `$1-$9` (only when regexUri matches).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeaderActions>,

    /// Structural JSON request-body rewrites using RFC 6901 JSON pointers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_body: Option<JsonBodyRewrite>,

    // ========== Runtime fields (not serialized) ==========
    /// Precompiled regex pattern.
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<Regex>,

    /// Precompiled JSON Patch plan.
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_json_body: Option<CompiledJsonBodyRewrite>,
}

/// Regex URI rewrite configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegexUri {
    /// Regex pattern to match the URI.
    pub pattern: String,

    /// Replacement template, supports `$1`, `$2`, etc. for capture group references.
    pub replacement: String,
}

/// HTTP method enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
    Trace,
    Connect,
    Mkcol,
    Copy,
    Move,
    Propfind,
    Lock,
    Unlock,
}

/// A single header name-value pair.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeaderEntry {
    /// Header name
    pub name: String,
    /// Header value (supports variable substitution)
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JsonBodyRewrite {
    /// Ordered RFC 6902 operations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<JsonPatchOperation>,
}

/// One ordered RFC 6902 JSON Patch operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub enum JsonPatchOperation {
    Add {
        path: String,
        #[serde(default, skip_serializing_if = "JsonLiteralValue::is_missing")]
        value: JsonLiteralValue,
        #[serde(rename = "valueTemplate", default, skip_serializing_if = "Option::is_none")]
        value_template: Option<String>,
    },
    Remove {
        path: String,
    },
    Replace {
        path: String,
        #[serde(default, skip_serializing_if = "JsonLiteralValue::is_missing")]
        value: JsonLiteralValue,
        #[serde(rename = "valueTemplate", default, skip_serializing_if = "Option::is_none")]
        value_template: Option<String>,
    },
    Move {
        from: String,
        path: String,
    },
    Copy {
        from: String,
        path: String,
    },
    Test {
        path: String,
        #[serde(default, skip_serializing_if = "JsonLiteralValue::is_missing")]
        value: JsonLiteralValue,
        #[serde(rename = "valueTemplate", default, skip_serializing_if = "Option::is_none")]
        value_template: Option<String>,
    },
}

/// Presence-aware JSON literal.
///
/// `Option<Value>` alone cannot distinguish an omitted field from an explicit
/// JSON `null`. This wrapper records field presence while keeping the wire
/// representation equal to the literal itself.
#[derive(Debug, Clone, Default, JsonSchema)]
#[schemars(transparent)]
pub struct JsonLiteralValue(#[schemars(with = "serde_json::Value")] Option<serde_json::Value>);

impl JsonLiteralValue {
    pub fn new(value: serde_json::Value) -> Self {
        Self(Some(value))
    }

    pub fn is_missing(&self) -> bool {
        self.0.is_none()
    }

    pub fn as_ref(&self) -> Option<&serde_json::Value> {
        self.0.as_ref()
    }
}

impl Serialize for JsonLiteralValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for JsonLiteralValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_json::Value::deserialize(deserializer).map(Self::new)
    }
}

/// Immutable JSON Patch plan published with the plugin runtime.
#[derive(Debug, Clone)]
pub struct CompiledJsonBodyRewrite {
    pub operations: Vec<CompiledJsonPatchOperation>,
}

#[derive(Debug, Clone)]
pub enum CompiledJsonPatchOperation {
    Add {
        path: PointerBuf,
        value: CompiledJsonPatchValue,
    },
    Remove {
        path: PointerBuf,
    },
    Replace {
        path: PointerBuf,
        value: CompiledJsonPatchValue,
    },
    Move {
        from: PointerBuf,
        path: PointerBuf,
    },
    Copy {
        from: PointerBuf,
        path: PointerBuf,
    },
    Test {
        path: PointerBuf,
        value: CompiledJsonPatchValue,
    },
}

#[derive(Debug, Clone)]
pub enum CompiledJsonPatchValue {
    Literal(serde_json::Value),
    Template(CompiledTemplate),
}

impl JsonBodyRewrite {
    pub fn compile(&self) -> Result<CompiledJsonBodyRewrite, String> {
        if self.operations.is_empty() {
            return Err("jsonBody.operations must not be empty".to_string());
        }
        if self.operations.len() > MAX_JSON_PATCH_OPERATIONS {
            return Err(format!(
                "jsonBody.operations must contain at most {MAX_JSON_PATCH_OPERATIONS} operations"
            ));
        }
        if serde_json::to_vec(&self.operations)
            .map_err(|_| "jsonBody.operations could not be serialized".to_string())?
            .len()
            > MAX_JSON_PATCH_PLAN_BYTES
        {
            return Err(format!(
                "jsonBody.operations must not exceed {MAX_JSON_PATCH_PLAN_BYTES} serialized bytes"
            ));
        }

        let mut operations = Vec::with_capacity(self.operations.len());
        for (index, operation) in self.operations.iter().enumerate() {
            let field = |name: &str| format!("jsonBody.operations[{index}].{name}");
            let compiled = match operation {
                JsonPatchOperation::Add {
                    path,
                    value,
                    value_template,
                } => CompiledJsonPatchOperation::Add {
                    path: compile_pointer(path, &field("path"))?,
                    value: compile_value(value, value_template.as_deref(), &field("value"))?,
                },
                JsonPatchOperation::Remove { path } => CompiledJsonPatchOperation::Remove {
                    path: compile_pointer(path, &field("path"))?,
                },
                JsonPatchOperation::Replace {
                    path,
                    value,
                    value_template,
                } => CompiledJsonPatchOperation::Replace {
                    path: compile_pointer(path, &field("path"))?,
                    value: compile_value(value, value_template.as_deref(), &field("value"))?,
                },
                JsonPatchOperation::Move { from, path } => CompiledJsonPatchOperation::Move {
                    from: compile_pointer(from, &field("from"))?,
                    path: compile_pointer(path, &field("path"))?,
                },
                JsonPatchOperation::Copy { from, path } => CompiledJsonPatchOperation::Copy {
                    from: compile_pointer(from, &field("from"))?,
                    path: compile_pointer(path, &field("path"))?,
                },
                JsonPatchOperation::Test {
                    path,
                    value,
                    value_template,
                } => CompiledJsonPatchOperation::Test {
                    path: compile_pointer(path, &field("path"))?,
                    value: compile_value(value, value_template.as_deref(), &field("value"))?,
                },
            };
            operations.push(compiled);
        }
        Ok(CompiledJsonBodyRewrite { operations })
    }
}

fn compile_pointer(pointer: &str, field: &str) -> Result<PointerBuf, String> {
    if pointer.len() > MAX_JSON_POINTER_BYTES {
        return Err(format!(
            "{field} must not exceed {MAX_JSON_POINTER_BYTES} encoded bytes"
        ));
    }
    let pointer = PointerBuf::parse(pointer.to_string()).map_err(|_| format!("{field} is not a valid JSON Pointer"))?;
    if pointer.count() > MAX_JSON_POINTER_TOKENS {
        return Err(format!(
            "{field} must not contain more than {MAX_JSON_POINTER_TOKENS} tokens"
        ));
    }
    Ok(pointer)
}

fn compile_value(
    literal: &JsonLiteralValue,
    template: Option<&str>,
    field: &str,
) -> Result<CompiledJsonPatchValue, String> {
    match (literal.as_ref(), template) {
        (Some(value), None) => {
            profile_json_value(value).map_err(|error| format!("{field}: {error}"))?;
            Ok(CompiledJsonPatchValue::Literal(value.clone()))
        }
        (None, Some(template)) => {
            let compiled = CompiledTemplate::compile(template).map_err(|error| format!("{field}Template: {error}"))?;
            if template_has_secret_ref(&compiled) {
                return Err(format!("{field}Template must not reference ${{secretRef:...}}"));
            }
            validate_template_variable_shapes(&compiled).map_err(|error| format!("{field}Template: {error}"))?;
            Ok(CompiledJsonPatchValue::Template(compiled))
        }
        (Some(_), Some(_)) => Err(format!(
            "{field} and {field}Template are mutually exclusive; configure exactly one"
        )),
        (None, None) => Err(format!("{field} or {field}Template is required; configure exactly one")),
    }
}

fn validate_template_variable_shapes(template: &CompiledTemplate) -> Result<(), String> {
    for segment in template.segments() {
        let TemplateSegment::Variable { var_type, var_name } = segment else {
            continue;
        };
        match var_type {
            TemplateVarType::Header
            | TemplateVarType::Query
            | TemplateVarType::Cookie
            | TemplateVarType::Ctx
            | TemplateVarType::MatchedHttpRoute
            | TemplateVarType::MatchedGrpcRoute => {
                if var_name.as_deref().is_none_or(str::is_empty) {
                    return Err("named variable must include a non-empty selector".to_string());
                }
            }
            TemplateVarType::Path
            | TemplateVarType::Uri
            | TemplateVarType::RequestPath
            | TemplateVarType::RequestUri
            | TemplateVarType::Method
            | TemplateVarType::ClientIp => {
                if var_name.is_some() {
                    return Err("scalar variable must not include a selector".to_string());
                }
            }
            TemplateVarType::SecretRef => {
                return Err("secretRef variables are not allowed".to_string());
            }
        }
    }
    Ok(())
}

/// Request header operations configuration.
///
/// Uses Vec instead of HashMap to preserve insertion order.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeaderActions {
    /// Add request headers (append to existing values with comma separation).
    /// Headers are processed in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<HeaderEntry>>,

    /// Set request headers (overwrite existing values).
    /// Headers are processed in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<Vec<HeaderEntry>>,

    /// Remove request headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<String>>,
}

impl ProxyRewriteConfig {
    /// Check all validation rules without mutating self.
    ///
    /// Returns `Some(error)` if any field is invalid, `None` if all checks pass.
    /// Used by both `precompile()` and the `PluginValidation` trait impl.
    pub fn check_fields(&self) -> Option<String> {
        // Check for Host conflict in headers.set
        if self.host.is_some() {
            if let Some(ref headers) = self.headers {
                if let Some(ref set_headers) = headers.set {
                    for entry in set_headers {
                        if entry.name.eq_ignore_ascii_case("host") {
                            return Some(
                                "Conflict: 'host' field and 'headers.set' with Host header both configured. Use only 'host' field.".to_string()
                            );
                        }
                    }
                }
            }
        }

        // Validate regex pattern if configured
        if let Some(ref regex_uri) = self.regex_uri {
            if let Err(e) = Regex::new(&regex_uri.pattern) {
                return Some(format!("Invalid regex pattern: {}", e));
            }
        }

        if let Some(rewrite) = &self.json_body {
            if let Err(error) = rewrite.compile() {
                return Some(error);
            }
        }

        None
    }

    /// Precompile regex pattern and validate configuration.
    ///
    /// Returns true if configuration is valid, false otherwise.
    pub fn precompile(&mut self) -> bool {
        self.compiled_regex = None;
        self.compiled_json_body = None;
        let compiled_regex = match self.regex_uri.as_ref() {
            Some(regex_uri) => match Regex::new(&regex_uri.pattern) {
                Ok(regex) => Some(regex),
                Err(_) => return false,
            },
            None => None,
        };
        let compiled_json_body = match self.json_body.as_ref() {
            Some(rewrite) => match rewrite.compile() {
                Ok(rewrite) => Some(rewrite),
                Err(_) => return false,
            },
            None => None,
        };
        if self.check_fields().is_some() {
            return false;
        }
        self.compiled_regex = compiled_regex;
        self.compiled_json_body = compiled_json_body;
        true
    }

    /// Check if any rewrite is configured (for logging purposes)
    pub fn has_any_config(&self) -> bool {
        self.uri.is_some()
            || self.regex_uri.is_some()
            || self.host.is_some()
            || self.method.is_some()
            || self.headers.is_some()
            || self.json_body.is_some()
    }
}

impl HttpMethod {
    /// Convert to HTTP method string.
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Trace => "TRACE",
            HttpMethod::Connect => "CONNECT",
            HttpMethod::Mkcol => "MKCOL",
            HttpMethod::Copy => "COPY",
            HttpMethod::Move => "MOVE",
            HttpMethod::Propfind => "PROPFIND",
            HttpMethod::Lock => "LOCK",
            HttpMethod::Unlock => "UNLOCK",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_rewrite_config_default() {
        let config = ProxyRewriteConfig::default();
        assert!(config.uri.is_none());
        assert!(config.regex_uri.is_none());
        assert!(config.host.is_none());
        assert!(config.method.is_none());
        assert!(config.headers.is_none());
        assert!(config.check_fields().is_none());
        assert!(!config.has_any_config());
    }

    #[test]
    fn test_proxy_rewrite_config_precompile_valid() {
        let mut config = ProxyRewriteConfig {
            regex_uri: Some(RegexUri {
                pattern: r"^/api/v1/users/(\d+)".to_string(),
                replacement: "/users/$1".to_string(),
            }),
            ..Default::default()
        };

        assert!(config.precompile());
        assert!(config.compiled_regex.is_some());
    }

    #[test]
    fn test_proxy_rewrite_config_precompile_invalid() {
        let mut config = ProxyRewriteConfig {
            regex_uri: Some(RegexUri {
                pattern: r"[invalid".to_string(), // Invalid regex
                replacement: "/users/$1".to_string(),
            }),
            ..Default::default()
        };

        assert!(!config.precompile());
        assert!(config.check_fields().is_some());
    }

    #[test]
    fn test_host_conflict_detection() {
        let mut config = ProxyRewriteConfig {
            host: Some("backend.svc".to_string()),
            headers: Some(HeaderActions {
                add: None,
                set: Some(vec![HeaderEntry {
                    name: "Host".to_string(),
                    value: "other.svc".to_string(),
                }]),
                remove: None,
            }),
            ..Default::default()
        };

        assert!(!config.precompile());
        assert!(config.check_fields().unwrap().contains("Conflict"));
    }

    #[test]
    fn test_http_method_as_str() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
        assert_eq!(HttpMethod::Put.as_str(), "PUT");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
    }

    #[test]
    fn test_proxy_rewrite_config_serde() {
        let json = r#"{
            "uri": "/new/path",
            "host": "backend.svc",
            "method": "POST",
            "headers": {
                "set": [
                    {"name": "X-Api-Version", "value": "v2"}
                ],
                "remove": ["X-Debug"]
            }
        }"#;

        let config: ProxyRewriteConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.uri, Some("/new/path".to_string()));
        assert_eq!(config.host, Some("backend.svc".to_string()));
        assert!(matches!(config.method, Some(HttpMethod::Post)));

        let headers = config.headers.as_ref().unwrap();
        let set_headers = headers.set.as_ref().unwrap();
        assert_eq!(set_headers.len(), 1);
        assert_eq!(set_headers[0].name, "X-Api-Version");
        assert_eq!(set_headers[0].value, "v2");
        assert!(headers.remove.as_ref().unwrap().contains(&"X-Debug".to_string()));
    }

    #[test]
    fn test_regex_uri_serde() {
        let json = r#"{
            "regexUri": {
                "pattern": "^/api/v1/(.*)",
                "replacement": "/internal/$1"
            }
        }"#;

        let config: ProxyRewriteConfig = serde_json::from_str(json).unwrap();
        let regex_uri = config.regex_uri.as_ref().unwrap();
        assert_eq!(regex_uri.pattern, "^/api/v1/(.*)");
        assert_eq!(regex_uri.replacement, "/internal/$1");
    }

    #[test]
    fn test_header_order_preserved() {
        let json = r#"{
            "headers": {
                "set": [
                    {"name": "X-First", "value": "1"},
                    {"name": "X-Second", "value": "2"},
                    {"name": "X-Third", "value": "3"}
                ]
            }
        }"#;

        let config: ProxyRewriteConfig = serde_json::from_str(json).unwrap();
        let set_headers = config.headers.as_ref().unwrap().set.as_ref().unwrap();
        assert_eq!(set_headers[0].name, "X-First");
        assert_eq!(set_headers[1].name, "X-Second");
        assert_eq!(set_headers[2].name, "X-Third");
    }

    #[test]
    fn json_patch_all_operations_deserialize_in_order() {
        let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "add", "path": "/items/-", "value": {"enabled": true}},
                    {"op": "remove", "path": "/secret"},
                    {"op": "replace", "path": "/name", "valueTemplate": "${header:X-Name}"},
                    {"op": "move", "from": "/old", "path": "/new"},
                    {"op": "copy", "from": "/source", "path": "/copy"},
                    {"op": "test", "path": "/version", "value": 1}
                ]
            }
        }))
        .unwrap();

        let operations = &config.json_body.as_ref().unwrap().operations;
        assert_eq!(operations.len(), 6);
        assert!(matches!(operations[0], JsonPatchOperation::Add { .. }));
        assert!(matches!(operations[1], JsonPatchOperation::Remove { .. }));
        assert!(matches!(operations[2], JsonPatchOperation::Replace { .. }));
        assert!(matches!(operations[3], JsonPatchOperation::Move { .. }));
        assert!(matches!(operations[4], JsonPatchOperation::Copy { .. }));
        assert!(matches!(operations[5], JsonPatchOperation::Test { .. }));
        assert!(config.check_fields().is_none());
    }

    #[test]
    fn explicit_null_literal_is_present_and_round_trips() {
        let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "add", "path": "/nullable", "value": null}
                ]
            }
        }))
        .unwrap();
        let operation = &config.json_body.as_ref().unwrap().operations[0];
        match operation {
            JsonPatchOperation::Add {
                value, value_template, ..
            } => {
                assert_eq!(value.as_ref(), Some(&serde_json::Value::Null));
                assert!(value_template.is_none());
            }
            _ => panic!("expected add operation"),
        }

        let encoded = serde_json::to_value(&config).unwrap();
        assert_eq!(
            encoded.pointer("/jsonBody/operations/0/value"),
            Some(&serde_json::Value::Null)
        );
    }

    #[test]
    fn literal_and_template_must_be_exactly_one() {
        let both: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {
                        "op": "replace",
                        "path": "/name",
                        "value": "literal",
                        "valueTemplate": "${ctx:name}"
                    }
                ]
            }
        }))
        .unwrap();
        assert!(both.check_fields().unwrap().contains("mutually exclusive"));

        let neither: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "test", "path": "/name"}
                ]
            }
        }))
        .unwrap();
        assert!(neither.check_fields().unwrap().contains("configure exactly one"));
    }

    #[test]
    fn templates_compile_once_and_secret_refs_are_rejected() {
        let mut config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "add", "path": "/tenant", "valueTemplate": "${header:X-Tenant}"}
                ]
            }
        }))
        .unwrap();
        assert!(config.precompile());
        assert_eq!(config.compiled_json_body.as_ref().unwrap().operations.len(), 1);

        let secret: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "add", "path": "/token", "valueTemplate": "${secretRef:ns/name:key}"}
                ]
            }
        }))
        .unwrap();
        assert!(secret.check_fields().unwrap().contains("must not reference"));

        for template in ["${header}", "${query:}", "${path:ignored}", "${method:ignored}"] {
            let invalid: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
                "jsonBody": {
                    "operations": [
                        {"op": "add", "path": "/value", "valueTemplate": template}
                    ]
                }
            }))
            .unwrap();
            assert!(invalid.check_fields().is_some(), "{template}");
        }
    }

    #[test]
    fn json_pointer_syntax_size_and_token_limits_are_validated() {
        let invalid: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "remove", "path": "/bad~2escape"}
                ]
            }
        }))
        .unwrap();
        assert!(invalid.check_fields().unwrap().contains("valid JSON Pointer"));

        let too_long_path = format!("/{}", "x".repeat(MAX_JSON_POINTER_BYTES));
        let too_long = ProxyRewriteConfig {
            json_body: Some(JsonBodyRewrite {
                operations: vec![JsonPatchOperation::Remove { path: too_long_path }],
            }),
            ..Default::default()
        };
        assert!(too_long.check_fields().unwrap().contains("encoded bytes"));

        let too_many_tokens = format!("/{}", vec!["x"; MAX_JSON_POINTER_TOKENS + 1].join("/"));
        let too_deep = ProxyRewriteConfig {
            json_body: Some(JsonBodyRewrite {
                operations: vec![JsonPatchOperation::Remove { path: too_many_tokens }],
            }),
            ..Default::default()
        };
        assert!(too_deep.check_fields().unwrap().contains("tokens"));

        let root: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "replace", "path": "", "value": {"ok": true}}
                ]
            }
        }))
        .unwrap();
        assert!(root.check_fields().is_none());
    }

    #[test]
    fn operation_count_and_static_literal_complexity_are_bounded() {
        let too_many = ProxyRewriteConfig {
            json_body: Some(JsonBodyRewrite {
                operations: (0..=MAX_JSON_PATCH_OPERATIONS)
                    .map(|index| JsonPatchOperation::Remove {
                        path: format!("/{index}"),
                    })
                    .collect(),
            }),
            ..Default::default()
        };
        assert!(too_many.check_fields().unwrap().contains("at most"));

        let mut deeply_nested = serde_json::Value::Null;
        for _ in 0..65 {
            deeply_nested = serde_json::json!([deeply_nested]);
        }
        let too_deep = ProxyRewriteConfig {
            json_body: Some(JsonBodyRewrite {
                operations: vec![JsonPatchOperation::Add {
                    path: "/value".to_string(),
                    value: JsonLiteralValue::new(deeply_nested),
                    value_template: None,
                }],
            }),
            ..Default::default()
        };
        assert!(too_deep.check_fields().unwrap().contains("nesting-depth"));

        let oversized_plan = ProxyRewriteConfig {
            json_body: Some(JsonBodyRewrite {
                operations: vec![JsonPatchOperation::Add {
                    path: "/value".to_string(),
                    value: JsonLiteralValue::new(serde_json::Value::String("x".repeat(MAX_JSON_PATCH_PLAN_BYTES))),
                    value_template: None,
                }],
            }),
            ..Default::default()
        };
        assert!(oversized_plan.check_fields().unwrap().contains("serialized bytes"));
    }

    #[test]
    fn compiled_runtime_fields_are_not_serialized() {
        let mut config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "regexUri": {"pattern": "^/old$", "replacement": "/new"},
            "jsonBody": {
                "operations": [
                    {"op": "copy", "from": "/source", "path": "/copy"}
                ]
            }
        }))
        .unwrap();
        assert!(config.precompile());
        let encoded = serde_json::to_value(config).unwrap();
        assert!(encoded.get("compiledRegex").is_none());
        assert!(encoded.get("compiledJsonBody").is_none());
    }

    #[test]
    fn unknown_operation_fields_are_rejected() {
        let result = serde_json::from_value::<ProxyRewriteConfig>(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "remove", "path": "/x", "value": 1}
                ]
            }
        }));
        assert!(result.is_err());
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for ProxyRewriteConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod validation_trait_tests {
    use super::*;
    use crate::resources::edgion_plugins::PluginValidation;

    /// Verify that `PluginValidation::validation_error()` works WITHOUT a prior `precompile()` call.
    /// The trait impl calls `check_fields()` directly, so no mutable state is required.
    #[test]
    fn validation_error_returns_field_error_without_validate_call() {
        // Construct an invalid config (host conflict) without calling precompile().
        let config = ProxyRewriteConfig {
            host: Some("backend.svc".to_string()),
            headers: Some(HeaderActions {
                add: None,
                set: Some(vec![HeaderEntry {
                    name: "Host".to_string(),
                    value: "other.svc".to_string(),
                }]),
                remove: None,
            }),
            ..Default::default()
        };

        let result = PluginValidation::validation_error(&config);
        assert_eq!(
            result,
            Some(
                "Conflict: 'host' field and 'headers.set' with Host header both configured. Use only 'host' field."
                    .to_string()
            )
        );
    }

    #[test]
    fn validation_error_returns_none_for_valid_config() {
        let config = ProxyRewriteConfig {
            uri: Some("/new/path".to_string()),
            ..Default::default()
        };

        assert!(PluginValidation::validation_error(&config).is_none());
    }

    #[test]
    fn validation_error_returns_error_for_invalid_regex_without_validate_call() {
        let config = ProxyRewriteConfig {
            regex_uri: Some(RegexUri {
                pattern: r"[invalid".to_string(),
                replacement: "/users/$1".to_string(),
            }),
            ..Default::default()
        };

        let result = PluginValidation::validation_error(&config);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Invalid regex pattern"));
    }
}
