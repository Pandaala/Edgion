//! Plugin entry types with enable/disable functionality

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
///   - conditionEnable:                # conditional enable
///       timeBefore: "2024-12-31T23:59:59Z"
///     type: requestRedirect
///     config:
///       hostname: example.com
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Conditional enable configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_enable: Option<ConditionEnable>,

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
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            condition_enable: None,
            plugin,
        }
    }

    /// Create a new plugin entry with condition enable
    pub fn with_condition(plugin: EdgionPlugin, condition: ConditionEnable) -> Self {
        Self {
            enable: true,
            condition_enable: Some(condition),
            plugin,
        }
    }

    /// Check if this plugin is enabled (considering both enable flag and condition)
    pub fn is_enabled(&self) -> bool {
        if !self.enable {
            return false;
        }
        // Check condition if present
        if let Some(condition) = &self.condition_enable {
            return condition.is_satisfied();
        }
        true
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

    /// Conditional enable configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_enable: Option<ConditionEnable>,

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
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            condition_enable: None,
            plugin,
        }
    }

    /// Create a new plugin entry with condition enable
    pub fn with_condition(plugin: EdgionPlugin, condition: ConditionEnable) -> Self {
        Self {
            enable: true,
            condition_enable: Some(condition),
            plugin,
        }
    }

    /// Check if this plugin is enabled (considering both enable flag and condition)
    pub fn is_enabled(&self) -> bool {
        if !self.enable {
            return false;
        }
        // Check condition if present
        if let Some(condition) = &self.condition_enable {
            return condition.is_satisfied();
        }
        true
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

    /// Conditional enable configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_enable: Option<ConditionEnable>,

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
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            condition_enable: None,
            plugin,
        }
    }

    /// Create a new plugin entry with condition enable
    pub fn with_condition(plugin: EdgionPlugin, condition: ConditionEnable) -> Self {
        Self {
            enable: true,
            condition_enable: Some(condition),
            plugin,
        }
    }

    /// Check if this plugin is enabled (considering both enable flag and condition)
    pub fn is_enabled(&self) -> bool {
        if !self.enable {
            return false;
        }
        // Check condition if present
        if let Some(condition) = &self.condition_enable {
            return condition.is_satisfied();
        }
        true
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

    /// Conditional enable configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_enable: Option<ConditionEnable>,

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
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            condition_enable: None,
            plugin,
        }
    }

    /// Create a new plugin entry with condition enable
    pub fn with_condition(plugin: EdgionPlugin, condition: ConditionEnable) -> Self {
        Self {
            enable: true,
            condition_enable: Some(condition),
            plugin,
        }
    }

    /// Check if this plugin is enabled (considering both enable flag and condition)
    pub fn is_enabled(&self) -> bool {
        if !self.enable {
            return false;
        }
        // Check condition if present
        if let Some(condition) = &self.condition_enable {
            return condition.is_satisfied();
        }
        true
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

