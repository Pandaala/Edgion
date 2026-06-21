//! CtxSet plugin configuration
//!
//! Sets context variables from various sources with optional extraction,
//! transformation, and value mapping.
//!
//! ## Features:
//! - Extract values from headers, cookies, query params, path, etc.
//! - Use templates with variable interpolation
//! - Regex extraction with capture groups
//! - Value transformation (replace, case, substring, prefix, suffix, trim)
//! - Value mapping with default fallback
//!
//! ## Configuration Examples:
//!
//! ### Basic: copy header to context
//! ```yaml
//! ctxSet:
//!   vars:
//!     - name: "user_id"
//!       from:
//!         type: header
//!         name: "X-User-Id"
//! ```
//!
//! ### Regex extraction
//! ```yaml
//! ctxSet:
//!   vars:
//!     - name: "jwt_user"
//!       from:
//!         type: header
//!         name: "Authorization"
//!       extract:
//!         regex: "Bearer .*\\.(.+)\\."
//!         group: 1
//! ```
//!
//! ### Value transformation
//! ```yaml
//! ctxSet:
//!   vars:
//!     - name: "method_lower"
//!       from:
//!         type: method
//!       transform:
//!         case: "lower"
//! ```
//!
//! ### Value mapping
//! ```yaml
//! ctxSet:
//!   vars:
//!     - name: "rate_tier"
//!       from:
//!         type: header
//!         name: "X-Plan"
//!       mapping:
//!         values:
//!           premium: "tier_1"
//!           enterprise: "tier_1"
//!           basic: "tier_2"
//!         default: "tier_3"
//! ```
//!
//! ### Template with interpolation
//! ```yaml
//! ctxSet:
//!   vars:
//!     - name: "rate_key"
//!       template: "${header:X-Tenant}_${clientIp}"
//! ```

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::common::KeyGet;

/// CtxSet plugin configuration
///
/// Sets context variables that can be accessed by downstream plugins.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct CtxSetConfig {
    /// List of context variable rules
    pub vars: Vec<CtxVarRule>,

    /// Compiled regex patterns for extract (populated during validation)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_patterns: HashMap<usize, Regex>,

    /// Compiled regex patterns for transform.replace (populated during validation)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_transform_patterns: HashMap<usize, Regex>,

    /// Compiled templates for variable interpolation (populated during validation)
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_templates: HashMap<usize, CompiledTemplate>,
}

/// Rule for setting a single context variable
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CtxVarRule {
    /// Target context variable name
    pub name: String,

    /// Value source (using unified KeyGet accessor)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<KeyGet>,

    /// Template string with variable interpolation
    /// Supports: ${header:X-Name}, ${query:name}, ${clientIp}, ${path}, ${method}
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,

    /// Static value (highest priority if set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,

    /// Regex extraction configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<ExtractConfig>,

    /// Value transformation configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformConfig>,

    /// Value mapping configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping: Option<MappingConfig>,

    /// Default value when source is empty or extraction/transformation fails
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Configuration for regex extraction
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtractConfig {
    /// Regex pattern for extraction
    pub regex: String,

    /// Capture group to use (1-indexed, default: 1)
    /// Group 0 is the entire match, group 1 is the first capture group, etc.
    #[serde(default = "default_group")]
    pub group: usize,
}

fn default_group() -> usize {
    1
}

/// Value transformation configuration
///
/// Transformations are applied after extraction and before mapping.
/// Only one transformation type should be specified at a time.
///
/// ## YAML Examples:
///
/// ```yaml
/// # Regex replace
/// transform:
///   replace:
///     pattern: "^/api/v[0-9]+/"
///     with: "/"
///
/// # Case conversion
/// transform:
///   case: lower
///
/// # Substring extraction
/// transform:
///   substring: [0, 8]
///
/// # Add prefix
/// transform:
///   prefix: "user_"
///
/// # Add suffix
/// transform:
///   suffix: "_v2"
///
/// # Trim whitespace
/// transform:
///   trim: true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransformConfig {
    /// Regex replace: { pattern, with }
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<ReplaceConfig>,

    /// Substring extraction: [start, end] (0-indexed, exclusive end)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub substring: Option<(usize, usize)>,

    /// Case conversion: "upper" or "lower"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<CaseType>,

    /// Add prefix to value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    /// Add suffix to value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,

    /// Trim whitespace from both ends
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<bool>,
}

