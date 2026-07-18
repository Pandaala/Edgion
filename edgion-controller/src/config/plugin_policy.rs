//! Plugin policy configuration: schema, fail-fast startup validation, and the
//! compiled lookup used by the preparse stamping handlers.
//! See docs/superpowers/specs/2026-07-17-controller-plugin-policy-design.md.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use edgion_resources::resources::plugin_policy::{
    canonical_type_names, parse_qualified_name, PluginFamily, PluginPolicyAction,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyMode {
    Denylist,
    Allowlist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyActionKind {
    Block,
    Bypass,
}

/// Operator-facing `validation.plugin_policy` section. Absent = allow all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPolicyConfig {
    pub mode: PolicyMode,
    #[serde(default = "default_action")]
    pub default_action: PolicyActionKind,
    #[serde(default = "default_block_status")]
    pub default_block_status: u16,
    #[serde(default)]
    pub plugins: Vec<PluginPolicyEntryConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPolicyEntryConfig {
    /// Qualified name: `http/Wasm`, `stream/ConnectionRateLimit`, `tls-route/IpRestriction`.
    pub name: String,
    /// Per-entry action override (denylist mode only).
    #[serde(default)]
    pub action: Option<PolicyActionKind>,
    /// Per-entry block status override (denylist mode only).
    #[serde(default)]
    pub block_status: Option<u16>,
}

fn default_action() -> PolicyActionKind {
    PolicyActionKind::Block
}

fn default_block_status() -> u16 {
    503
}

/// Startup-compiled policy: one lookup per (family, type) at preparse time.
#[derive(Debug)]
pub struct CompiledPluginPolicy {
    mode: PolicyMode,
    default_action: PluginPolicyAction,
    /// denylist: name -> action for listed entries.
    /// allowlist: name -> () membership (value unused; kept as the same map
    /// type for simplicity, always the default action).
    listed: HashMap<(PluginFamily, String), PluginPolicyAction>,
}

impl CompiledPluginPolicy {
    /// `Some(action)` = restricted (stamp it); `None` = plugin may run.
    pub fn action_for(&self, family: PluginFamily, type_name: &str) -> Option<PluginPolicyAction> {
        let key = (family, type_name.to_string());
        match self.mode {
            PolicyMode::Denylist => self.listed.get(&key).cloned(),
            PolicyMode::Allowlist => {
                if self.listed.contains_key(&key) {
                    None
                } else {
                    Some(self.default_action.clone())
                }
            }
        }
    }
}

fn to_action(kind: PolicyActionKind, status: u16) -> PluginPolicyAction {
    match kind {
        PolicyActionKind::Block => PluginPolicyAction::Block { status },
        PolicyActionKind::Bypass => PluginPolicyAction::Bypass,
    }
}

fn check_status(status: u16, what: &str) -> Result<()> {
    if !(400..=599).contains(&status) {
        bail!("plugin_policy: {what} must be in 400..=599, got {status}");
    }
    Ok(())
}

/// Validate and compile the policy. All errors abort controller startup.
pub fn compile(cfg: &PluginPolicyConfig) -> Result<CompiledPluginPolicy> {
    check_status(cfg.default_block_status, "default_block_status")?;

    // schema_for! extraction runs once here, never per preparse.
    let canon: HashMap<PluginFamily, std::collections::BTreeSet<String>> =
        [PluginFamily::Http, PluginFamily::Stream, PluginFamily::TlsRoute]
            .into_iter()
            .map(|f| (f, canonical_type_names(f)))
            .collect();

    let mut listed = HashMap::new();
    for entry in &cfg.plugins {
        let (family, type_name) =
            parse_qualified_name(&entry.name).map_err(|e| anyhow::anyhow!("plugin_policy: {e}"))?;
        if !canon[&family].contains(&type_name) {
            bail!(
                "plugin_policy: unknown plugin {:?}; valid {} types: {:?}",
                entry.name,
                family.as_str(),
                canon[&family]
            );
        }
        if cfg.mode == PolicyMode::Allowlist && (entry.action.is_some() || entry.block_status.is_some()) {
            bail!(
                "plugin_policy: allowlist entry {:?} must not set action/block_status \
                 (listed entries are allowed; the fields would be meaningless)",
                entry.name
            );
        }
        if let Some(s) = entry.block_status {
            check_status(s, &format!("plugins[{}].block_status", entry.name))?;
        }
        let action = to_action(
            entry.action.unwrap_or(cfg.default_action),
            entry.block_status.unwrap_or(cfg.default_block_status),
        );
        if listed.insert((family, type_name), action).is_some() {
            bail!("plugin_policy: duplicate entry {:?}", entry.name);
        }
    }

    Ok(CompiledPluginPolicy {
        mode: cfg.mode,
        default_action: to_action(cfg.default_action, cfg.default_block_status),
        listed,
    })
}

static COMPILED_POLICY: OnceLock<CompiledPluginPolicy> = OnceLock::new();

/// Compile and install the policy at startup. Call once, right after
/// `init_controller_config`; a `None` config is a no-op (allow all).
pub fn init_compiled_plugin_policy(cfg: Option<&PluginPolicyConfig>) -> Result<()> {
    if let Some(cfg) = cfg {
        let compiled = compile(cfg)?;
        let _ = COMPILED_POLICY.set(compiled);
    }
    Ok(())
}

/// The active compiled policy; `None` = no policy configured (allow all).
pub fn compiled_plugin_policy() -> Option<&'static CompiledPluginPolicy> {
    COMPILED_POLICY.get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::plugin_policy::{PluginFamily, PluginPolicyAction};

    fn cfg(yaml: &str) -> PluginPolicyConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn denylist_compiles_and_evaluates() {
        let p = compile(&cfg("mode: denylist\nplugins:\n  - name: http/Wasm\n  - name: http/Dsl\n    action: block\n    block_status: 403\n  - name: stream/ConnectionRateLimit\n    action: bypass\n")).unwrap();
        assert_eq!(
            p.action_for(PluginFamily::Http, "Wasm"),
            Some(PluginPolicyAction::Block { status: 503 }) // default_action + default status
        );
        assert_eq!(
            p.action_for(PluginFamily::Http, "Dsl"),
            Some(PluginPolicyAction::Block { status: 403 })
        );
        assert_eq!(
            p.action_for(PluginFamily::Stream, "ConnectionRateLimit"),
            Some(PluginPolicyAction::Bypass)
        );
        assert_eq!(p.action_for(PluginFamily::Http, "Cors"), None); // unlisted runs
    }

    #[test]
    fn allowlist_is_global_across_families() {
        let p = compile(&cfg(
            "mode: allowlist\ndefault_action: bypass\nplugins:\n  - name: http/Cors\n",
        ))
        .unwrap();
        assert_eq!(p.action_for(PluginFamily::Http, "Cors"), None); // listed = allowed
        assert_eq!(
            p.action_for(PluginFamily::Http, "Wasm"),
            Some(PluginPolicyAction::Bypass)
        );
        // Global semantics: stream plugins are restricted too even though
        // only http/ entries are listed.
        assert_eq!(
            p.action_for(PluginFamily::Stream, "ConnectionRateLimit"),
            Some(PluginPolicyAction::Bypass)
        );
    }

    #[test]
    fn fail_fast_cases() {
        // unknown family
        assert!(compile(&cfg("mode: denylist\nplugins:\n  - name: grpc/Wasm\n")).is_err());
        // unknown plugin name
        assert!(compile(&cfg("mode: denylist\nplugins:\n  - name: http/NoSuchPlugin\n")).is_err());
        // duplicate entry
        assert!(compile(&cfg(
            "mode: denylist\nplugins:\n  - name: http/Wasm\n  - name: http/Wasm\n"
        ))
        .is_err());
        // block_status out of range (per-entry and default)
        assert!(compile(&cfg(
            "mode: denylist\nplugins:\n  - name: http/Wasm\n    block_status: 200\n"
        ))
        .is_err());
        assert!(compile(&cfg(
            "mode: denylist\ndefault_block_status: 302\nplugins:\n  - name: http/Wasm\n"
        ))
        .is_err());
        // allowlist entry with action/block_status is meaningless -> error
        assert!(compile(&cfg(
            "mode: allowlist\nplugins:\n  - name: http/Cors\n    action: block\n"
        ))
        .is_err());
        assert!(compile(&cfg(
            "mode: allowlist\nplugins:\n  - name: http/Cors\n    block_status: 403\n"
        ))
        .is_err());
    }
}
