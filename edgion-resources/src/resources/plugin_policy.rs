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
mod tests {
    use super::*;
    use crate::resources::edgion_plugins::RequestFilterEntry;

    fn compile_yaml(yaml: &str) -> Result<CompiledPluginPolicy, String> {
        serde_yaml::from_str::<PluginPolicyConfig>(yaml).unwrap().compile()
    }

    #[test]
    fn gateway_policy_defaults_and_explicit_empty_allow_differ() {
        let absent: crate::EdgionGatewayConfigSpec = serde_yaml::from_str("{}").unwrap();
        assert!(absent.plugin_policy.is_none());
        assert!(absent.validate_plugin_policy().is_empty());
        assert_eq!(compile_yaml("{}").unwrap(), CompiledPluginPolicy::default());
        let empty_allow = compile_yaml("allow: []\ndefaultAction: allow").unwrap();
        for (family, name) in [
            (PluginFamily::Http, "Wasm"),
            (PluginFamily::Stream, "ConnectionRateLimit"),
            (PluginFamily::TlsRoute, "IpRestriction"),
        ] {
            assert_eq!(CompiledPluginPolicy::default().action_for(family, name), None);
            assert_eq!(
                empty_allow.action_for(family, name),
                Some(PluginPolicyAction::Block { status: 503 })
            );
        }
        assert_eq!(
            empty_allow.action_for(PluginFamily::Http, "FutureType"),
            Some(PluginPolicyAction::Block { status: 503 })
        );
        let roundtrip: PluginPolicyConfig = serde_yaml::from_str("allow: []").unwrap();
        assert_eq!(serde_json::to_value(roundtrip).unwrap()["allow"], serde_json::json!([]));
        assert!(serde_json::to_value(PluginPolicyConfig::default())
            .unwrap()
            .get("allow")
            .is_none());
    }

    #[test]
    fn gateway_policy_deny_overrides_allow_and_allow_overrides_default_deny() {
        let policy = compile_yaml(
            "allow: [http/Wasm, http/Dsl]\ndefaultAction: deny\ndeny:\n  - name: http/Wasm\n    blockStatus: 451",
        )
        .unwrap();
        assert_eq!(
            policy.action_for(PluginFamily::Http, "Wasm"),
            Some(PluginPolicyAction::Block { status: 451 })
        );
        assert_eq!(policy.action_for(PluginFamily::Http, "Dsl"), None);
        assert_eq!(
            policy.action_for(PluginFamily::Http, "Mock"),
            Some(PluginPolicyAction::Block { status: 503 })
        );
        let default_deny = compile_yaml("defaultAction: deny\ndeniedAction: bypass").unwrap();
        assert_eq!(
            default_deny.action_for(PluginFamily::Http, "Mock"),
            Some(PluginPolicyAction::Bypass)
        );
    }

    #[test]
    fn gateway_policy_inherits_status_and_actions_across_families() {
        let policy = compile_yaml("blockStatus: 451\ndeny:\n  - name: http/Wasm\n  - name: stream/ConnectionRateLimit\n    action: bypass\n  - name: tls-route/IpRestriction\n    blockStatus: 403").unwrap();
        assert_eq!(
            policy.action_for(PluginFamily::Http, "Wasm"),
            Some(PluginPolicyAction::Block { status: 451 })
        );
        assert_eq!(
            policy.action_for(PluginFamily::Stream, "ConnectionRateLimit"),
            Some(PluginPolicyAction::Bypass)
        );
        assert_eq!(
            policy.action_for(PluginFamily::TlsRoute, "IpRestriction"),
            Some(PluginPolicyAction::Block { status: 403 })
        );
        assert_eq!(policy.action_for(PluginFamily::Http, "IpRestriction"), None);

        let bypass = compile_yaml(
            "deniedAction: bypass\nblockStatus: 451\ndeny:\n  - name: http/Wasm\n  - name: http/Dsl\n    action: block",
        )
        .unwrap();
        assert_eq!(
            bypass.action_for(PluginFamily::Http, "Wasm"),
            Some(PluginPolicyAction::Bypass)
        );
        assert_eq!(
            bypass.action_for(PluginFamily::Http, "Dsl"),
            Some(PluginPolicyAction::Block { status: 451 })
        );
    }