/// Replace configuration for regex replacement
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceConfig {
    /// Regex pattern to match
    pub pattern: String,
    /// Replacement string (supports $1, $2 for capture groups)
    #[serde(default)]
    pub with: String,
}

impl TransformConfig {
    /// Get the transform type for validation and processing
    pub fn get_transform_type(&self) -> Option<TransformType> {
        if let Some(ref replace) = self.replace {
            return Some(TransformType::Replace {
                pattern: replace.pattern.clone(),
                with: replace.with.clone(),
            });
        }
        if let Some((start, end)) = self.substring {
            return Some(TransformType::Substring(start, end));
        }
        if let Some(ref case) = self.case {
            return Some(TransformType::Case(case.clone()));
        }
        if let Some(ref prefix) = self.prefix {
            return Some(TransformType::Prefix(prefix.clone()));
        }
        if let Some(ref suffix) = self.suffix {
            return Some(TransformType::Suffix(suffix.clone()));
        }
        if self.trim == Some(true) {
            return Some(TransformType::Trim);
        }
        None
    }
}

/// Internal transform type for processing (not serialized)
#[derive(Debug, Clone)]
pub enum TransformType {
    /// Regex replace
    Replace { pattern: String, with: String },
    /// Substring extraction
    Substring(usize, usize),
    /// Case conversion
    Case(CaseType),
    /// Add prefix
    Prefix(String),
    /// Add suffix
    Suffix(String),
    /// Trim whitespace
    Trim,
}

/// Case conversion type
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CaseType {
    /// Convert to uppercase
    Upper,
    /// Convert to lowercase
    Lower,
}

/// Value mapping configuration
///
/// Maps input values to output values with optional default fallback.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MappingConfig {
    /// Mapping table: input -> output
    #[serde(default)]
    pub values: HashMap<String, String>,

    /// Default value when no mapping matches
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Known template variable types
#[derive(Debug, Clone)]
pub enum TemplateVarType {
    Header,
    Query,
    Cookie,
    Ctx,
    /// Current normalized path (without query string).
    Path,
    /// Current normalized full URI (current path + current query).
    Uri,
    /// Entry-time raw path (frozen, immune to rewrites).
    RequestPath,
    /// Entry-time raw full URI (frozen).
    RequestUri,
    Method,
    ClientIp,
    /// Kubernetes Secret reference `${secretRef:ns/name:key}`. Webhook-only: in
    /// ctx_set it resolves to empty and is rejected at validation. The resolved
    /// value is carried inline by the controller (never read at gateway runtime
    /// from a global store).
    SecretRef,
}

impl TemplateVarType {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "header" => Ok(Self::Header),
            "query" => Ok(Self::Query),
            "cookie" => Ok(Self::Cookie),
            "ctx" => Ok(Self::Ctx),
            "path" => Ok(Self::Path),
            "uri" => Ok(Self::Uri),
            "requestPath" => Ok(Self::RequestPath),
            "requestUri" => Ok(Self::RequestUri),
            "method" => Ok(Self::Method),
            "clientIp" => Ok(Self::ClientIp),
            "secretRef" => Ok(Self::SecretRef),
            _ => Err(format!("unknown variable type: {}", s)),
        }
    }
}

/// Pre-compiled template segment
#[derive(Debug, Clone)]
pub enum TemplateSegment {
    /// Literal text, output as-is
    Literal(String),
    /// Variable reference, resolved at runtime
    Variable {
        var_type: TemplateVarType,
        var_name: Option<String>,
    },
}

