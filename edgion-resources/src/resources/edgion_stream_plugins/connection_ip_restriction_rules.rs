//! L4-specific inline IP restriction rules used by `EdgionStreamPlugin::IpRestriction`.
//!
//! Contrast with `IpRestrictionConfig` (HTTP-layer): this omits `ip_source`, `message`,
//! and `status` because L4 has no headers or protocol-level response (rejection = silent close).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::matcher::ip_cidr_helpers::{build_grouped_matcher_partial, evaluate_ip_access, validate_refs_structure};
use crate::resources::edgion_config_data::IpGroup;
use crate::resources::edgion_plugins::{AccessDecision, DefaultAction, PluginValidation};
use edgion_algo::matcher::radix_ip::IpRadixMatcher;

/// Legacy HTTP-layer field names that must be flagged when found on L4 configs.
///
/// These belong to the old HTTP `IpRestrictionConfig` schema and have no meaning in L4
/// configs. Detecting them at preparse-time lets the Controller surface a
/// `LegacySchemaDeprecated` status condition instead of failing CR deserialization at the
/// watcher level (which would silently bypass L4 enforcement and give operators no
/// migration hint).
const LEGACY_HTTP_FIELDS: &[&str] = &["ipSource", "message", "status"];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct ConnectionIpRestrictionRules {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<IpGroup>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<IpGroup>>,

    #[serde(default)]
    pub default_action: DefaultAction,

    /// Allow-list references to `IpList` EdgionConfigData resources (resolved in gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_refs: Option<Vec<crate::resources::edgion_config_data::EdgionConfigDataRef>>,

    /// Deny-list references to `IpList` EdgionConfigData resources (resolved in gateway).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_refs: Option<Vec<crate::resources::edgion_config_data::EdgionConfigDataRef>>,

    /// Catch-all for any field not recognized by the struct. Populated by serde during
    /// deserialization so the Controller's preparse can identify legacy HTTP fields
    /// (`ipSource`/`message`/`status`) from the old schema and emit a migration status
    /// condition. Never written back to K8s (Controller only patches `/status`).
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(skip)]
    unknown_fields: BTreeMap<String, serde_json::Value>,

    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) allow_matcher: Option<IpRadixMatcher>,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) deny_matcher: Option<IpRadixMatcher>,
}

impl ConnectionIpRestrictionRules {
    /// Return any legacy HTTP-layer field names (`ipSource`/`message`/`status`) captured
    /// during deserialization. A non-empty result means the YAML uses the old HTTP schema
    /// and the operator must migrate.
    pub fn legacy_http_fields(&self) -> Vec<&'static str> {
        LEGACY_HTTP_FIELDS
            .iter()
            .filter(|name| self.unknown_fields.contains_key(**name))
            .copied()
            .collect()
    }

    pub fn validate_and_init(&mut self) -> Result<(), String> {
        let any_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let any_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !any_allow && !any_deny {
            return Err("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string());
        }
        if let Some(groups) = &self.allow {
            if !groups.is_empty() {
                let (m, _errs) = build_grouped_matcher_partial(groups);
                self.allow_matcher = Some(m);
            }
        }
        if let Some(groups) = &self.deny {
            if !groups.is_empty() {
                let (m, _errs) = build_grouped_matcher_partial(groups);
                self.deny_matcher = Some(m);
            }
        }
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

/// Stateless structural validation mirroring HTTP `IpRestrictionConfig`. The
/// controller preparse calls this instead of the side-effectful
/// `validate_and_init` (which builds radix matchers and is reserved for the
/// data-plane builders in `StreamIpRestriction::new`).
impl PluginValidation for ConnectionIpRestrictionRules {
    fn validation_error(&self) -> Option<String> {
        let any_allow = self.allow.as_ref().is_some_and(|v| !v.is_empty())
            || self.allow_refs.as_ref().is_some_and(|v| !v.is_empty());
        let any_deny =
            self.deny.as_ref().is_some_and(|v| !v.is_empty()) || self.deny_refs.as_ref().is_some_and(|v| !v.is_empty());
        if !any_allow && !any_deny {
            return Some("At least one of 'allow'/'deny' or 'allowRefs'/'denyRefs' must be non-empty".to_string());
        }
        if let Some(refs) = &self.allow_refs {
            if let Err(e) = validate_refs_structure(refs) {
                return Some(format!("allowRefs: {}", e));
            }
        }
        if let Some(refs) = &self.deny_refs {
            if let Err(e) = validate_refs_structure(refs) {
                return Some(format!("denyRefs: {}", e));
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_stream_plugins/connection_ip_restriction_rules_test.rs"]
mod tests;
