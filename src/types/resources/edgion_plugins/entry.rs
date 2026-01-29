//! Plugin entry types with enable/disable functionality

use crate::core::plugins::plugins_com::PluginConditions;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::gateway_api_plugins::EdgionPlugin;

/// Helper functions for serde defaults
pub(super) fn default_true() -> bool {
    true
}

pub(super) fn is_true(v: &bool) -> bool {
    *v
}

/// Conditional enable configuration
///
/// YAML format:
/// ```yaml
/// conditionEnable:
///   timeBefore: "2024-12-31T23:59:59Z"   # Plugin enabled before this time
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConditionEnable {
    /// Plugin is enabled before this time (RFC3339 format)
    /// Example: "2024-12-31T23:59:59Z"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_before: Option<String>,
}

impl ConditionEnable {
    /// Check if the condition is satisfied at the current time
    pub fn is_satisfied(&self) -> bool {
        if let Some(time_before) = &self.time_before {
            if let Ok(deadline) = chrono::DateTime::parse_from_rfc3339(time_before) {
                return chrono::Utc::now() < deadline;
            }
        }
        // If no condition or parse error, default to true
        true
    }
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
///   - conditionEnable:                # simple conditional enable (legacy)
///       timeBefore: "2024-12-31T23:59:59Z"
///     type: requestRedirect
///     config:
///       hostname: example.com
///   - conditions:                     # advanced conditions
///       skip:
///         - keyExist:
///             source: header
///             key: "X-Internal"
///       run:
///         - timeRange:
///             after: "2024-01-01T00:00:00Z"
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

    /// Simple conditional enable configuration (legacy, use `conditions` for advanced use)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_enable: Option<ConditionEnable>,

    /// Advanced plugin conditions for conditional execution
    /// Supports skip/run conditions with various matchers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl PluginEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            condition_enable: None,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            condition_enable: None,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with simple condition enable
    pub fn with_condition(plugin: EdgionPlugin, condition: ConditionEnable) -> Self {
        Self {
            enable: true,
            condition_enable: Some(condition),
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with advanced conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            condition_enable: None,
            conditions: Some(conditions),
            plugin,
        }
    }

    /// Check if this plugin is enabled (considering enable flag and simple condition)
    /// Note: This only checks basic enable state. For advanced conditions,
    /// use `conditions()` and evaluate with request context at runtime.
    pub fn is_enabled(&self) -> bool {
        if !self.enable {
            return false;
        }
        // Check simple condition if present
        if let Some(condition) = &self.condition_enable {
            return condition.is_satisfied();
        }
        true
    }

    /// Get the advanced conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Check if this entry has advanced conditions defined
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
/// - BasicAuth, Cors, Csrf, IpRestriction, Mock (from edgion_plugins)
/// - RequestHeaderModifier, RequestRedirect, ExtensionRef (from gapi_filters)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestFilterEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Simple conditional enable configuration (legacy)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_enable: Option<ConditionEnable>,

    /// Advanced plugin conditions for conditional execution
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl RequestFilterEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            condition_enable: None,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            condition_enable: None,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with simple condition enable
    pub fn with_condition(plugin: EdgionPlugin, condition: ConditionEnable) -> Self {
        Self {
            enable: true,
            condition_enable: Some(condition),
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with advanced conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            condition_enable: None,
            conditions: Some(conditions),
            plugin,
        }
    }

    /// Check if this plugin is enabled (considering enable flag and simple condition)
    pub fn is_enabled(&self) -> bool {
        if !self.enable {
            return false;
        }
        // Check simple condition if present
        if let Some(condition) = &self.condition_enable {
            return condition.is_satisfied();
        }
        true
    }

    /// Get the advanced conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Check if this entry has advanced conditions defined
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

    /// Simple conditional enable configuration (legacy)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_enable: Option<ConditionEnable>,

    /// Advanced plugin conditions for conditional execution
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl UpstreamResponseFilterEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            condition_enable: None,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            condition_enable: None,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with simple condition enable
    pub fn with_condition(plugin: EdgionPlugin, condition: ConditionEnable) -> Self {
        Self {
            enable: true,
            condition_enable: Some(condition),
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with advanced conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            condition_enable: None,
            conditions: Some(conditions),
            plugin,
        }
    }

    /// Check if this plugin is enabled (considering enable flag and simple condition)
    pub fn is_enabled(&self) -> bool {
        if !self.enable {
            return false;
        }
        // Check simple condition if present
        if let Some(condition) = &self.condition_enable {
            return condition.is_satisfied();
        }
        true
    }

    /// Get the advanced conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Check if this entry has advanced conditions defined
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

    /// Simple conditional enable configuration (legacy)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_enable: Option<ConditionEnable>,

    /// Advanced plugin conditions for conditional execution
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// The actual plugin configuration
    #[serde(flatten)]
    pub plugin: EdgionPlugin,
}

impl UpstreamResponseEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            condition_enable: None,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            condition_enable: None,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with simple condition enable
    pub fn with_condition(plugin: EdgionPlugin, condition: ConditionEnable) -> Self {
        Self {
            enable: true,
            condition_enable: Some(condition),
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with advanced conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            condition_enable: None,
            conditions: Some(conditions),
            plugin,
        }
    }

    /// Check if this plugin is enabled (considering enable flag and simple condition)
    pub fn is_enabled(&self) -> bool {
        if !self.enable {
            return false;
        }
        // Check simple condition if present
        if let Some(condition) = &self.condition_enable {
            return condition.is_satisfied();
        }
        true
    }

    /// Get the advanced conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Check if this entry has advanced conditions defined
    pub fn has_conditions(&self) -> bool {
        self.conditions.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}