/// Pre-compiled template for safe evaluation.
///
/// By compiling the template at config-load time, runtime evaluation only
/// iterates over fixed segments and never re-parses `${…}` markers.
/// This prevents template injection: user-controlled values that contain
/// `${…}` are emitted as literal text instead of being expanded.
#[derive(Debug, Clone)]
pub struct CompiledTemplate {
    segments: Vec<TemplateSegment>,
    /// Estimated result length for pre-allocating the output buffer
    hint_len: usize,
}

const VAR_HINT_BYTES: usize = 32;

impl CompiledTemplate {
    /// Compile a template string into segments.
    /// Returns Err if the template has unclosed `${` or an unknown variable type.
    pub fn compile(template: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut hint_len: usize = 0;
        let mut remaining = template;

        while let Some(start) = remaining.find("${") {
            // Push literal before the `${`
            if start > 0 {
                let lit = remaining[..start].to_string();
                hint_len += lit.len();
                segments.push(TemplateSegment::Literal(lit));
            }

            let after_start = &remaining[start + 2..];
            match after_start.find('}') {
                Some(end) => {
                    let var_expr = &after_start[..end];
                    if var_expr.is_empty() {
                        return Err("empty variable expression: ${}".to_string());
                    }
                    let (var_type_str, var_name) = if let Some(colon_pos) = var_expr.find(':') {
                        (&var_expr[..colon_pos], Some(var_expr[colon_pos + 1..].to_string()))
                    } else {
                        (var_expr, None)
                    };
                    let var_type = TemplateVarType::parse(var_type_str)?;
                    hint_len += VAR_HINT_BYTES;
                    segments.push(TemplateSegment::Variable { var_type, var_name });
                    remaining = &after_start[end + 1..];
                }
                None => {
                    return Err(format!("unclosed template variable at: ${{{}", after_start));
                }
            }
        }

        // Push trailing literal
        if !remaining.is_empty() {
            hint_len += remaining.len();
            segments.push(TemplateSegment::Literal(remaining.to_string()));
        }

        Ok(CompiledTemplate { segments, hint_len })
    }

    /// Iterate over the compiled segments
    pub fn segments(&self) -> &[TemplateSegment] {
        &self.segments
    }

    /// Estimated output length for pre-allocating buffers
    pub fn hint_len(&self) -> usize {
        self.hint_len
    }
}

/// Parsed components of a `${secretRef:ns/name:key}` variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRefParts {
    pub namespace: String,
    pub name: String,
    pub key: String,
}

/// Parse the `var_name` of a `SecretRef` segment (the raw `"ns/name:key"`
/// string, already stripped of the `secretRef:` prefix by `compile`).
///
/// Requires an explicit namespace and a non-empty key. The map key used to
/// carry the resolved value is the unparsed `var_name` itself; this function is
/// only for the controller, which needs the split components to fetch the Secret.
pub fn parse_secret_ref(var_name: &str) -> Result<SecretRefParts, String> {
    let (path, key) = var_name
        .rsplit_once(':')
        .ok_or_else(|| format!("secretRef '{}' is missing ':key' (expected ns/name:key)", var_name))?;
    if key.is_empty() {
        return Err(format!("secretRef '{}' has an empty key", var_name));
    }
    let (namespace, name) = path.split_once('/').ok_or_else(|| {
        format!(
            "secretRef '{}' is missing 'namespace/' (expected ns/name:key)",
            var_name
        )
    })?;
    if namespace.is_empty() {
        return Err(format!("secretRef '{}' has an empty namespace", var_name));
    }
    if name.is_empty() {
        return Err(format!("secretRef '{}' has an empty name", var_name));
    }
    Ok(SecretRefParts {
        namespace: namespace.to_string(),
        name: name.to_string(),
        key: key.to_string(),
    })
}

