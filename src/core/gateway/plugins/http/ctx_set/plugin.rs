//! CtxSet plugin implementation
//!
//! Sets context variables from various sources with optional extraction,
//! transformation, and value mapping.
//!
//! ## Processing Pipeline:
//!
//! ```text
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//! │   from/     │ ──▶ │   extract   │ ──▶ │  transform  │ ──▶ │   mapping   │ ──▶ ctx.set(name, value)
//! │  template   │     │  (regex)    │     │ (replace) │     │  ()     │
//! └─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘
//!                          ↓                  ↓               ↓
//!                      default            default         mapping.default
//! ```
//!
//! ## Features:
//! - Phase 1: from + name basic setting, default, template support
//! - Phase 2: extract regex extraction
//! - Phase 3: transform.replace, transform.case, mapping

use async_trait::async_trait;
use regex::Regex;

use crate::core::gateway::plugins::runtime::log::la;
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{
    CaseType, CompiledTemplate, CtxSetConfig, CtxVarRule, TemplateSegment, TemplateVarType, TransformConfig,
    TransformType,
};

// ============================================================================
// CtxSet Plugin
// ============================================================================

/// CtxSet plugin for setting context variables
///
/// Sets context variables that can be accessed by downstream plugins,
/// typically used for passing extracted values (like tenant ID, user tier, etc.)
/// between plugins.
pub struct CtxSet {
    name: String,
    config: CtxSetConfig,
}

impl CtxSet {
    /// Create a new CtxSet plugin from configuration
    pub fn create(config: &CtxSetConfig) -> Box<dyn RequestFilter> {
        let mut validated_config = config.clone();
        let _ = validated_config.validate();

        let plugin = CtxSet {
            name: "CtxSet".to_string(),
            config: validated_config,
        };

        Box::new(plugin)
    }

    /// Evaluate a pre-compiled template by resolving each segment.
    ///
    /// Because the template structure is fixed at config-load time, user-supplied
    /// values that happen to contain `${…}` are emitted verbatim -- they are
    /// never re-parsed as template directives.  This eliminates the template
    /// injection vulnerability present in the old `resolve_template` approach.
    fn evaluate_template(&self, compiled: &CompiledTemplate, session: &dyn PluginSession) -> String {
        let mut result = String::with_capacity(compiled.hint_len());
        for segment in compiled.segments() {
            match segment {
                TemplateSegment::Literal(lit) => result.push_str(lit),
                TemplateSegment::Variable { var_type, var_name } => {
                    let value = Self::resolve_var(var_type, var_name.as_deref(), session);
                    result.push_str(&value);
                }
            }
        }
        result
    }

    /// Resolve a single variable by type and optional name
    fn resolve_var(var_type: &TemplateVarType, var_name: Option<&str>, session: &dyn PluginSession) -> String {
        match var_type {
            TemplateVarType::Header => var_name.and_then(|n| session.header_value(n)).unwrap_or_default(),
            TemplateVarType::Query => var_name.and_then(|n| session.get_query_param(n)).unwrap_or_default(),
            TemplateVarType::Cookie => var_name.and_then(|n| session.get_cookie(n)).unwrap_or_default(),
            TemplateVarType::Ctx => var_name.and_then(|n| session.get_ctx_var(n)).unwrap_or_default(),
            TemplateVarType::Path => session.get_path().to_string(),
            TemplateVarType::Uri => session.get_uri(),
            TemplateVarType::RequestPath => session.get_request_path().to_string(),
            TemplateVarType::RequestUri => session.get_request_uri(),
            TemplateVarType::Method => session.get_method().to_string(),
            TemplateVarType::ClientIp => session.remote_addr().to_string(),
        }
    }

