//! `GeoIpLocation` plugin configuration.
//!
//! One plugin, registered into both `EdgionPlugin` (HTTP) and
//! `EdgionStreamPlugin` (L4 ConnectionFilter stage) with the identical config
//! shape — this struct. Two jobs:
//!
//! 1. **Parse-only** (`denyRuleGroups`/`allowRuleGroups` both empty) —
//!    **HTTP only**: resolve geo for the request and stop, populating the
//!    access log's `geo` block via `ctx.direct_peer_geo()`/`remote_geo()`. The
//!    plugin makes no allow/deny decision and does not require `failOpen`.
//!    At L4 this same config is a hard admission error: `StreamContext` has
//!    no logging sink and there is no L4 access log, so a resolved location
//!    has nowhere to go and the plugin would provably do nothing. An L4
//!    config must carry at least one rule group; observe-without-enforcing
//!    is an HTTP-layer capability.
//! 2. **Rule mode**: evaluate `denyRuleGroups`/`allowRuleGroups` (each OR-ed
//!    internally AND-ed groups of [`GeoRule`]s, see `geo_rule`) and decide:
//!    deny hit => deny; allow configured and missed => deny; otherwise
//!    `defaultAction`. `failOpen` becomes required and governs ONLY this
//!    plugin's own decision when the GeoIP database is unavailable or a
//!    loaded-database lookup fails — it never reaches into any other plugin
//!    on the chain.
//!
//! Validation (`check_fields`) is layer-parameterized (`is_stream`) rather
//! than split into two methods so the controller ingest path and the gateway
//! construction path can never drift: both call this exact function. See
//! `edgion-gateway/src/plugins/http/geoip_location/plugin.rs` and
//! `edgion-gateway/src/plugins/stream/geoip_location.rs` for the two callers,
//! and `geo_ip_location_check_fields_parity` (this module's tests) plus the
//! gateway-side parity tests for the "identical error text" guarantee.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::geo_rule::{GeoRuleContext, GeoRuleGroup, GeoRuleKey, GeoRuleSource};
use super::ip_restriction::DefaultAction;
use crate::resources::edgion_plugins::PluginValidation;

/// `GeoIpLocation` plugin configuration — shared by the HTTP and L4 registrations.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeoIpLocationConfig {
    /// Action when neither side has an opinion (no deny hit, no allow
    /// configured or an allow hit). Defaults to `Allow`, matching the
    /// codebase's `defaultAction` convention.
    #[serde(default)]
    pub default_action: DefaultAction,

    /// Required when `denyRuleGroups`/`allowRuleGroups` is non-empty; ignored
    /// (and not required) in parse-only mode. Governs ONLY this plugin's own
    /// decision when the GeoIP database is unavailable or a loaded-database
    /// lookup fails: `false` denies, `true` treats this plugin as having no
    /// opinion (the chain's other plugins still decide normally).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_open: Option<bool>,

    /// Deny groups, OR-ed. A hit here denies regardless of `allowRuleGroups`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_rule_groups: Vec<GeoRuleGroup>,

    /// Allow groups, OR-ed. When non-empty, a miss here denies even if no
    /// deny group matched (whitelist semantics).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_rule_groups: Vec<GeoRuleGroup>,
}

/// Final judgement of `GeoIpLocationConfig::decide`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoDecision {
    Allow,
    Deny,
}

impl GeoIpLocationConfig {
    /// True when this config has no rule groups at all — parse-only mode.
    pub fn is_parse_only(&self) -> bool {
        self.deny_rule_groups.is_empty() && self.allow_rule_groups.is_empty()
    }

    /// Stateless field validation, shared verbatim by the controller ingest
    /// path and the gateway construction path. `is_stream` is the only
    /// layer-specific input: an L4 config rejects `RemoteIp` rules.
    pub fn check_fields(&self, is_stream: bool) -> Option<String> {
        if is_stream && self.is_parse_only() {
            return Some(
                "GeoIpLocation: an L4 (stream) config must declare at least one denyRuleGroups or \
                 allowRuleGroups entry — parse-only mode has no effect at L4, where there is no \
                 access log for a resolved location to reach; use the HTTP registration to observe \
                 geo without enforcing"
                    .to_string(),
            );
        }
        if !self.is_parse_only() && self.fail_open.is_none() {
            return Some(
                "GeoIpLocation: failOpen is required when denyRuleGroups or allowRuleGroups is configured".to_string(),
            );
        }
        for (label, groups) in [
            ("denyRuleGroups", &self.deny_rule_groups),
            ("allowRuleGroups", &self.allow_rule_groups),
        ] {
            for (gi, group) in groups.iter().enumerate() {
                if let Some(e) = group.check_fields(is_stream) {
                    return Some(format!("{label}[{gi}]: {e}"));
                }
            }
        }
        None
    }

