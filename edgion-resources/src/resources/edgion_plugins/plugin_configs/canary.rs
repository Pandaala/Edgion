//! Canary plugin configuration.
//!
//! Sends 100% of a route's traffic to one named backend at a time (the
//! "active profile"), switchable at runtime via a generic `Selector`
//! EdgionConfigData referenced by `active_profile_ref`. This is the
//! full-cutover / blue-green counterpart to `backendRefs[].weight`'s
//! percentage-based split. See
//! `docs/superpowers/specs/2026-07-07-canary-plugin-design.md` for the full
//! design rationale.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::dynamic_internal_upstream::DynUpstreamTarget;
use super::upstream_fallback::{default_upstream_reject, UpstreamFallback};
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::PluginValidation;

/// Canary plugin configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CanaryConfig {
    /// Kill switch. `false` => plugin is a no-op; the route's own weighted
    /// `backendRefs` selection applies unmodified.
    pub enable: bool,

    /// Base/default profile key; must exist in `profiles`. Used whenever no
    /// `active_profile_ref` is configured, and as the fail-to-base target on
    /// any selector resolution error.
    pub active_profile: String,

    /// Named profiles: profile name -> target backendRef. Git-owned, always
    /// safe to fall back to.
    pub profiles: HashMap<String, DynUpstreamTarget>,

    /// Optional Selector EdgionConfigData supplying the active profile name
    /// at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_ref: Option<EdgionConfigDataRef>,

    /// Behavior when the resolved profile's backendRef is not usable: not
    /// present in the route's `backendRefs` (stale/misconfigured profile), or
    /// present but denied by ReferenceGrant.
    /// - `Reject` (default): 403.
    /// - `Fallback`: fall through to the route's own weighted `backendRefs`
    ///   selection.
    #[serde(default = "default_upstream_reject")]
    pub on_invalid: UpstreamFallback,
}

impl CanaryConfig {
    /// Stateless validation entry point (no compile step needed — Canary
    /// has no regex/matchers to compile, unlike DynamicInternalUpstream).
    pub fn check_fields(&self) -> Option<String> {
        if self.profiles.is_empty() {
            return Some("canary: at least one profile is required".to_string());
        }
        if !self.profiles.contains_key(&self.active_profile) {
            return Some(format!(
                "canary: activeProfile '{}' not found in profiles",
                self.active_profile
            ));
        }
        None
    }

    /// Resolve a profile by optional selector name, returning `None` on an
    /// unknown name so the caller can apply fail-to-base. `None` selector
    /// => the base `active_profile`.
    pub fn resolve_profile(&self, selected: Option<&str>) -> Option<&DynUpstreamTarget> {
        match selected {
            Some(name) => self.profiles.get(name),
            None => self.profiles.get(&self.active_profile),
        }
    }
}

impl PluginValidation for CanaryConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/canary_test.rs"]
mod tests;