    #[test]
    fn gateway_policy_rejects_invalid_status_and_bypass_status_override() {
        for status in [400, 599] {
            assert!(compile_yaml(&format!(
                "blockStatus: {status}\ndeny:\n  - name: http/Wasm\n    blockStatus: {status}"
            ))
            .is_ok());
        }
        for status in [0, 399, 600, u16::MAX] {
            assert!(compile_yaml(&format!("blockStatus: {status}")).is_err());
            assert!(compile_yaml(&format!("deny:\n  - name: http/Wasm\n    blockStatus: {status}")).is_err());
        }
        for yaml in [
            "deny:\n  - name: http/Wasm\n    action: bypass\n    blockStatus: 451",
            "deniedAction: bypass\ndeny:\n  - name: http/Wasm\n    blockStatus: 451",
        ] {
            assert!(compile_yaml(yaml).unwrap_err().contains("must not be set for bypass"));
        }
        assert!(compile_yaml(
            "deniedAction: bypass\ndeny:\n  - name: http/Wasm\n    action: block\n    blockStatus: 451"
        )
        .is_ok());
    }

    #[test]
    fn gateway_policy_rejects_duplicate_and_noncanonical_names() {
        for name in [
            "http/wasm",
            "grpc/Wasm",
            "http/Unknown",
            "http/*",
            "http/Waf",
            "http/Wasm/extra",
            "Wasm",
        ] {
            assert!(compile_yaml(&format!("allow: [{name}]")).is_err(), "{name}");
            assert!(compile_yaml(&format!("deny:\n  - name: {name}")).is_err(), "{name}");
        }
        for yaml in [
            "allow: [http/Wasm, http/Wasm]",
            "deny:\n  - name: http/Wasm\n  - name: http/Wasm\n    action: bypass",
        ] {
            assert!(compile_yaml(yaml).unwrap_err().contains("duplicate"));
        }
    }

    #[test]
    fn gateway_policy_serde_rejects_unknown_fields_types_and_legacy_spelling() {
        for yaml in [
            "mode: denylist",
            "default_action: deny",
            "defaultAction: Allow",
            "deniedAction: allow",
            "blockStatus: bad",
            "blockStatus: 65536",
            "allow: http/Wasm",
            "deny:\n  - name: http/Wasm\n    block_status: 451",
            "deny:\n  - name: http/Wasm\n    enable: false",
            "deny:\n  - action: block",
        ] {
            assert!(serde_yaml::from_str::<PluginPolicyConfig>(yaml).is_err(), "{yaml}");
        }
        let cfg: PluginPolicyConfig = serde_yaml::from_str("deny:\n  - name: http/Wasm").unwrap();
        let wire = serde_json::to_value(&cfg).unwrap();
        assert!(wire["deny"][0].get("action").is_none());
        assert!(wire["deny"][0].get("blockStatus").is_none());
        assert_eq!(serde_json::from_value::<PluginPolicyConfig>(wire).unwrap(), cfg);
    }

    #[test]
    fn execution_action_serde_round_trip() {
        let block = PluginPolicyAction::Block { status: 403 };
        let json = serde_json::to_string(&block).unwrap();
        assert_eq!(json, r#"{"action":"block","status":403}"#);
        let back: PluginPolicyAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, block);

        let bypass = PluginPolicyAction::Bypass;
        let json = serde_json::to_string(&bypass).unwrap();
        assert_eq!(json, r#"{"action":"bypass"}"#);
    }

    #[test]
    fn plugin_entries_do_not_carry_execution_policy() {
        let entry: RequestFilterEntry = serde_yaml::from_str("type: Mock\nconfig: {}").unwrap();
        let wire = serde_json::to_value(&entry).unwrap();
        let schema = serde_json::to_value(schemars::schema_for!(RequestFilterEntry)).unwrap();
        assert!(wire.get("policyAction").is_none());
        assert!(schema["properties"].get("policyAction").is_none());
    }

    #[test]
    fn parse_qualified_name_accepts_all_families() {
        assert_eq!(
            parse_qualified_name("http/Wasm").unwrap(),
            (PluginFamily::Http, "Wasm".to_string())
        );
        assert_eq!(
            parse_qualified_name("stream/ConnectionRateLimit").unwrap(),
            (PluginFamily::Stream, "ConnectionRateLimit".to_string())
        );
        assert_eq!(
            parse_qualified_name("tls-route/IpRestriction").unwrap(),
            (PluginFamily::TlsRoute, "IpRestriction".to_string())
        );
    }

    #[test]
    fn parse_qualified_name_rejects_bad_input() {
        for bad in ["Wasm", "http/", "/Wasm", "grpc/Wasm", "http/Wasm/extra", ""] {
            assert!(parse_qualified_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn canonical_type_names_extracts_all_three_families() {
        let http = canonical_type_names(PluginFamily::Http);
        assert!(http.contains("Wasm") && http.contains("RequestHeaderModifier"));
        assert!(http.len() >= 30, "http family too small: {}", http.len());

        let stream = canonical_type_names(PluginFamily::Stream);
        assert!(stream.contains("ConnectionRateLimit"));
        assert!(!stream.is_empty());

        let tls_route = canonical_type_names(PluginFamily::TlsRoute);
        assert!(tls_route.contains("IpRestriction"));
        assert!(!tls_route.is_empty());
    }
}
