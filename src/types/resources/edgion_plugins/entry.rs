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
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            conditions: Some(conditions),
            plugin,
        }
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
/// - BasicAuth, Cors, Csrf, IpRestriction, Mock (from edgion_plugins)
/// - RequestHeaderModifier, RequestRedirect, ExtensionRef (from gapi_filters)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestFilterEntry {
    /// Whether this plugin is enabled (default: true)
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,

    /// Plugin conditions for conditional execution
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
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            conditions: Some(conditions),
            plugin,
        }
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
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            conditions: Some(conditions),
            plugin,
        }
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
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            plugin,
        }
    }

    /// Create a new plugin entry with conditions
    pub fn with_conditions(plugin: EdgionPlugin, conditions: PluginConditions) -> Self {
        Self {
            enable: true,
            conditions: Some(conditions),
            plugin,
        }
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
