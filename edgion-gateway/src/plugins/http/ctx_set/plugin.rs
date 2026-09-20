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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    fn parse_config(yaml: &str) -> CtxSetConfig {
        serde_yaml::from_str(yaml).expect("config should deserialize")
    }

    fn parse_transform(yaml: &str) -> TransformConfig {
        serde_yaml::from_str(yaml).expect("transform should deserialize")
    }

    #[test]
    fn name_is_static_across_instances() {
        let first = CtxSet::create(&parse_config(
            r#"
vars:
  - name: first
    value: one
"#,
        ));
        let second = CtxSet::create(&parse_config(
            r#"
vars:
  - name: second
    value: two
"#,
        ));

        assert_eq!(first.name(), "CtxSet");
        assert_eq!(second.name(), "CtxSet");
    }

    #[test]
    fn source_value_wins_and_missing_source_uses_default() {
        let plugin = CtxSet {
            config: parse_config(
                r#"
vars:
  - name: result
    from:
      type: header
      name: X-Source
    default: fallback
"#,
            ),
        };
        let rule = &plugin.config.vars[0];

        let mut source_session = MockPluginSession::new();
        source_session.expect_key_get().return_const(Some("source".to_string()));
        let mut source_log = PluginLog::new("CtxSet");
        assert_eq!(
            plugin.process_rule(rule, 0, &source_session, &mut source_log),
            Some("source".to_string())
        );

        let mut missing_session = MockPluginSession::new();
        missing_session.expect_key_get().return_const(None::<String>);
        let mut missing_log = PluginLog::new("CtxSet");
        assert_eq!(
            plugin.process_rule(rule, 0, &missing_session, &mut missing_log),
            Some("fallback".to_string())
        );
    }

    #[test]
    fn malformed_multi_operation_transform_keeps_priority() {
        let plugin = CtxSet {
            config: CtxSetConfig::default(),
        };
        let mut plugin_log = PluginLog::new("CtxSet");

        let replace_first = parse_transform(
            r#"
replace:
  pattern: a
  with: x
substring: [1, 2]
case: upper
prefix: pre-
suffix: -post
trim: true
"#,
        );
        let replace_regex = Regex::new("a").unwrap();
        assert_eq!(
            plugin.apply_transform("abc", &replace_first, Some(&replace_regex), &mut plugin_log),
            Some("xbc".to_string())
        );

        let substring_first = parse_transform(
            r#"
substring: [1, 3]
case: upper
prefix: pre-
suffix: -post
trim: true
"#,
        );
        assert_eq!(
            plugin.apply_transform("abcd", &substring_first, None, &mut plugin_log),
            Some("bc".to_string())
        );

        let case_first = parse_transform(
            r#"
case: upper
prefix: pre-
suffix: -post
trim: true
"#,
        );
        assert_eq!(
            plugin.apply_transform("ab", &case_first, None, &mut plugin_log),
            Some("AB".to_string())
        );

        let prefix_first = parse_transform(
            r#"
prefix: pre-
suffix: -post
trim: true
"#,
        );
        assert_eq!(
            plugin.apply_transform("ab", &prefix_first, None, &mut plugin_log),
            Some("pre-ab".to_string())
        );

        let suffix_first = parse_transform(
            r#"
suffix: -post
trim: true
"#,
        );
        assert_eq!(
            plugin.apply_transform("ab", &suffix_first, None, &mut plugin_log),
            Some("ab-post".to_string())
        );
    }

    #[test]
    fn substring_uses_unicode_scalar_indices_and_clamps_ranges() {
        let plugin = CtxSet {
            config: CtxSetConfig::default(),
        };
        let mut plugin_log = PluginLog::new("CtxSet");

        for (range, expected) in [
            ((1, 3), "é€"),
            ((2, 99), "€z"),
            ((99, 100), ""),
            ((2, 2), ""),
            ((3, 1), ""),
        ] {
            let transform = parse_transform(&format!("substring: [{}, {}]", range.0, range.1));
            assert_eq!(
                plugin.apply_transform("aé€z", &transform, None, &mut plugin_log),
                Some(expected.to_string())
            );
        }
    }

    #[test]
    fn missing_compiled_replace_logs_error_and_uses_rule_default() {
        let plugin = CtxSet {
            config: parse_config(
                r#"
vars:
  - name: result
    from:
      type: header
      name: X-Source
    transform:
      replace:
        pattern: source
        with: transformed
    default: fallback
"#,
            ),
        };
        let rule = &plugin.config.vars[0];
        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(Some("source".to_string()));
        let mut plugin_log = PluginLog::new("CtxSet");

        assert_eq!(
            plugin.process_rule(rule, 0, &session, &mut plugin_log),
            Some("fallback".to_string())
        );
        assert!(plugin_log.contains("err: transform regex missing"));
    }

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
            .expect_request_header_map()
            .return_const(http::HeaderMap::from_iter([(
                http::HeaderName::from_bytes(b"X-Inject").unwrap(),
                http::HeaderValue::from_static("${header:Authorization}"),
            )]));
        // The value should be the literal string, NOT the resolved Authorization header
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "injected" && value == "${header:Authorization}")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[test]
    fn compile_plugins_rejects_multiple_transform_operations() {
        use crate::plugins::http::compile_plugins;
        use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins, RequestFilterEntry};

        let config = parse_config(
            r#"
vars:
  - name: user
    value: ALICE
    transform:
      case: lower
      prefix: user_
"#,
        );
        let mut resource = EdgionPlugins::new("ctx-set-multi-transform", Default::default());
        resource.metadata.namespace = Some("default".to_string());
        resource.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::CtxSet(config))]);

        let schema_errors = resource.validate_plugins();
        let compiled = compile_plugins(&resource);
        assert_eq!(compiled.preparse_errors, schema_errors);
        assert_eq!(
            compiled.preparse_errors,
            vec!["requestPlugins[0] (CtxSet): vars[0] 'user': transform allows at most one operation".to_string()]
        );
        assert!(!compiled.is_activatable);
    }

    #[test]
    fn compile_plugins_accepts_chained_single_transform_rules() {
        use crate::plugins::http::compile_plugins;
        use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins, RequestFilterEntry};

        let config = parse_config(
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
      prefix: user_
"#,
        );
        let mut resource = EdgionPlugins::new("ctx-set-chained-transform", Default::default());
        resource.metadata.namespace = Some("default".to_string());
        resource.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::CtxSet(config))]);

        let compiled = compile_plugins(&resource);
        assert!(compiled.preparse_errors.is_empty());
        assert!(resource.validate_plugins().is_empty());
        assert!(compiled.is_activatable);
    }
}
