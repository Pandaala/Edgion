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
                if let Some(replace) = transform.replace.as_ref() {
                    let pattern = &replace.pattern;
                    if let Err(e) = Regex::new(pattern) {
                        return Some(format!(
                            "vars[{}] '{}': invalid transform.replace pattern '{}': {}",
                            idx, rule.name, pattern, e
                        ));
                    }
                }
                if let Some((start, end)) = transform.substring {
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
                if let Some(replace) = transform.replace.as_ref() {
                    let pattern = &replace.pattern;
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
                if let Some((start, end)) = transform.substring {
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
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/ctx_set_test.rs"]
mod tests;
