//! Plugin entry types with enable/disable functionality

use super::body_requirement::BodyRequirement;
use super::conditions::PluginConditions;
use super::dye::{DyeOutcome, DyeSuite};
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
///     type: RequestHeaderModifier
///     config:
///       set:
///         - name: X-Test
///           value: test-value
///   - enable: false                   # disabled plugin
///     type: ResponseHeaderModifier
///     config:
///       add:
///         - name: X-Response
///           value: added
///   - conditions:                     # conditional execution
///       skip:
///         - type: keyExist
///           key:
///             type: header
///             name: "X-Internal"
///       run:
///         - type: timeRange
///           after: "2024-01-01T00:00:00Z"
///           before: "2024-12-31T23:59:59Z"
///     type: IpRestriction
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PluginConditions>,

    /// Operator-defined label echoed verbatim into PluginLog.alias.
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$`.
    /// Lets multiple entries of the same plugin type be distinguished
    /// in the access log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and the 2026-07-20 entry-level dye suite spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

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
            dye: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            dye: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Validate the optional dye suite against rule-local checks + this
    /// plugin's declared capabilities and key sources. `PluginEntry` sits in
    /// a request-stage array, so request-direction rules are allowed.
    pub fn validate_dye(&self) -> Option<String> {
        let dye = self.dye.as_ref()?;
        let keys = self.plugin.key_header_names();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        dye.validation_error(true, self.plugin.dye_capabilities(), &key_refs)
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

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// Plugin entry for the request filter stage (async).
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

    /// Set by the controller when the plugin policy matches this entry.
    /// Never operator-facing: skipped in the CRD schema, carried to the
    /// data plane via conf_sync. The controller clears and re-stamps it on
    /// every preparse, so a hand-written value can never survive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub policy_action: Option<crate::resources::plugin_policy::PluginPolicyAction>,

    /// Optional body requirement. When set (by the operator) or declared
    /// intrinsically by the plugin, the runtime wraps the plugin in a body gate
    /// that buffers the request body (OpenResty-style, at the request phase) and
    /// applies the configured fail-open / fail-close policy on a read failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyRequirement>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and the 2026-07-20 entry-level dye suite spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

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
            policy_action: None,
            body: None,
            dye: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            policy_action: None,
            body: None,
            dye: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Validate the optional dye suite against rule-local checks + this
    /// plugin's declared capabilities and key sources. `RequestFilterEntry`
    /// sits in the request-stage array, so request-direction rules are allowed.
    pub fn validate_dye(&self) -> Option<String> {
        let dye = self.dye.as_ref()?;
        let keys = self.plugin.key_header_names();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        dye.validation_error(true, self.plugin.dye_capabilities(), &key_refs)
    }

    /// Check if this plugin is enabled
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// Plugin entry for the upstream response filter stage (sync).
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

    /// Set by the controller when the plugin policy matches this entry.
    /// Never operator-facing: skipped in the CRD schema, carried to the
    /// data plane via conf_sync. The controller clears and re-stamps it on
    /// every preparse, so a hand-written value can never survive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub policy_action: Option<crate::resources::plugin_policy::PluginPolicyAction>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and the 2026-07-20 entry-level dye suite spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

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
            policy_action: None,
            dye: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            policy_action: None,
            dye: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Validate the optional dye suite against rule-local checks + this
    /// plugin's declared capabilities and key sources. `UpstreamResponseFilterEntry`
    /// is a response-stage entry, so request-direction rules are rejected.
    pub fn validate_dye(&self) -> Option<String> {
        let dye = self.dye.as_ref()?;
        let keys = self.plugin.key_header_names();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        dye.validation_error(false, self.plugin.dye_capabilities(), &key_refs)
    }

    /// Check if this plugin is enabled
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// Plugin entry for the upstream response body filter stage (sync, per body chunk).
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

    /// Set by the controller when the plugin policy matches this entry.
    /// Never operator-facing: skipped in the CRD schema, carried to the
    /// data plane via conf_sync. The controller clears and re-stamps it on
    /// every preparse, so a hand-written value can never survive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub policy_action: Option<crate::resources::plugin_policy::PluginPolicyAction>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and the 2026-07-20 entry-level dye suite spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

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
            policy_action: None,
            dye: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            policy_action: None,
            dye: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Validate the optional dye suite against rule-local checks + this
    /// plugin's declared capabilities and key sources. `UpstreamResponseBodyFilterEntry`
    /// is a response-stage entry, so request-direction rules are rejected.
    pub fn validate_dye(&self) -> Option<String> {
        let dye = self.dye.as_ref()?;
        let keys = self.plugin.key_header_names();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        dye.validation_error(false, self.plugin.dye_capabilities(), &key_refs)
    }

    /// Check if this plugin is enabled
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// Plugin entry for the upstream response stage (async). Reserved for future use.
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

    /// Set by the controller when the plugin policy matches this entry.
    /// Never operator-facing: skipped in the CRD schema, carried to the
    /// data plane via conf_sync. The controller clears and re-stamps it on
    /// every preparse, so a hand-written value can never survive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub policy_action: Option<crate::resources::plugin_policy::PluginPolicyAction>,

    /// Reserved body requirement. Proxied upstream responses are
    /// streaming-only, so validation rejects this field for every currently
    /// supported response-stage plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyRequirement>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and the 2026-07-20 entry-level dye suite spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

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
            policy_action: None,
            body: None,
            dye: None,
            plugin,
        }
    }

    /// Create a new plugin entry with specified enable state
    pub fn with_enable(plugin: EdgionPlugin, enable: bool) -> Self {
        Self {
            enable,
            conditions: None,
            alias: None,
            policy_action: None,
            body: None,
            dye: None,
            plugin,
        }
    }

    /// Validate the optional alias against the entry-level rule.
    /// Returns `Some(error)` if alias is set and invalid; `None` otherwise.
    pub fn validate_alias(&self) -> Option<String> {
        self.alias.as_deref().and_then(validate_alias_str)
    }

    /// Validate the optional dye suite against rule-local checks + this
    /// plugin's declared capabilities and key sources. `UpstreamResponseEntry`
    /// is a response-stage entry, so request-direction rules are rejected.
    pub fn validate_dye(&self) -> Option<String> {
        let dye = self.dye.as_ref()?;
        let keys = self.plugin.key_header_names();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        dye.validation_error(false, self.plugin.dye_capabilities(), &key_refs)
    }

    /// Check if this plugin is enabled
    pub fn is_enabled(&self) -> bool {
        self.enable
    }

    /// Get the conditions for runtime evaluation
    pub fn conditions(&self) -> Option<&PluginConditions> {
        self.conditions.as_ref()
    }

    /// Get the plugin type name
    pub fn type_name(&self) -> &'static str {
        self.plugin.type_name()
    }
}

/// Within one EdgionPlugins resource, a failure rule's response name must not
/// equal (case-insensitively) any success rule's response name in any entry:
/// success renders with Set (replace-all) semantics and would silently wipe
/// appended failure values queued earlier. Cross-resource composition cannot
/// be checked statically — user docs instruct dedicated failure names.
pub fn dye_set_clobber_error<'a>(suites: impl Iterator<Item = Option<&'a DyeSuite>>) -> Option<String> {
    let mut success_names: Vec<String> = Vec::new();
    let mut failure_names: Vec<String> = Vec::new();
    for suite in suites.flatten() {
        for rule in &suite.response {
            let bucket = if rule.on.iter().any(DyeOutcome::is_failure) {
                &mut failure_names
            } else {
                &mut success_names
            };
            bucket.push(rule.name.to_ascii_lowercase());
        }
    }
    for f in &failure_names {
        if success_names.iter().any(|s| s == f) {
            return Some(format!(
                "dye: response header '{}' is used by both a success rule (Set \
                 semantics) and a failure rule (Append semantics) in this resource; \
                 the Set would wipe appended failure values — use distinct names",
                f
            ));
        }
    }
    None
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
        use crate::resources::edgion_plugins::EdgionPlugin;
        use crate::resources::plugin_configs::CorsConfig;

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
        use crate::resources::edgion_plugins::EdgionPlugin;
        use crate::resources::plugin_configs::CorsConfig;

        let mut entry = PluginEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
        assert_eq!(entry.validate_alias(), None);

        entry.alias = Some("bad/alias".to_string());
        let err = entry.validate_alias().expect("must reject");
        assert!(err.contains("[a-zA-Z0-9._-]"), "{err}");

        entry.alias = Some("ok-name_v1.2".to_string());
        assert_eq!(entry.validate_alias(), None);
    }
}

#[cfg(test)]
mod dye_entry_tests {
    use super::*;
    use crate::resources::edgion_plugins::dye::{DyeOutcome, DyeRule, DyeSuite};
    use crate::resources::edgion_plugins::EdgionPlugin;
    use crate::resources::plugin_configs::CorsConfig;

    fn dye_response(name: &str, on: &[DyeOutcome]) -> DyeSuite {
        DyeSuite {
            request: vec![],
            response: vec![DyeRule {
                name: name.to_string(),
                on: on.to_vec(),
                value: None,
                conditions: None,
            }],
        }
    }

    #[test]
    fn entry_dye_serde_roundtrip() {
        let yaml = "type: Cors\nconfig: {}\ndye:\n  response:\n    - name: X-O\n      on: [success]\n";
        let entry: RequestFilterEntry = serde_yaml::from_str(yaml).unwrap();
        assert!(entry.dye.is_some());
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["dye"]["response"][0]["name"], "X-O");
        // absent stays absent
        let e2 = RequestFilterEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
        assert!(serde_json::to_value(&e2).unwrap().get("dye").is_none());
    }

    #[test]
    fn validate_dye_uses_plugin_capabilities() {
        let mut entry = RequestFilterEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
        entry.dye = Some(dye_response("X-O", &[DyeOutcome::FailOpen]));
        let err = entry.validate_dye().expect("cors records nothing");
        assert!(err.contains("never records"), "{err}");
        assert!(RequestFilterEntry::new(EdgionPlugin::Cors(CorsConfig::default()))
            .validate_dye()
            .is_none());
    }

    // `CanaryConfig` has no `Default` impl (its fields are all operator-required),
    // so build a minimal valid instance directly rather than relying on `::default()`.
    fn canary_config() -> crate::resources::plugin_configs::CanaryConfig {
        use crate::resources::plugin_configs::{CanaryConfig, DynUpstreamTarget};
        use std::collections::HashMap;
        CanaryConfig {
            enable: true,
            active_profile: "stable".to_string(),
            profiles: HashMap::from([(
                "stable".to_string(),
                DynUpstreamTarget {
                    name: "app-v1".to_string(),
                    namespace: None,
                },
            )]),
            active_profile_ref: None,
            on_invalid: Default::default(),
        }
    }

    #[test]
    fn set_clobber_collision_across_entries_rejected() {
        // entry A: success dye on X-O; entry B: failure dye on x-o (case-insensitive hit)
        let mut a = RequestFilterEntry::new(EdgionPlugin::Canary(canary_config()));
        a.dye = Some(dye_response("X-O", &[DyeOutcome::Success]));
        let mut b = RequestFilterEntry::new(EdgionPlugin::Canary(canary_config()));
        b.dye = Some(dye_response("x-o", &[DyeOutcome::Fallback]));
        let err = dye_set_clobber_error([&a, &b].iter().map(|e| e.dye.as_ref())).expect("clobber must be rejected");
        assert!(err.contains("x-o") || err.contains("X-O"), "{err}");
    }
}
