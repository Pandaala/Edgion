//! Plugin entry types with enable/disable functionality

use super::body_requirement::BodyRequirement;
use super::conditions::PluginConditions;
use super::dye::{DyeOutcome, DyeStage, DyeSuite};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::edgion_plugin::{EdgionPlugin, InvalidPluginConfig};

/// Decode the flattened plugin while the enclosing stage entry handles its
/// controls. Only a string-tagged envelope can become a configuration-error
/// sentinel. Replay preserves duplicate keys for the typed decoder.
fn deserialize_entry_plugin<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<EdgionPlugin, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    #[expect(
        clippy::large_enum_variant,
        reason = "return the typed plugin without allocating a temporary box"
    )]
    enum Wire {
        // Serde also accepts a singleton map as an enum discriminator. The
        // plugin API requires a string even if the map names a real plugin.
        ObjectTag {
            #[serde(rename = "type")]
            _kind: ObjectTag,
        },
        Typed(EdgionPlugin),
        Invalid {
            #[serde(rename = "type")]
            kind: String,
            #[serde(rename = "config", default)]
            _config: Option<serde::de::IgnoredAny>,
        },
    }

    #[derive(Deserialize)]
    struct ObjectTag {}

    match Wire::deserialize(deserializer)? {
        Wire::ObjectTag { .. } => Err(serde::de::Error::custom("plugin type must be a string")),
        Wire::Typed(plugin) => Ok(plugin),
        Wire::Invalid { kind, .. } => Ok(EdgionPlugin::InvalidConfig(InvalidPluginConfig::from_type_name(&kind))),
    }
}

/// Helper functions for serde defaults
pub(super) fn default_true() -> bool {
    true
}

pub(super) fn is_true(v: &bool) -> bool {
    *v
}

/// Record a valid alias and report it if an earlier entry in the same stage
/// array already claimed it.
///
/// `seen` is the caller's per-stage-array accumulator; `alias` should already
/// have passed [`validate_alias_str`] (an invalid alias is dropped, not
/// deduplicated). Entries without an alias never collide.
///
/// Uniqueness matters beyond the access log: the alias is the discriminator in
/// a plugin entry's runtime identity (`<namespace>/<name>/<alias or #index>`),
/// which `RateLimitRedis` uses to namespace its Redis buckets. Two entries
/// sharing an alias would silently share one bucket namespace — and one
/// access-log label. Callers therefore report the duplicate AND drop it, the way
/// an invalid alias is dropped: reporting alone would not protect the data plane,
/// since lenient admission keeps an `Accepted=False` resource synced and serving.
pub fn duplicate_alias_error(seen: &mut std::collections::HashSet<String>, alias: Option<&str>) -> Option<String> {
    let alias = alias?;
    if seen.insert(alias.to_string()) {
        return None;
    }
    Some(format!(
        "duplicate alias '{alias}': aliases must be unique within a stage"
    ))
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
/// Named groups and CIDRs belong to the referenced IpList.
///
/// YAML format:
/// ```yaml
/// requestPlugins:
///   - enable: true                    # optional, defaults to true
///     type: RequestHeaderModifier
///     config:
///       set:
///         - name: X-Test
///           value: test-value
///   - conditions:                     # conditional execution
///       skip:
///         allOf:
///           - type: keyExist
///             key:
///               type: header
///               name: "X-Internal"
///       run:
///         allOf:
///           - type: timeRange
///             after: "2024-01-01T00:00:00Z"
///             before: "2024-12-31T23:59:59Z"
///     type: IpRestriction
///     config:
///       allowRefs: [{name: internal-network}]
///       defaultAction: deny
/// upstreamResponseFilterPlugins:
///   - enable: false                   # disabled plugin
///     type: ResponseHeaderModifier
///     config:
///       add:
///         - name: X-Response
///           value: added
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
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$` and be unique
    /// within its stage array. Lets multiple entries of the same plugin type be
    /// distinguished in the access log; it is also the discriminator in the
    /// entry's runtime identity, which namespaces state a plugin keeps outside
    /// its own instance (RateLimitRedis buckets), so a duplicate is reported and
    /// dropped rather than allowed to merge two entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and `skills/01-architecture/02-gateway/entry-level-dye.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

    /// The actual plugin configuration
    #[serde(flatten, deserialize_with = "deserialize_entry_plugin")]
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
        dye.validation_error(true, self.plugin.dye_capabilities(DyeStage::Request), &key_refs)
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
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$` and be unique
    /// within its stage array. Lets multiple entries of the same plugin type be
    /// distinguished in the access log; it is also the discriminator in the
    /// entry's runtime identity, which namespaces state a plugin keeps outside
    /// its own instance (RateLimitRedis buckets), so a duplicate is reported and
    /// dropped rather than allowed to merge two entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// Optional body requirement. When set (by the operator) or declared
    /// intrinsically by the plugin, the runtime wraps the plugin in a body gate
    /// that buffers the request body (OpenResty-style, at the request phase) and
    /// applies the configured fail-open / fail-close policy on a read failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyRequirement>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and `skills/01-architecture/02-gateway/entry-level-dye.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

    /// The actual plugin configuration
    #[serde(flatten, deserialize_with = "deserialize_entry_plugin")]
    pub plugin: EdgionPlugin,
}