/// Collect the raw `var_name`s of every `SecretRef` segment in a compiled
/// template (e.g. `"ns/name:key"`). These are the canonical `resolved_secrets`
/// map keys.
pub fn secret_ref_var_names(tpl: &CompiledTemplate) -> Vec<String> {
    tpl.segments()
        .iter()
        .filter_map(|seg| match seg {
            TemplateSegment::Variable {
                var_type: TemplateVarType::SecretRef,
                var_name: Some(name),
            } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// True if the template contains ANY `secretRef` segment (including a malformed
/// bare `${secretRef}` with no `ns/name:key`).
pub fn template_has_secret_ref(tpl: &CompiledTemplate) -> bool {
    tpl.segments().iter().any(|seg| {
        matches!(
            seg,
            TemplateSegment::Variable {
                var_type: TemplateVarType::SecretRef,
                ..
            }
        )
    })
}

/// Validate every `secretRef` segment in a compiled template: each must be a
/// well-formed `${secretRef:ns/name:key}` (explicit ns + key). A bare
/// `${secretRef}` (no name) is rejected. Returns the first error.
pub fn validate_secret_ref_segments(tpl: &CompiledTemplate) -> Result<(), String> {
    for seg in tpl.segments() {
        if let TemplateSegment::Variable {
            var_type: TemplateVarType::SecretRef,
            var_name,
        } = seg
        {
            match var_name {
                Some(name) => {
                    parse_secret_ref(name)?;
                }
                None => return Err("secretRef requires the form ${secretRef:ns/name:key}".to_string()),
            }
        }
    }
    Ok(())
}

impl CtxSetConfig {
    /// Check all fields for validity without mutating self.
    ///
    /// Mirrors every error branch in `validate()` without side effects.
    /// Called by `PluginValidation::validation_error` for controller pre-parse.
    fn check_fields(&self) -> Option<String> {
        if self.vars.is_empty() {
            return Some("vars list is empty".to_string());
        }
        for (idx, rule) in self.vars.iter().enumerate() {
            if rule.name.is_empty() {
                return Some(format!("vars[{}]: name is empty", idx));
            }
            if rule.value.is_none() && rule.from.is_none() && rule.template.is_none() {
                return Some(format!(
                    "vars[{}] '{}': must specify 'value', 'from', or 'template'",
                    idx, rule.name
                ));
            }
            if let Some(ref extract) = rule.extract {
                if let Err(e) = Regex::new(&extract.regex) {
                    return Some(format!(
                        "vars[{}] '{}': invalid extract regex '{}': {}",
                        idx, rule.name, extract.regex, e
                    ));
                }
            }
            if let Some(ref transform) = rule.transform {
                if let Some(TransformType::Replace { ref pattern, .. }) = transform.get_transform_type() {
                    if let Err(e) = Regex::new(pattern) {
                        return Some(format!(
                            "vars[{}] '{}': invalid transform.replace pattern '{}': {}",
                            idx, rule.name, pattern, e
                        ));
                    }
                }
                if let Some(TransformType::Substring(start, end)) = transform.get_transform_type() {
                    if start > end {
                        return Some(format!(
                            "vars[{}] '{}': substring start ({}) > end ({})",
                            idx, rule.name, start, end
                        ));
                    }
                }
            }
            if let Some(ref template) = rule.template {
                match CompiledTemplate::compile(template) {
                    Err(e) => return Some(format!("vars[{}] '{}': invalid template: {}", idx, rule.name, e)),
                    Ok(compiled) => {
                        if template_has_secret_ref(&compiled) {
                            return Some(format!(
                                "vars[{}] '{}': secretRef is only supported in webhook templates, not ctx_set",
                                idx, rule.name
                            ));
                        }
                    }
                }
            }
        }
        None
    }

    /// Validate configuration and compile regex patterns.
    ///
    /// Returns `Err(message)` on the first invalid rule. On success the compiled
    /// regex / template caches are populated.
    pub fn validate(&mut self) -> Result<(), String> {
        self.compiled_patterns.clear();
        self.compiled_transform_patterns.clear();
        self.compiled_templates.clear();

        if self.vars.is_empty() {
            return Err("vars list is empty".to_string());
        }

        for (idx, rule) in self.vars.iter().enumerate() {
            // Validate rule name
            if rule.name.is_empty() {
                return Err(format!("vars[{}]: name is empty", idx));
            }

            // Must have at least one value source
            if rule.value.is_none() && rule.from.is_none() && rule.template.is_none() {
                return Err(format!(
                    "vars[{}] '{}': must specify 'value', 'from', or 'template'",
                    idx, rule.name
                ));
            }

            // Compile extract regex if present
            if let Some(ref extract) = rule.extract {
                match Regex::new(&extract.regex) {
                    Ok(re) => {
                        self.compiled_patterns.insert(idx, re);
                    }
                    Err(e) => {
                        return Err(format!(
                            "vars[{}] '{}': invalid extract regex '{}': {}",
                            idx, rule.name, extract.regex, e
                        ));
                    }
                }
            }

            // Validate transform regex if present
            if let Some(ref transform) = rule.transform {
                if let Some(TransformType::Replace { ref pattern, .. }) = transform.get_transform_type() {
                    match Regex::new(pattern) {
                        Ok(re) => {
                            self.compiled_transform_patterns.insert(idx, re);
                        }
                        Err(e) => {
                            return Err(format!(
                                "vars[{}] '{}': invalid transform.replace pattern '{}': {}",
                                idx, rule.name, pattern, e
                            ));
                        }
                    }
                }

                // Validate substring bounds
                if let Some(TransformType::Substring(start, end)) = transform.get_transform_type() {
                    if start > end {
                        return Err(format!(
                            "vars[{}] '{}': substring start ({}) > end ({})",
                            idx, rule.name, start, end
                        ));
                    }
                }
            }

            // Compile template if present
            if let Some(ref template) = rule.template {
                match CompiledTemplate::compile(template) {
                    Ok(compiled) => {
                        if template_has_secret_ref(&compiled) {
                            return Err(format!(
                                "vars[{}] '{}': secretRef is only supported in webhook templates, not ctx_set",
                                idx, rule.name
                            ));
                        }
                        self.compiled_templates.insert(idx, compiled);
                    }
                    Err(e) => {
                        return Err(format!("vars[{}] '{}': invalid template: {}", idx, rule.name, e));
                    }
                }
            }
        }
        Ok(())
    }

    /// Get compiled extract regex for a rule by index
    pub fn get_compiled_regex(&self, idx: usize) -> Option<&Regex> {
        self.compiled_patterns.get(&idx)
    }

    /// Get compiled transform.replace regex for a rule by index
    pub fn get_compiled_transform_regex(&self, idx: usize) -> Option<&Regex> {
        self.compiled_transform_patterns.get(&idx)
    }

    /// Get compiled template for a rule by index
    pub fn get_compiled_template(&self, idx: usize) -> Option<&CompiledTemplate> {
        self.compiled_templates.get(&idx)
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for CtxSetConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_from_header_config() {
        let yaml = r#"
vars:
  - name: user_id
    from:
      type: header
      name: X-User-Id
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        assert_eq!(config.vars.len(), 1);
        assert_eq!(config.vars[0].name, "user_id");
    }

    #[test]
    fn test_static_value_config() {
        let yaml = r#"
vars:
  - name: env
    value: production
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        assert_eq!(config.vars[0].value, Some("production".to_string()));
    }

    #[test]
    fn test_template_config() {
        let yaml = r#"
vars:
  - name: rate_key
    template: "${header:X-Tenant}_${clientIp}"
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        assert_eq!(
            config.vars[0].template,
            Some("${header:X-Tenant}_${clientIp}".to_string())
        );
    }

    #[test]
    fn test_extract_config() {
        let yaml = r#"
vars:
  - name: jwt_user
    from:
      type: header
      name: Authorization
    extract:
      regex: "Bearer .*\\.(.+)\\."
      group: 1
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        assert!(config.get_compiled_regex(0).is_some());
    }

    #[test]
    fn test_transform_replace_config() {
        let yaml = r#"
vars:
  - name: clean_path
    from:
      type: path
    transform:
      replace:
        pattern: "^/api/v[0-9]+/"
        with: "/"
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
    }

    #[test]
    fn test_transform_case_config() {
        let yaml = r#"
vars:
  - name: method_lower
    from:
      type: method
    transform:
      case: lower
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        let transform = config.vars[0].transform.as_ref().unwrap();
        assert!(matches!(transform.case, Some(CaseType::Lower)));
    }

    #[test]
    fn test_transform_substring_config() {
        let yaml = r#"
vars:
  - name: short_trace
    from:
      type: header
      name: X-Trace-Id
    transform:
      substring: [0, 8]
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        let transform = config.vars[0].transform.as_ref().unwrap();
        assert_eq!(transform.substring, Some((0, 8)));
    }

    #[test]
    fn test_transform_prefix_suffix_config() {
        let yaml = r#"
vars:
  - name: prefixed_id
    from:
      type: header
      name: X-Id
    transform:
      prefix: "user_"
  - name: suffixed_id
    from:
      type: header
      name: X-Id
    transform:
      suffix: "_v2"
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        let t0 = config.vars[0].transform.as_ref().unwrap();
        let t1 = config.vars[1].transform.as_ref().unwrap();
        assert!(t0.prefix.is_some());
        assert!(t1.suffix.is_some());
    }

    #[test]
    fn test_mapping_config() {
        let yaml = r#"
vars:
  - name: rate_tier
    from:
      type: header
      name: X-Plan
    mapping:
      values:
        premium: tier_1
        enterprise: tier_1
        basic: tier_2
      default: tier_3
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        let mapping = config.vars[0].mapping.as_ref().unwrap();
        assert_eq!(mapping.values.get("premium"), Some(&"tier_1".to_string()));
        assert_eq!(mapping.default, Some("tier_3".to_string()));
    }

    #[test]
    fn test_full_pipeline_config() {
        let yaml = r#"
vars:
  - name: tenant_id
    from:
      type: header
      name: Authorization
    extract:
      regex: '"tenant":"([^"]+)"'
      group: 1
    transform:
      case: lower
    mapping:
      values:
        acme: tenant_acme
        globex: tenant_globex
      default: tenant_default
    default: unknown_tenant
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
    }

    #[test]
    fn test_validation_empty_vars() {
        let yaml = r#"
vars: []
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.contains("vars list is empty"));
    }

    #[test]
    fn test_validation_empty_name() {
        let yaml = r#"
vars:
  - name: ""
    value: test
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.contains("name is empty"));
    }

    #[test]
    fn test_validation_no_value_source() {
        let yaml = r#"
vars:
  - name: test_var
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.contains("must specify"));
    }

    #[test]
    fn test_validation_invalid_extract_regex() {
        let yaml = r#"
vars:
  - name: test
    from:
      type: header
      name: X-Test
    extract:
      regex: "([invalid"
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.contains("invalid extract regex"));
    }

    #[test]
    fn test_validation_invalid_transform_regex() {
        let yaml = r#"
vars:
  - name: test
    from:
      type: path
    transform:
      replace:
        pattern: "([invalid"
        with: ""
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.contains("invalid transform.replace pattern"));
    }

    #[test]
    fn test_validation_invalid_substring_bounds() {
        let yaml = r#"
vars:
  - name: test
    from:
      type: header
      name: X-Test
    transform:
      substring: [10, 5]
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.contains("substring start"));
    }

    #[test]
    fn test_template_compilation() {
        let compiled = CompiledTemplate::compile("hello ${header:X-User} from ${clientIp}").unwrap();
        let segs = compiled.segments();
        assert_eq!(segs.len(), 4);
        assert!(matches!(&segs[0], TemplateSegment::Literal(s) if s == "hello "));
        assert!(
            matches!(&segs[1], TemplateSegment::Variable { var_type, var_name } if matches!(var_type, TemplateVarType::Header) && var_name.as_deref() == Some("X-User"))
        );
        assert!(matches!(&segs[2], TemplateSegment::Literal(s) if s == " from "));
        assert!(
            matches!(&segs[3], TemplateSegment::Variable { var_type, var_name } if matches!(var_type, TemplateVarType::ClientIp) && var_name.is_none())
        );
        assert!(compiled.hint_len() > 0);
    }

    #[test]
    fn test_template_compilation_unclosed() {
        let result = CompiledTemplate::compile("hello ${header:X-User");
        assert!(result.is_err());
    }

    #[test]
    fn test_template_compilation_unknown_var_type() {
        let result = CompiledTemplate::compile("${unknown:foo}");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown variable type"));
    }

    #[test]
    fn test_template_compilation_no_variables() {
        let compiled = CompiledTemplate::compile("just a literal string").unwrap();
        assert_eq!(compiled.segments().len(), 1);
        assert!(matches!(&compiled.segments()[0], TemplateSegment::Literal(s) if s == "just a literal string"));
    }

    /// Regression test: `validation_error()` via `PluginValidation` must work
    /// without calling `validate()` first, and must NOT touch the cached field.
    #[test]
    fn validation_error_without_validate_call() {
        let config = CtxSetConfig::default(); // vars is empty
        assert_eq!(config.validation_error(), Some("vars list is empty".to_string()));
    }

    #[test]
    fn test_template_validation_catches_unclosed() {
        let yaml = r#"
vars:
  - name: bad_template
    template: "hello ${header:oops"
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.contains("invalid template"));
    }

    #[test]
    fn secret_ref_variant_parses() {
        let compiled = CompiledTemplate::compile("${secretRef:edgion-system/tok:key}").expect("compile");
        let names = secret_ref_var_names(&compiled);
        assert_eq!(names, vec!["edgion-system/tok:key".to_string()]);
    }

    #[test]
    fn parse_secret_ref_valid() {
        let p = parse_secret_ref("edgion-system/tok:api-key").expect("valid");
        assert_eq!(p.namespace, "edgion-system");
        assert_eq!(p.name, "tok");
        assert_eq!(p.key, "api-key");
    }

    #[test]
    fn parse_secret_ref_rejects_missing_key() {
        assert!(parse_secret_ref("edgion-system/tok").is_err());
    }

    #[test]
    fn parse_secret_ref_rejects_missing_namespace() {
        assert!(parse_secret_ref("tok:key").is_err());
    }

    #[test]
    fn ctx_set_rejects_secret_ref_template() {
        let cfg: CtxSetConfig = serde_json::from_value(serde_json::json!({
            "vars": [ { "name": "x", "template": "${secretRef:ns/n:k}" } ]
        }))
        .expect("deserialize");
        let err = cfg.check_fields().expect("must reject");
        assert!(err.contains("secretRef"), "got: {err}");
    }

    #[test]
    fn ctx_set_rejects_bare_secret_ref() {
        // A bare `${secretRef}` (no ns/name:key) must also be rejected by check_fields.
        let cfg: CtxSetConfig = serde_json::from_value(serde_json::json!({
            "vars": [ { "name": "x", "template": "${secretRef}" } ]
        }))
        .expect("deserialize");
        let err = cfg.check_fields().expect("must reject bare secretRef");
        assert!(err.contains("secretRef"), "got: {err}");
    }
}
