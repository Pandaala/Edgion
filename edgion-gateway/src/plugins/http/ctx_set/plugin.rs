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
use async_trait::async_trait;
use regex::Regex;

use crate::link_sys::template::evaluate_template;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{CaseType, CtxSetConfig, CtxVarRule, TransformConfig};

// ============================================================================
// CtxSet Plugin
// ============================================================================

/// CtxSet plugin for setting context variables
///
/// Sets context variables that can be accessed by downstream plugins,
/// typically used for passing extracted values (like tenant ID, user tier, etc.)
/// between plugins.
pub struct CtxSet {
    config: CtxSetConfig,
}

impl CtxSet {
    /// Create a new CtxSet plugin from configuration
    pub fn create(config: &CtxSetConfig) -> Box<dyn RequestFilter> {
        let mut validated_config = config.clone();
        let _ = validated_config.validate();

        let plugin = CtxSet {
            config: validated_config,
        };

        Box::new(plugin)
    }

    /// Process a single variable rule
    ///
    /// Returns the final value to set, or None if the value should not be set
    fn process_rule(
        &self,
        rule: &CtxVarRule,
        rule_idx: usize,
        session: &dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> Option<String> {
        // Priority: value > template > from
        let mut value = if let Some(ref static_value) = rule.value {
            static_value.clone()
        } else if let Some(compiled) = self.config.get_compiled_template(rule_idx) {
            evaluate_template(compiled, session)
        } else if let Some(ref from) = rule.from {
            // Get from KeyGet source
            session.key_get(from).or_else(|| rule.default.clone())?
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
        if let Some(replace) = transform.replace.as_ref() {
            return match compiled_replace {
                Some(re) => Some(re.replace_all(value, replace.with.as_str()).to_string()),
                None => {
                    // Invariant: validate() compiles every Replace pattern; reaching
                    // here means the config bypassed validation. Fail-open via default.
                    plugin_log.push_err("transform regex missing");
                    None
                }
            };
        }

        if let Some((start, end)) = transform.substring {
            return Some(value.chars().skip(start).take(end.saturating_sub(start)).collect());
        }

        if let Some(case_type) = transform.case.as_ref() {
            return match case_type {
                CaseType::Upper => Some(value.to_uppercase()),
                CaseType::Lower => Some(value.to_lowercase()),
            };
        }

        if let Some(prefix) = transform.prefix.as_deref() {
            let mut transformed = String::with_capacity(prefix.len() + value.len());
            transformed.push_str(prefix);
            transformed.push_str(value);
            return Some(transformed);
        }

        if let Some(suffix) = transform.suffix.as_deref() {
            let mut transformed = String::with_capacity(value.len() + suffix.len());
            transformed.push_str(value);
            transformed.push_str(suffix);
            return Some(transformed);
        }

        if transform.trim == Some(true) {
            return Some(value.trim().to_string());
        }

        None
    }
}

#[async_trait]
impl RequestFilter for CtxSet {
    fn name(&self) -> &str {
        "CtxSet"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        for (idx, rule) in self.config.vars.iter().enumerate() {
            let value = self.process_rule(rule, idx, session, plugin_log);

            if let Some(v) = value {
                if session.set_ctx_var(&rule.name, &v).is_err() {
                    plugin_log.push_err("ctx var set fail");
                } else {
                    plugin_log.push_fmt("key {} set", la![rule.name.as_str()]);
                }
            }
        }

        PluginRunningResult::GoodNext
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/ctx_set/plugin_test.rs"]
mod tests;
