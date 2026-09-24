//! Shared GatewayClass plugin-policy schema, pure compiler, execution actions,
//! and canonical plugin-name helpers.
//!
//! The Gateway compiles EdgionGatewayConfig policy into scoped execution views.
//! Plugin actions are runtime decisions, never serialized plugin-entry stamps.

use std::collections::{BTreeSet, HashMap};

use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::DefaultAction;

/// Execution action selected by the Gateway's effective scoped policy.
/// The gateway enforces it; the plugin itself is never instantiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum PluginPolicyAction {
    /// Reject requests on chains containing this entry. `status` is the HTTP
    /// response code; stream stages ignore it and close the connection.
    Block { status: u16 },
    /// Skip the plugin; requests proceed and the skip is logged.
    Bypass,
}

/// GatewayClass-scoped plugin execution policy in `EdgionGatewayConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPolicyConfig {
    /// Exact qualified names allowed to execute. Presence enables allow-list
    /// mode, including deny-all when the list is explicitly empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    /// Explicit restrictions, taking precedence over the allow list.
    #[serde(default)]
    pub deny: Vec<PluginPolicyEntryConfig>,
    /// Action for unmatched names when no allow list is configured.
    #[serde(default)]
    pub default_action: DefaultAction,
    /// Execution behavior for restricted plugins, unless an entry overrides it.
    #[serde(default)]
    pub denied_action: PolicyActionKind,
    /// Default HTTP rejection status for block actions. Stream stages ignore it.
    #[serde(default = "default_block_status")]
    #[schemars(range(min = 400, max = 599))]
    pub block_status: u16,
}

impl Default for PluginPolicyConfig {
    fn default() -> Self {
        Self {
            allow: None,
            deny: Vec::new(),
            default_action: DefaultAction::Allow,
            denied_action: PolicyActionKind::Block,
            block_status: default_block_status(),
        }
    }
}

/// One exact-name restriction and its optional execution overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPolicyEntryConfig {
    /// Case-sensitive `<family>/<Type>` from the canonical plugin catalog.
    pub name: String,
    /// Overrides the policy's `deniedAction` for this name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<PolicyActionKind>,
    /// Overrides the policy's HTTP status; forbidden when the effective action
    /// is bypass, including when bypass is inherited from `deniedAction`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 400, max = 599))]
    pub block_status: Option<u16>,
}

/// Execution control, independent of runtime-dependency failure policies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PolicyActionKind {
    #[default]
    Block,
    Bypass,
}

impl PolicyActionKind {
    fn resolve(self, status: u16) -> PluginPolicyAction {
        match self {
            Self::Block => PluginPolicyAction::Block { status },
            Self::Bypass => PluginPolicyAction::Bypass,
        }
    }
}

fn default_block_status() -> u16 {
    503
}

/// Immutable, config-independent lookup. Compilation owns all name parsing and
/// validation; querying borrows the type name without allocating.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompiledPluginPolicy {
    default_action: Option<PluginPolicyAction>,
    listed: [HashMap<String, Option<PluginPolicyAction>>; 3],
}

impl CompiledPluginPolicy {
    /// `None` permits execution; `Some` restricts it. Unknown runtime names
    /// retain the configured fallback, including deny-all in allow-list mode.
    pub fn action_for(&self, family: PluginFamily, type_name: &str) -> Option<PluginPolicyAction> {
        self.listed[family.index()]
            .get(type_name)
            .copied()
            .unwrap_or(self.default_action)
    }
}

