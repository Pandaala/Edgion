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
//! │  template   │     │  (regex)    │     │ (replace等) │     │  (可选)     │
//! └─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘
//!                          ↓ 失败                 ↓ 失败              ↓ 无匹配
//!                     使用 default           使用 default        使用 mapping.default
//! ```
//!
//! ## Features:
//! - Phase 1: from + name basic setting, default, template support
//! - Phase 2: extract regex extraction
//! - Phase 3: transform.replace, transform.case, mapping

use async_trait::async_trait;
use regex::Regex;
use tracing::debug;

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{CaseType, CtxSetConfig, CtxVarRule, TransformConfig, TransformType};

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
        validated_config.validate();

        let plugin = CtxSet {
            name: "CtxSet".to_string(),
            config: validated_config,
        };

        Box::new(plugin)
    }

    /// Resolve a template string with variable interpolation
    ///
    /// Supports:
    /// - ${header:X-Name} - request header value
    /// - ${query:name} - query parameter value
    /// - ${cookie:name} - cookie value
    /// - ${path} - request path
    /// - ${method} - HTTP method
    /// - ${clientIp} - client IP address
    /// - ${ctx:name} - context variable
    fn resolve_template(&self, template: &str, session: &dyn PluginSession) -> String {
        let mut result = template.to_string();

        // Simple pattern matching for ${type:name} or ${type}
        // Using a loop to handle all occurrences
        while let Some(start) = result.find("${") {
            let end = match result[start..].find('}') {
                Some(pos) => start + pos,
                None => break,
            };

            let var_expr = &result[start + 2..end];
            let replacement = self.resolve_var_expr(var_expr, session);

            result = format!("{}{}{}", &result[..start], replacement, &result[end + 1..]);
        }

        result
    }

    /// Resolve a single variable expression (without ${})
    fn resolve_var_expr(&self, expr: &str, session: &dyn PluginSession) -> String {
        // Parse type:name or just type
        let (var_type, var_name) = if let Some(colon_pos) = expr.find(':') {
            (&expr[..colon_pos], Some(&expr[colon_pos + 1..]))
        } else {
            (expr, None)
        };

        match var_type {
            "header" => {
                if let Some(name) = var_name {
                    session.header_value(name).unwrap_or_default()
                } else {
                    String::new()
                }
            }
            "query" => {
                if let Some(name) = var_name {
                    session.get_query_param(name).unwrap_or_default()
                } else {
                    String::new()
                }
            }
            "cookie" => {
                if let Some(name) = var_name {
                    session.get_cookie(name).unwrap_or_default()
                } else {
                    String::new()
                }
            }
            "ctx" => {
                if let Some(name) = var_name {
                    session.get_ctx_var(name).unwrap_or_default()
                } else {
                    String::new()
                }
            }
            "path" => session.get_path().to_string(),
            "method" => session.get_method().to_string(),
            "clientIp" => session.remote_addr().to_string(),
            _ => {
                // Unknown variable type, return empty
                debug!("Unknown template variable type: {}", var_type);
                String::new()
            }
        }
    }

    /// Process a single variable rule
    ///
    /// Returns the final value to set, or None if the value should not be set
    async fn process_rule(&self, rule: &CtxVarRule, rule_idx: usize, session: &dyn PluginSession) -> Option<String> {
        // Priority: value > template > from
        let mut value = if let Some(ref static_value) = rule.value {
            // Static value has highest priority
            static_value.clone()
        } else if let Some(ref template) = rule.template {
            // Template interpolation
            self.resolve_template(template, session)
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
                value = match self.apply_extract(&value, regex, rule.extract.as_ref().unwrap().group) {
                    Some(extracted) => extracted,
                    None => {
                        debug!(
                            "CtxSet: extract regex did not match for var '{}', using default",
                            rule.name
                        );
                        return rule.default.clone();
                    }
                };
            }
        }

        // Apply transform
        if let Some(ref transform) = rule.transform {
            value = match self.apply_transform(&value, transform) {
                Some(transformed) => transformed,
                None => {
                    debug!("CtxSet: transform failed for var '{}', using default", rule.name);
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
    fn apply_transform(&self, value: &str, transform: &TransformConfig) -> Option<String> {
        // Get the transform type from the config struct
        let transform_type = transform.get_transform_type()?;

        match transform_type {
            TransformType::Replace { pattern, with } => {
                // Compile regex for replacement
                match Regex::new(&pattern) {
                    Ok(re) => Some(re.replace_all(value, with.as_str()).to_string()),
                    Err(_) => None, // Should be caught by validation
                }
            }
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
        // Check for configuration errors
        if !self.config.is_valid() {
            let error = self.config.get_validation_error().unwrap_or("Unknown error");
            plugin_log.push(&format!("Config error: {}; ", error));
            // Fail-open: allow request to proceed on config error
            return PluginRunningResult::GoodNext;
        }

        let mut set_count = 0;
        let mut skip_count = 0;

        for (idx, rule) in self.config.vars.iter().enumerate() {
            // Process the rule to get the final value
            let value = self.process_rule(rule, idx, session).await;

            match value {
                Some(v) => {
                    // Set the context variable
                    if let Err(e) = session.set_ctx_var(&rule.name, &v) {
                        plugin_log.push(&format!("Failed to set ctx '{}': {}; ", rule.name, e));
                    } else {
                        set_count += 1;
                        debug!("CtxSet: set ctx '{}' = '{}'", rule.name, v);
                    }
                }
                None => {
                    skip_count += 1;
                    debug!("CtxSet: skipped ctx '{}' (no value, no default)", rule.name);
                }
            }
        }

        plugin_log.push(&format!("Set {} vars, skipped {}; ", set_count, skip_count));
        PluginRunningResult::GoodNext
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
    use crate::types::common::KeyGet;

    fn create_basic_config() -> CtxSetConfig {
        let yaml = r#"
vars:
  - name: test_var
    value: static_value
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        config
    }

    #[tokio::test]
    async fn test_static_value() {
        let config = create_basic_config();
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "test_var" && value == "static_value")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Set 1 vars"));
    }

    #[tokio::test]
    async fn test_from_header() {
        let yaml = r#"
vars:
  - name: user_id
    from:
      type: header
      name: X-User-Id
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::Header { name } if name == "X-User-Id"))
            .return_const(Some("user123".to_string()));
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "user_id" && value == "user123")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_from_header_missing_with_default() {
        let yaml = r#"
vars:
  - name: user_id
    from:
      type: header
      name: X-User-Id
    default: anonymous
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::Header { name } if name == "X-User-Id"))
            .return_const(None);
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "user_id" && value == "anonymous")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_template_interpolation() {
        let yaml = r#"
vars:
  - name: rate_key
    template: "${header:X-Tenant}_${clientIp}"
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_header_value()
            .withf(|name| name == "X-Tenant")
            .return_const(Some("acme".to_string()));
        mock_session
            .expect_remote_addr()
            .return_const("192.168.1.1".to_string());
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "rate_key" && value == "acme_192.168.1.1")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_regex_extract() {
        let yaml = r#"
vars:
  - name: tenant_id
    from:
      type: header
      name: Authorization
    extract:
      regex: '"tenant":"([^"]+)"'
      group: 1
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_key_get()
            .return_const(Some(r#"{"tenant":"acme_corp","user":"test"}"#.to_string()));
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "tenant_id" && value == "acme_corp")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_transform_case_lower() {
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
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::Method))
            .return_const(Some("GET".to_string()));
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "method_lower" && value == "get")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_transform_replace() {
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
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_key_get()
            .withf(|k| matches!(k, KeyGet::Path))
            .return_const(Some("/api/v2/users/123".to_string()));
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "clean_path" && value == "/users/123")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_transform_substring() {
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
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_key_get()
            .return_const(Some("abc123def456".to_string()));
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "short_trace" && value == "abc123de")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_mapping() {
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
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session.expect_key_get().return_const(Some("premium".to_string()));
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "rate_tier" && value == "tier_1")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_mapping_default() {
        let yaml = r#"
vars:
  - name: rate_tier
    from:
      type: header
      name: X-Plan
    mapping:
      values:
        premium: tier_1
      default: tier_3
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_key_get()
            .return_const(Some("unknown_plan".to_string()));
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "rate_tier" && value == "tier_3")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_full_pipeline() {
        // Extract tenant from JWT-like header, lowercase it, then map to tier
        let yaml = r#"
vars:
  - name: final_tier
    from:
      type: header
      name: X-Token
    extract:
      regex: 'T:([A-Za-z]+)'
      group: 1
    transform:
      case: lower
    mapping:
      values:
        premium: tier_1
        enterprise: tier_1
      default: tier_2
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        // X-Token contains "T:PREMIUM" -> extract "PREMIUM" -> lowercase "premium" -> map to "tier_1"
        mock_session
            .expect_key_get()
            .return_const(Some("abc.T:PREMIUM.xyz".to_string()));
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "final_tier" && value == "tier_1")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn test_config_error_fail_open() {
        let config = CtxSetConfig {
            validation_error: Some("test error".to_string()),
            ..Default::default()
        };
        let plugin = CtxSet {
            name: "CtxSet".to_string(),
            config,
        };

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Config error"));
    }

    #[tokio::test]
    async fn test_multiple_vars() {
        let yaml = r#"
vars:
  - name: var1
    value: value1
  - name: var2
    value: value2
"#;
        let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
        config.validate();
        let plugin = CtxSet::create(&config);

        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("CtxSet");

        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "var1" && value == "value1")
            .returning(|_, _| Ok(()));
        mock_session
            .expect_set_ctx_var()
            .withf(|key, value| key == "var2" && value == "value2")
            .returning(|_, _| Ok(()));

        let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Set 2 vars"));
    }
}