    /// Process a single variable rule
    ///
    /// Returns the final value to set, or None if the value should not be set
    async fn process_rule(
        &self,
        rule: &CtxVarRule,
        rule_idx: usize,
        session: &dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> Option<String> {
        // Priority: value > template > from
        let mut value = if let Some(ref static_value) = rule.value {
            // Static value has highest priority
            static_value.clone()
        } else if let Some(compiled) = self.config.get_compiled_template(rule_idx) {
            self.evaluate_template(compiled, session)
        } else if let Some(ref from) = rule.from {
            // Get from KeyGet source
            session.key_get(from).await.or(rule.default.clone())?
        } else {
            // No source specified (should be caught by validation)
            return rule.default.clone();
        };

        // Apply extract (regex capture)
        if let Some(ref _extract) = rule.extract {
            if let Some(regex) = self.config.get_compiled_regex(rule_idx) {
                value = match self.apply_extract(&value, regex, _extract.group) {
                    Some(extracted) => extracted,
                    None => {
                        return rule.default.clone();
                    }
                };
            }
        }

        // Apply transform
        if let Some(ref transform) = rule.transform {
            let compiled_replace = self.config.get_compiled_transform_regex(rule_idx);
            value = match self.apply_transform(&value, transform, compiled_replace, plugin_log) {
                Some(transformed) => transformed,
                None => {
                    return rule.default.clone();
                }
            };
        }

        // Apply mapping
        if let Some(ref mapping) = rule.mapping {
            value = if let Some(mapped) = mapping.values.get(&value) {
                mapped.clone()
            } else if let Some(ref default) = mapping.default {
                default.clone()
            } else {
                // No mapping match and no mapping default, use rule default
                return rule.default.clone();
            };
        }

        Some(value)
    }

    /// Apply regex extraction
    fn apply_extract(&self, value: &str, regex: &Regex, group: usize) -> Option<String> {
        regex
            .captures(value)
            .and_then(|caps| caps.get(group))
            .map(|m| m.as_str().to_string())
    }

    /// Apply transformation
    fn apply_transform(
        &self,
        value: &str,
        transform: &TransformConfig,
        compiled_replace: Option<&Regex>,
        plugin_log: &mut PluginLog,
    ) -> Option<String> {
        // Get the transform type from the config struct
        let transform_type = transform.get_transform_type()?;

        match transform_type {
            TransformType::Replace { with, .. } => match compiled_replace {
                Some(re) => Some(re.replace_all(value, with.as_str()).to_string()),
                None => {
                    // Invariant: validate() compiles every Replace pattern; reaching
                    // here means the config bypassed validation. Fail-open via default.
                    plugin_log.push_err("transform regex missing");
                    None
                }
            },
            TransformType::Substring(start, end) => {
                let chars: Vec<char> = value.chars().collect();
                let start = start.min(chars.len());
                let end = end.min(chars.len());
                Some(chars[start..end].iter().collect())
            }
            TransformType::Case(case_type) => match case_type {
                CaseType::Upper => Some(value.to_uppercase()),
                CaseType::Lower => Some(value.to_lowercase()),
            },
            TransformType::Prefix(prefix) => Some(format!("{}{}", prefix, value)),
            TransformType::Suffix(suffix) => Some(format!("{}{}", value, suffix)),
            TransformType::Trim => Some(value.trim().to_string()),
        }
    }
}

#[async_trait]
impl RequestFilter for CtxSet {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        for (idx, rule) in self.config.vars.iter().enumerate() {
            let value = self.process_rule(rule, idx, session, plugin_log).await;

            match value {
                Some(v) => {
                    if let Err(_) = session.set_ctx_var(&rule.name, &v) {
                        plugin_log.push_err("ctx var set fail");
                    } else {
                        plugin_log.push_fmt("key {} set", la![rule.name.as_str()]);
                    }
                }
                None => {}
            }
        }

        PluginRunningResult::GoodNext
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;

    #[tokio::test]
    async fn test_template_injection_prevention() {
        let yaml = r#"
vars:
  - name: injected
    template: "${header:X-Inject}"
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate().expect("validate should succeed");
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        // Attacker sends header value containing template syntax
        mock_session
            .expect_header_value()
            .withf(|name| name == "X-Inject")
            .return_const(Some("${header:Authorization}".to_string()));
        // The value should be the literal string, NOT the resolved Authorization header
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "injected" && value == "${header:Authorization}")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }
}
