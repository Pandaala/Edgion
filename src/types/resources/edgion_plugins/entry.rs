//! Plugin entry types with enable/disable functionality

use crate::core::gateway::plugins::runtime::conditions::PluginConditions;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::edgion_plugin::EdgionPlugin;

/// Helper functions for serde defaults
pub(super) fn default_true() -> bool {
    true
}

pub(super) fn is_true(v: &bool) -> bool {
    *v
}

/// Validate a plugin entry alias.
///
/// Rule: must match `^[a-zA-Z0-9._-]{1,32}$`. Returns the human-readable
/// error string if invalid, or `None` if valid. The error format is
/// suitable for inclusion in the `EdgionPlugins` validation_error vector.
pub(super) fn validate_alias_str(alias: &str) -> Option<String> {
    let len = alias.len();
    if !(1..=32).contains(&len) {
        return Some(format!("alias must match [a-zA-Z0-9._-]{{1,32}} (got {len} bytes)"));
    }
    let ok = alias
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
    if !ok {
        return Some("alias must match [a-zA-Z0-9._-]{1,32} (contains disallowed character)".to_string());
    }
    None
}

/// Plugin entry with enable switch
///
/// YAML format:
/// ```yaml
/// plugins:
///   - enable: true                    # optional, defaults to true
///     type: requestHeaderModifier
///     config:
///       set:
///         - name: X-Test
///           value: test-value
///   - enable: false                   # disabled plugin
///     type: responseHeaderModifier
///     config:
///       add:
///         - name: X-Response
///           value: added
///   - conditions:                     # conditional execution
///       skip:
///         - keyExist:
///             source: header
///             key: "X-Internal"
///       run:
///         - timeRange:
///             startTime: "2024-01-01T00:00:00Z"
///             endTime: "2024-12-31T23:59:59Z"
///     type: ipRestriction
///     config:
///       allow: ["10.0.0.0/8"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Plugin conditions for conditional execution
    /// Supports skip/run conditions with various matchers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// Operator-defined label echoed verbatim into PluginLog.alias.
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$`.
    /// Lets multiple entries of the same plugin type be distinguished
    /// in the access log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl PluginEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            conditions: Some(conditions),
            alias: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Check if this plugin is enabled
    /// Note: This only checks the enable flag. For conditions,
    /// use `conditions()` and evaluate with request context at runtime.
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Check if this entry has conditions defined
    pub fn has_conditions(&self) -> bool {
        self.conditions.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// RequestFilterEntry - for request stage plugins (async)
///
/// Supports plugins:
/// - BasicAuth, Cors, Csrf, IpRestriction, JwtAuth, Mock (from edgion_plugins)
/// - RequestHeaderModifier, RequestRedirect (from gapi_filters)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestFilterEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Plugin conditions for conditional execution
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// Operator-defined label echoed verbatim into PluginLog.alias.
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$`.
    /// Lets multiple entries of the same plugin type be distinguished
    /// in the access log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl RequestFilterEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            conditions: Some(conditions),
            alias: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Check if this plugin is enabled
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Check if this entry has conditions defined
    pub fn has_conditions(&self) -> bool {
        self.conditions.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// UpstreamResponseFilterEntry - for upstream response filter stage plugins (sync)
///
/// Supports plugins:
/// - ResponseHeaderModifier (from gapi_filters)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamResponseFilterEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Plugin conditions for conditional execution
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// Operator-defined label echoed verbatim into PluginLog.alias.
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$`.
    /// Lets multiple entries of the same plugin type be distinguished
    /// in the access log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl UpstreamResponseFilterEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            conditions: Some(conditions),
            alias: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Check if this plugin is enabled
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Check if this entry has conditions defined
    pub fn has_conditions(&self) -> bool {
        self.conditions.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// UpstreamResponseBodyFilterEntry - for upstream response body filter stage plugins (sync)
///
/// Supports plugins:
/// - BandwidthLimit (from edgion_plugins)
///
/// These plugins run during the upstream_response_body_filter hook for each body chunk.
/// They return an optional Duration to control bandwidth throttling.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamResponseBodyFilterEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Plugin conditions for conditional execution
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// Operator-defined label echoed verbatim into PluginLog.alias.
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$`.
    /// Lets multiple entries of the same plugin type be distinguished
    /// in the access log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl UpstreamResponseBodyFilterEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            conditions: Some(conditions),
            alias: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Check if this plugin is enabled
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Check if this entry has conditions defined
    pub fn has_conditions(&self) -> bool {
        self.conditions.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// UpstreamResponseEntry - for upstream response stage plugins (async)
///
/// Currently no plugins, reserved for future expansion
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamResponseEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Plugin conditions for conditional execution
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// Operator-defined label echoed verbatim into PluginLog.alias.
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$`.
    /// Lets multiple entries of the same plugin type be distinguished
    /// in the access log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl UpstreamResponseEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            plugin,
        }
    }

    /// Create a new plugin entry with conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            conditions: Some(conditions),
            alias: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Check if this plugin is enabled
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Check if this entry has conditions defined
    pub fn has_conditions(&self) -> bool {
        self.conditions.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

#[cfg(test)]
mod alias_validation_tests {
    use super::*;

    fn check(s: &str) -> Option<String> {
        validate_alias_str(s)
    }

    #[test]
    fn accepts_letters_digits_dot_dash_underscore_within_32_bytes() {
        assert!(check("a").is_none());
        assert!(check("auth-internal").is_none());
        assert!(check("auth_v2.1").is_none());
        assert!(check("ABC.def-ghi_123").is_none());
        assert!(check(&"a".repeat(32)).is_none());
    }

    #[test]
    fn rejects_empty_string() {
        let err = check("").expect("empty alias must be rejected");
        assert!(err.contains("[a-zA-Z0-9._-]"), "{err}");
    }

    #[test]
    fn rejects_over_32_bytes() {
        let err = check(&"a".repeat(33)).expect("33-char alias must be rejected");
        assert!(err.contains("[a-zA-Z0-9._-]"), "{err}");
    }

    #[test]
    fn rejects_disallowed_chars() {
        for bad in [
            "space here",
            "slash/here",
            // Multi-byte UTF-8 must be rejected (validator allows ASCII alphanumerics + . _ - only)
            "caf\u{e9}",
            "with\nnewline",
            "quote\"here",
            "/leading",
        ] {
            assert!(check(bad).is_some(), "must reject {bad:?}");
        }
    }

    #[test]
    fn entry_with_alias_serde_roundtrip() {
        use crate::types::resources::edgion_plugins::EdgionPlugin;
        use crate::types::resources::plugin_configs::CorsConfig;

        let yaml = r#"
alias: auth-internal
type: Cors
config: {}
"#;
        let entry: PluginEntry = serde_yaml::from_str(yaml).expect("yaml parse");
        assert_eq!(entry.alias.as_deref(), Some("auth-internal"));
        assert!(matches!(entry.plugin, EdgionPlugin::Cors(_)));

        // Round-trip retains the field
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["alias"], serde_json::json!("auth-internal"));

        // No alias -> field absent in serialized form
        let entry_no_alias = PluginEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
        let json = serde_json::to_value(&entry_no_alias).unwrap();
        assert!(json.get("alias").is_none(), "alias must be omitted when None: {json}");
    }

    #[test]
    fn entry_validate_alias_returns_error_for_invalid() {
        use crate::types::resources::edgion_plugins::EdgionPlugin;
        use crate::types::resources::plugin_configs::CorsConfig;

        let mut entry = PluginEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
        assert_eq!(entry.validate_alias(), None);

        entry.alias = Some("bad/alias".to_string());
        let err = entry.validate_alias().expect("must reject");
        assert!(err.contains("[a-zA-Z0-9._-]"), "{err}");

        entry.alias = Some("ok-name_v1.2".to_string());
        assert_eq!(entry.validate_alias(), None);
    }
}