impl RequestFilterEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            conditions: None,
            alias: None,
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
        dye.validation_error(true, self.plugin.dye_capabilities(DyeStage::Request), &key_refs)
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
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$` and be unique
    /// within its stage array. Lets multiple entries of the same plugin type be
    /// distinguished in the access log; it is also the discriminator in the
    /// entry's runtime identity, which namespaces state a plugin keeps outside
    /// its own instance (RateLimitRedis buckets), so a duplicate is reported and
    /// dropped rather than allowed to merge two entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and `skills/01-architecture/02-gateway/entry-level-dye.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

    /// The actual plugin configuration
    #[serde(flatten, deserialize_with = "deserialize_entry_plugin")]
    pub plugin: EdgionPlugin,
}

impl UpstreamResponseFilterEntry {
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
    /// plugin's declared capabilities and key sources. `UpstreamResponseFilterEntry`
    /// is a response-stage entry, so request-direction rules are rejected.
    pub fn validate_dye(&self) -> Option<String> {
        let dye = self.dye.as_ref()?;
        let keys = self.plugin.key_header_names();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        dye.validation_error(
            false,
            self.plugin.dye_capabilities(DyeStage::UpstreamResponseFilter),
            &key_refs,
        )
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
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$` and be unique
    /// within its stage array. Lets multiple entries of the same plugin type be
    /// distinguished in the access log; it is also the discriminator in the
    /// entry's runtime identity, which namespaces state a plugin keeps outside
    /// its own instance (RateLimitRedis buckets), so a duplicate is reported and
    /// dropped rather than allowed to merge two entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and `skills/01-architecture/02-gateway/entry-level-dye.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

    /// The actual plugin configuration
    #[serde(flatten, deserialize_with = "deserialize_entry_plugin")]
    pub plugin: EdgionPlugin,
}

impl UpstreamResponseBodyFilterEntry {
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
    /// plugin's declared capabilities and key sources. `UpstreamResponseBodyFilterEntry`
    /// is a response-stage entry, so request-direction rules are rejected.
    pub fn validate_dye(&self) -> Option<String> {
        let dye = self.dye.as_ref()?;
        let keys = self.plugin.key_header_names();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        dye.validation_error(
            false,
            self.plugin.dye_capabilities(DyeStage::UpstreamResponseBodyFilter),
            &key_refs,
        )
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
    /// Optional; when set, must match `^[a-zA-Z0-9._-]{1,32}$` and be unique
    /// within its stage array. Lets multiple entries of the same plugin type be
    /// distinguished in the access log; it is also the discriminator in the
    /// entry's runtime identity, which namespaces state a plugin keeps outside
    /// its own instance (RateLimitRedis buckets), so a duplicate is reported and
    /// dropped rather than allowed to merge two entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    /// Reserved body requirement. Proxied upstream responses are
    /// streaming-only, so validation rejects this field for every currently
    /// supported response-stage plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyRequirement>,

    /// Entry-level dye suite (traffic tagging on outcomes). See
    /// `edgion_plugins/dye.rs` and `skills/01-architecture/02-gateway/entry-level-dye.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeSuite>,

    /// The actual plugin configuration
    #[serde(flatten, deserialize_with = "deserialize_entry_plugin")]
    pub plugin: EdgionPlugin,
}

impl UpstreamResponseEntry {
    /// Create a new enabled plugin entry
    pub fn new(plugin: EdgionPlugin) -> Self {
        Self {
            enable: true,
            conditions: None,
            alias: None,
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
        dye.validation_error(
            false,
            self.plugin.dye_capabilities(DyeStage::UpstreamResponse),
            &key_refs,
        )
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
#[path = "../../../tests/unit/resources/edgion_plugins/entry_test/alias_validation_tests.rs"]
mod alias_validation_tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/entry_test/dye_entry_tests.rs"]
mod dye_entry_tests;
