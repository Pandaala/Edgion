//! GlobalConnectionIpRestriction — L4 TCP connection-phase IP restriction (stream plugin config).
//!
//! This is a plain, git-owned plugin config: it carries the complete IP rule data
//! (`enable` + named `profiles`) inline. The only dynamic, hot-swappable input is *which
//! profile is active*, supplied via an optional `activeProfileRef` pointing at a `Selector`
//! EdgionConfigData. The rule data itself is never overlaid from outside.
//!
//! # Active-profile resolution (gateway, per-connection)
//! - No `activeProfileRef` → use `active_profile`.
//! - Selector present with `active: Some(name)` and `name` ∈ `profiles` → use `name`.
//! - Any error / `Selector.active: None` / envelope `enable=false` → fall back to
//!   `active_profile` (fail-to-base). A selector can only switch among these git-owned
//!   profiles; it can never disable filtering.

use crate::matcher::ip_cidr_helpers::{build_grouped_matcher_partial, evaluate_ip_access, EntryError};
use crate::resources::edgion_config_data::{EdgionConfigDataRef, IpGroup};
use crate::resources::edgion_plugins::{AccessDecision, DefaultAction};
use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;

/// Complete, git-owned GlobalConnectionIpRestriction plugin config.
///
/// # Security invariant
/// This config MUST be authored as the conservative/safe default (e.g. `defaultAction: Deny`
/// plus an explicit allowlist). The active-profile selector falls back to `active_profile`
/// on any error, so a permissive default creates a wide-open fallback.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalConnectionIpRestrictionConfig {
    /// Emergency kill switch. When `false`, the data plane completely skips this plugin
    /// (allow all connections).
    pub enable: bool,

    /// Currently active profile key. Must exist in `profiles`. May be overridden at
    /// connection time by an `activeProfileRef` → `Selector` EdgionConfigData.
    pub active_profile: String,

    /// Named rule presets. Switch the active profile to swap rules instantly.
    pub profiles: HashMap<String, ProfileRules>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional reference to a `Selector` EdgionConfigData that supplies the *active
    /// profile* at connection time. Namespace defaults to the parent EdgionStreamPlugins
    /// namespace. When absent, `active_profile` is used directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_ref: Option<EdgionConfigDataRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRules {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<IpGroup>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<IpGroup>>,

    #[serde(default)]
    pub default_action: DefaultAction,

    /// Allow-list references to `IpList` EdgionConfigData resources (resolved in gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_refs: Option<Vec<EdgionConfigDataRef>>,
    /// Deny-list references to `IpList` EdgionConfigData resources (resolved in gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_refs: Option<Vec<EdgionConfigDataRef>>,

    // Runtime-only, not serialized
    #[serde(skip)]
    #[schemars(skip)]
    pub allow_matcher: Option<IpRadixMatcher>,
    #[serde(skip)]
    #[schemars(skip)]
    pub deny_matcher: Option<IpRadixMatcher>,
}

impl GlobalConnectionIpRestrictionConfig {
    /// Validate the config and pre-compile runtime matchers for every profile.
    /// Fails fast on the first invalid profile. `active_profile_ref` is not validated
    /// here (it is a controller-resolved cross-resource reference).
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        if self.profiles.is_empty() {
            return Err("GlobalConnectionIpRestriction requires at least one profile".to_string());
        }

        if !self.profiles.contains_key(&self.active_profile) {
            return Err(format!(
                "activeProfile '{}' not found in profiles {:?}",
                self.active_profile,
                self.profiles.keys().collect::<Vec<_>>()
            ));
        }

        for (profile_name, profile) in self.profiles.iter_mut() {
            profile
                .validate_and_init()
                .map_err(|e| format!("profile '{}': {}", profile_name, e))?;
        }

        Ok(())
    }
}

impl ProfileRules {
    pub(crate) fn validate_and_init(&mut self) -> Result<(), String> {
        let has_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let has_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !has_allow && !has_deny {
            return Err("at least one of 'allow'/'deny'/'allowRefs'/'denyRefs' must be non-empty".to_string());
        }

        // Intentionally stricter than HTTP/L4 IpRestriction: a present-but-empty
        // `allow`/`deny` (`Some(vec![])`) is REJECTED here (structural), preserving
        // GlobalConnection semantics — do not "harmonize" it into the `!is_empty()`
        // guard the other consumers use. A non-empty list is partial-good: bad CIDRs
        // are skipped, not fatal.
        if let Some(allow) = &self.allow {
            if allow.is_empty() {
                return Err("allow group list is empty".to_string());
            }
            let (m, _errs) = build_flat_matcher_partial(allow);
            self.allow_matcher = Some(m);
        }
        if let Some(deny) = &self.deny {
            if deny.is_empty() {
                return Err("deny group list is empty".to_string());
            }
            let (m, _errs) = build_flat_matcher_partial(deny);
            self.deny_matcher = Some(m);
        }
        use crate::matcher::ip_cidr_helpers::validate_refs_structure;
        if let Some(refs) = &self.allow_refs {
            validate_refs_structure(refs)?;
        }
        if let Some(refs) = &self.deny_refs {
            validate_refs_structure(refs)?;
        }
        Ok(())
    }

    /// Returns the full access decision (group name included).
    ///
    /// Priority: deny > allow > default_action. See spec §4.6.
    /// Delegates to the shared `evaluate_ip_access` so the L4, HTTP, and
    /// global-connection IP-restriction paths cannot drift.
    pub fn check_ip(&self, ip: &IpAddr) -> AccessDecision {
        evaluate_ip_access(
            self.deny_matcher.as_ref(),
            self.allow_matcher.as_ref(),
            &self.default_action,
            ip,
        )
    }

    /// Whitelist mode by configured intent (spec Decision 5).
    pub fn allow_mode(&self) -> bool {
        self.allow.as_ref().is_some_and(|v| !v.is_empty()) || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty())
    }

    /// Decision over inline ∪ gateway-resolved reference matchers.
    pub fn check_ip_with_refs(
        &self,
        ip: &IpAddr,
        deny_ref_matchers: &[&IpRadixMatcher],
        allow_ref_matchers: &[&IpRadixMatcher],
    ) -> AccessDecision {
        use crate::matcher::ip_cidr_helpers::evaluate_ip_access_multi;
        let mut deny: Vec<&IpRadixMatcher> = self.deny_matcher.iter().collect();
        deny.extend_from_slice(deny_ref_matchers);
        let mut allow: Vec<&IpRadixMatcher> = self.allow_matcher.iter().collect();
        allow.extend_from_slice(allow_ref_matchers);
        evaluate_ip_access_multi(&deny, &allow, self.allow_mode(), &self.default_action, ip)
    }
}

/// Partial-good flat matcher builder. Used by `ProfileRules` once the
/// present-but-empty structural guard (kept in `validate_and_init`) has passed:
/// it flattens `Vec<IpGroup>` into a single IpRadixMatcher over the VALID entries,
/// skipping (and returning) bad CIDRs rather than failing the whole profile.
fn build_flat_matcher_partial(groups: &[IpGroup]) -> (IpRadixMatcher, Vec<EntryError>) {
    build_grouped_matcher_partial(groups)
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_stream_plugins/global_connection_ip_restriction_test.rs"]
mod tests;
