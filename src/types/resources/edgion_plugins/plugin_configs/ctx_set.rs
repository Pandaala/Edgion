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

use crate::types::common::KeyGet;

// ============================================================================
// CtxSet Configuration
// ============================================================================

/// CtxSet plugin configuration
///
/// Sets context variables that can be accessed by downstream plugins.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct CtxSetConfig {
    /// List of context variable rules
    pub vars: Vec<CtxVarRule>,

    /// Internal validation error (set during validation)
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,

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

// ============================================================================
// Extraction Configuration
// ============================================================================

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

// ============================================================================
// Transformation Configuration
// ============================================================================

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

// ============================================================================
// Mapping Configuration
// ============================================================================

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

// ============================================================================
// Pre-compiled Template
// ============================================================================

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

// ============================================================================
// Validation
// ============================================================================

impl CtxSetConfig {
    /// Validate configuration and compile regex patterns
    pub fn validate(&mut self) {
        self.validation_error = None;
        self.compiled_patterns.clear();
        self.compiled_transform_patterns.clear();
        self.compiled_templates.clear();

        if self.vars.is_empty() {
            self.validation_error = Some("vars list is empty".to_string());
            return;
        }

        for (idx, rule) in self.vars.iter().enumerate() {
            // Validate rule name
            if rule.name.is_empty() {
                self.validation_error = Some(format!("vars[{}]: name is empty", idx));
                return;
            }

            // Must have at least one value source
            if rule.value.is_none() && rule.from.is_none() && rule.template.is_none() {
                self.validation_error = Some(format!(
                    "vars[{}] '{}': must specify 'value', 'from', or 'template'",
                    idx, rule.name
                ));
                return;
            }

            // Compile extract regex if present
            if let Some(ref extract) = rule.extract {
                match Regex::new(&extract.regex) {
                    Ok(re) => {
                        self.compiled_patterns.insert(idx, re);
                    }
                    Err(e) => {
                        self.validation_error = Some(format!(
                            "vars[{}] '{}': invalid extract regex '{}': {}",
                            idx, rule.name, extract.regex, e
                        ));
                        return;
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
                            self.validation_error = Some(format!(
                                "vars[{}] '{}': invalid transform.replace pattern '{}': {}",
                                idx, rule.name, pattern, e
                            ));
                            return;
                        }
                    }
                }

                // Validate substring bounds
                if let Some(TransformType::Substring(start, end)) = transform.get_transform_type() {
                    if start > end {
                        self.validation_error = Some(format!(
                            "vars[{}] '{}': substring start ({}) > end ({})",
                            idx, rule.name, start, end
                        ));
                        return;
                    }
                }
            }

            // Compile template if present
            if let Some(ref template) = rule.template {
                match CompiledTemplate::compile(template) {
                    Ok(compiled) => {
                        self.compiled_templates.insert(idx, compiled);
                    }
                    Err(e) => {
                        self.validation_error = Some(format!("vars[{}] '{}': invalid template: {}", idx, rule.name, e));
                        return;
                    }
                }
            }
        }
    }

    /// Check if configuration is valid
    pub fn is_valid(&self) -> bool {
        self.validation_error.is_none()
    }

    /// Get validation error message
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
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

// ============================================================================
// Tests
// ============================================================================

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
        config.validate();
        assert!(config.is_valid(), "Error: {:?}", config.validation_error);
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
        config.validate();
        assert!(config.is_valid());
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
        config.validate();
        assert!(config.is_valid());
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
        config.validate();
        assert!(config.is_valid(), "Error: {:?}", config.validation_error);
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
        config.validate();
        assert!(config.is_valid(), "Error: {:?}", config.validation_error);
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
        config.validate();
        assert!(config.is_valid());
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
        config.validate();
        assert!(config.is_valid());
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
        config.validate();
        assert!(config.is_valid());
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
        config.validate();
        assert!(config.is_valid());
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
        config.validate();
        assert!(config.is_valid(), "Error: {:?}", config.validation_error);
    }

    #[test]
    fn test_validation_empty_vars() {
        let yaml = r#"
vars: []
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("vars list is empty"));
    }

    #[test]
    fn test_validation_empty_name() {
        let yaml = r#"
vars:
  - name: ""
    value: test
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("name is empty"));
    }

    #[test]
    fn test_validation_no_value_source() {
        let yaml = r#"
vars:
  - name: test_var
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("must specify"));
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
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("invalid extract regex"));
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
        config.validate();
        assert!(!config.is_valid());
        assert!(config
            .get_validation_error()
            .unwrap()
            .contains("invalid transform.replace pattern"));
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
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("substring start"));
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

    #[test]
    fn test_template_validation_catches_unclosed() {
        let yaml = r#"
vars:
  - name: bad_template
    template: "hello ${header:oops"
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("invalid template"));
    }
}
