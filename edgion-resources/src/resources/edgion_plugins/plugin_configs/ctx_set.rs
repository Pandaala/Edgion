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
//!       template: "${header:X-Tenant}_${remoteIp}"
//! ```

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::common::KeyGet;
#[cfg(test)]
use crate::resources::common::request_template::{
    parse_secret_ref, secret_ref_var_names, TemplateSegment, TemplateVarType,
};
use crate::resources::common::request_template::{CompiledTemplate, SecretPolicy, TemplateAdmission, VariablePolicy};

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
    /// Supports: ${header:X-Name}, ${query:name}, ${remoteIp}, ${path}, ${method}
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
/// A rule may configure zero or one operation. `trim: false` is not an
/// operation. Additional steps belong on a later rule that reads the previous
/// context variable.
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
    /// Count configured operations. `trim: false` is not an operation.
    fn configured_operation_count(&self) -> usize {
        usize::from(self.replace.is_some())
            + usize::from(self.substring.is_some())
            + usize::from(self.case.is_some())
            + usize::from(self.prefix.is_some())
            + usize::from(self.suffix.is_some())
            + usize::from(self.trim == Some(true))
    }

    fn multiple_operation_error(&self, idx: usize, name: &str) -> Option<String> {
        (self.configured_operation_count() > 1)
            .then(|| format!("vars[{idx}] '{name}': transform allows at most one operation"))
    }

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
                if let Some(error) = transform.multiple_operation_error(idx, &rule.name) {
                    return Some(error);
                }
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
                        if (TemplateAdmission {
                            variables: VariablePolicy::Request,
                            secrets: SecretPolicy::Deny,
                        })
                        .validate(&compiled)
                        .is_err()
                        {
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
                if let Some(error) = transform.multiple_operation_error(idx, &rule.name) {
                    return Err(error);
                }
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
                        if (TemplateAdmission {
                            variables: VariablePolicy::Request,
                            secrets: SecretPolicy::Deny,
                        })
                        .validate(&compiled)
                        .is_err()
                        {
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
    template: "${header:X-Tenant}_${remoteIp}"
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        assert_eq!(
            config.vars[0].template,
            Some("${header:X-Tenant}_${remoteIp}".to_string())
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

    fn parse_ctx_set(yaml: &str) -> CtxSetConfig {
        serde_yaml::from_str(yaml).expect("ctx set yaml should deserialize")
    }

    fn single_transform_yaml(transform: &str) -> String {
        format!("vars:\n  - name: user\n    value: ALICE\n    transform:\n{transform}")
    }

    #[test]
    fn transform_accepts_zero_or_one_operation() {
        for yaml in [
            single_transform_yaml("      {}"),
            single_transform_yaml("      trim: false"),
            single_transform_yaml("      replace:\n        pattern: A\n        with: x"),
            single_transform_yaml("      substring: [0, 1]"),
            single_transform_yaml("      case: lower"),
            single_transform_yaml("      prefix: user_"),
            single_transform_yaml("      suffix: _id"),
            single_transform_yaml("      trim: true"),
            single_transform_yaml("      prefix: user_\n      trim: false"),
        ] {
            let mut config = parse_ctx_set(&yaml);
            config
                .validate()
                .unwrap_or_else(|err| panic!("expected valid transform, got {err}: {yaml}"));
            assert_eq!(config.validation_error(), None, "{yaml}");
        }
    }

    #[test]
    fn transform_rejects_multiple_operations() {
        let expected = "vars[0] 'user': transform allows at most one operation";
        for yaml in [
            single_transform_yaml("      case: lower\n      prefix: user_"),
            single_transform_yaml("      prefix: user_\n      trim: true"),
            single_transform_yaml("      substring: [0, 8]\n      suffix: _id"),
            single_transform_yaml("      replace:\n        pattern: A\n        with: x\n      case: lower"),
        ] {
            let mut config = parse_ctx_set(&yaml);
            assert_eq!(config.validate().unwrap_err(), expected, "{yaml}");
            assert_eq!(config.validation_error().as_deref(), Some(expected), "{yaml}");
        }
    }

    #[test]
    fn chained_ctx_rules_may_apply_one_transform_each() {
        let mut config = parse_ctx_set(
            r#"
vars:
  - name: user
    value: ALICE
    transform:
      case: lower
  - name: user
    from:
      type: ctx
      name: user
    transform:
      prefix: "user_"
"#,
        );
        config.validate().expect("chained single-operation rules remain valid");
        assert_eq!(config.validation_error(), None);
    }

    #[test]
    fn test_template_compilation() {
        let compiled = CompiledTemplate::compile("hello ${header:X-User} from ${remoteIp}").unwrap();
        let segs = compiled.segments();
        assert_eq!(segs.len(), 4);
        assert!(matches!(&segs[0], TemplateSegment::Literal(s) if s == "hello "));
        assert!(
            matches!(&segs[1], TemplateSegment::Variable { var_type, var_name } if matches!(var_type, TemplateVarType::Header) && var_name.as_deref() == Some("X-User"))
        );
        assert!(matches!(&segs[2], TemplateSegment::Literal(s) if s == " from "));
        assert!(
            matches!(&segs[3], TemplateSegment::Variable { var_type, var_name } if matches!(var_type, TemplateVarType::RemoteIp) && var_name.is_none())
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
    fn old_client_ip_template_variable_is_rejected() {
        let result = CompiledTemplate::compile("${clientIp}");
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

    #[test]
    fn test_matched_route_tokens_accepted() {
        assert!(CompiledTemplate::compile("${matchedHttpRoute:name}").is_ok());
        assert!(CompiledTemplate::compile("${matchedGrpcRoute:annotation.owner}").is_ok());
        assert!(CompiledTemplate::compile("${matchedFooRoute:name}").is_err());
    }
}