    /// Validate then compile every rule's matcher. Mutates `self`.
    pub fn validate_and_init(&mut self, is_stream: bool) -> Result<(), String> {
        if let Some(e) = self.check_fields(is_stream) {
            return Err(e);
        }
        for group in self
            .deny_rule_groups
            .iter_mut()
            .chain(self.allow_rule_groups.iter_mut())
        {
            group.compile(is_stream)?;
        }
        Ok(())
    }

    /// True iff any rule for `source` needs resolved geo dimensions (a key
    /// other than `Cidr`). A CIDR-only config needs no GeoIP database at all
    /// — `key: cidr` matches purely off the peer address — so callers use
    /// this (not `source_is_used`) to decide whether an unavailable database
    /// should even be consulted for the fail-open/fail-close decision. Without
    /// this distinction, a pure-CIDR allow/deny list would spuriously trip
    /// `failOpen` whenever the (irrelevant) country database is not loaded.
    pub fn source_needs_geo_facts(&self, source: GeoRuleSource) -> bool {
        self.deny_rule_groups
            .iter()
            .chain(self.allow_rule_groups.iter())
            .any(|g| g.rules.iter().any(|r| r.source == source && r.key != GeoRuleKey::Cidr))
    }

    /// True iff any rule at all references `source` (any key, including
    /// `Cidr`) — used to decide whether the source's raw peer address is
    /// needed at all.
    pub fn source_is_used(&self, source: GeoRuleSource) -> bool {
        self.deny_rule_groups
            .iter()
            .chain(self.allow_rule_groups.iter())
            .any(|g| g.rules.iter().any(|r| r.source == source))
    }

    /// True iff any deny group matches. Exposed (not just used internally by
    /// `decide`) so a caller that needs to distinguish WHY a request was
    /// denied — e.g. the L4 plugin picking a bounded metric `reason` label —
    /// doesn't have to re-implement this half of the judgement.
    pub fn denies(&self, ctx: &GeoRuleContext<'_>) -> bool {
        self.deny_rule_groups.iter().any(|g| g.matches(ctx))
    }

    /// True iff any allow group matches. See [`Self::denies`].
    pub fn allows(&self, ctx: &GeoRuleContext<'_>) -> bool {
        self.allow_rule_groups.iter().any(|g| g.matches(ctx))
    }

    /// The plugin-level deny/allow judgement, assuming the GeoIP database was
    /// available for every source this config's rules consult (the
    /// db-unavailable / lookup-failed / `failOpen` short-circuit happens one
    /// layer up, in the gateway plugin, because only it knows which lookups
    /// returned `GeoLookup::Unavailable` or `GeoLookup::Failed`).
    ///
    /// Order — the canonical `deny > allow > default` priority every other
    /// access-policy plugin uses (`matcher::ip_cidr_helpers::
    /// evaluate_ip_access_multi`, AGENTS.md cross-cutting rule 11):
    ///
    /// 1. a deny group hits => `Deny`;
    /// 2. otherwise, if any allow group is configured this is whitelist mode
    ///    and the allow result is **terminal** — a hit is `Allow`, a miss is
    ///    `Deny`. `defaultAction` is deliberately not consulted here: falling
    ///    through to a `deny` default would make a matched whitelist entry
    ///    reject the very traffic it was written to admit;
    /// 3. with no allow group configured, `defaultAction`.
    pub fn decide(&self, ctx: &GeoRuleContext<'_>) -> GeoDecision {
        if self.denies(ctx) {
            return GeoDecision::Deny;
        }
        if !self.allow_rule_groups.is_empty() {
            return if self.allows(ctx) {
                GeoDecision::Allow
            } else {
                GeoDecision::Deny
            };
        }
        match self.default_action {
            DefaultAction::Allow => GeoDecision::Allow,
            DefaultAction::Deny => GeoDecision::Deny,
        }
    }
}

impl PluginValidation for GeoIpLocationConfig {
    /// HTTP-layer validation entry point (the fixed `PluginValidation`
    /// signature carries no layer flag): `EdgionPlugin::validation_error()`'s
    /// exhaustive match calls this for `EdgionPlugin::GeoIpLocation`, and the
    /// gateway's `plugin_validation_error` calls the exact same
    /// `EdgionPlugin::validation_error()`, so the HTTP controller/gateway
    /// paths are provably identical. The L4 registration does NOT go through
    /// this trait (`EdgionStreamPlugin` has no such exhaustive dispatcher);
    /// its controller and gateway call sites invoke
    /// `check_fields(true)`/`validate_and_init(true)` directly — see the two
    /// call sites named in the module doc.
    fn validation_error(&self) -> Option<String> {
        self.check_fields(false)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/geoip_location_test.rs"]
mod tests;