impl PluginPolicyConfig {
    /// Validate every entry and compile one complete policy. An error never
    /// returns a partially permissive lookup. Both binaries use this function.
    pub fn compile(&self) -> Result<CompiledPluginPolicy, String> {
        check_block_status(self.block_status, "pluginPolicy.blockStatus")?;
        let canon = [PluginFamily::Http, PluginFamily::Stream, PluginFamily::TlsRoute].map(canonical_type_names);
        let parse_name = |name: &str| -> Result<(PluginFamily, String), String> {
            let (family, type_name) = parse_qualified_name(name).map_err(|e| format!("pluginPolicy: {e}"))?;
            if !canon[family.index()].contains(&type_name) {
                return Err(format!("pluginPolicy: unknown plugin {name:?}"));
            }
            Ok((family, type_name))
        };
        let restricted = self.denied_action.resolve(self.block_status);
        let mut compiled = CompiledPluginPolicy {
            default_action: (self.allow.is_some() || self.default_action == DefaultAction::Deny).then_some(restricted),
            ..Default::default()
        };
        if let Some(allow) = &self.allow {
            for name in allow {
                let (family, type_name) = parse_name(name)?;
                if compiled.listed[family.index()].insert(type_name, None).is_some() {
                    return Err(format!("pluginPolicy.allow: duplicate plugin {name:?}"));
                }
            }
        }
        let mut denied = BTreeSet::new();
        for entry in &self.deny {
            let (family, type_name) = parse_name(&entry.name)?;
            if !denied.insert(entry.name.as_str()) {
                return Err(format!("pluginPolicy.deny: duplicate plugin {:?}", entry.name));
            }
            let kind = entry.action.unwrap_or(self.denied_action);
            if let Some(status) = entry.block_status {
                check_block_status(status, "pluginPolicy.deny[].blockStatus")?;
                if kind == PolicyActionKind::Bypass {
                    return Err("pluginPolicy.deny[].blockStatus must not be set for bypass".to_string());
                }
            }
            compiled.listed[family.index()].insert(
                type_name,
                Some(kind.resolve(entry.block_status.unwrap_or(self.block_status))),
            );
        }
        Ok(compiled)
    }
}

fn check_block_status(status: u16, field: &str) -> Result<(), String> {
    if !(400..=599).contains(&status) {
        return Err(format!("{field} must be between 400 and 599"));
    }
    Ok(())
}

/// The three plugin families addressable by a policy qualified name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginFamily {
    Http,
    Stream,
    TlsRoute,
}

impl PluginFamily {
    fn index(self) -> usize {
        match self {
            Self::Http => 0,
            Self::Stream => 1,
            Self::TlsRoute => 2,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PluginFamily::Http => "http",
            PluginFamily::Stream => "stream",
            PluginFamily::TlsRoute => "tls-route",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "http" => Some(PluginFamily::Http),
            "stream" => Some(PluginFamily::Stream),
            "tls-route" => Some(PluginFamily::TlsRoute),
            _ => None,
        }
    }
}

/// Parse a `<family>/<Type>` qualified plugin name. Exact, case-sensitive,
/// no wildcards.
pub fn parse_qualified_name(s: &str) -> Result<(PluginFamily, String), String> {
    let (family_str, type_name) = s
        .split_once('/')
        .ok_or_else(|| format!("invalid qualified plugin name {s:?}: expected <family>/<Type>"))?;
    let family = PluginFamily::parse(family_str)
        .ok_or_else(|| format!("invalid plugin family {family_str:?} in {s:?}: expected http, stream, or tls-route"))?;
    if type_name.is_empty() || type_name.contains('/') {
        return Err(format!("invalid plugin type in qualified name {s:?}"));
    }
    Ok((family, type_name.to_string()))
}

/// Extract the serde tag values (`properties.type.const` of each `oneOf`
/// branch) from a tagged-enum JSON schema. Same technique as
/// `edgion-resources/tests/plugin_crd_type_coverage.rs`.
fn tag_values(schema: serde_json::Value) -> BTreeSet<String> {
    schema
        .get("oneOf")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| {
            b.get("properties")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.get("const"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// Canonical set of `type:` discriminator values for one plugin family,
/// derived from the enum's own schema — no hand-maintained list, no drift.
/// Cost is one `schema_for!` walk; use only during configuration compilation,
/// never from a request or connection callback.
pub fn canonical_type_names(family: PluginFamily) -> BTreeSet<String> {
    let schema = match family {
        PluginFamily::Http => serde_json::to_value(schema_for!(crate::resources::edgion_plugins::EdgionPlugin)),
        PluginFamily::Stream => {
            serde_json::to_value(schema_for!(crate::resources::edgion_stream_plugins::EdgionStreamPlugin))
        }
        PluginFamily::TlsRoute => serde_json::to_value(schema_for!(
            crate::resources::edgion_stream_plugins::TlsRouteStreamPlugin
        )),
    }
    .expect("plugin enum schema serializes");
    let names = tag_values(schema);
    assert!(
        !names.is_empty(),
        "extracted zero type names for family {}; schemars output shape changed",
        family.as_str()
    );
    names
}

#[cfg(test)]
#[path = "../../tests/unit/resources/plugin_policy_test.rs"]
mod tests;
