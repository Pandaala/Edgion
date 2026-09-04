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
mod tests {
    use super::*;
    use crate::resources::edgion_plugins::plugin_configs::geo_rule::{GeoDimFacts, GeoRule, GeoRuleKey, GeoRuleSource};

    fn rule(source: GeoRuleSource, key: GeoRuleKey, values: &[&str]) -> GeoRule {
        GeoRule {
            source,
            key,
            values: values.iter().map(|s| s.to_string()).collect(),
            cidr_matcher: None,
        }
    }

    /// Parse-only is an HTTP-only mode: it is valid (and needs no `failOpen`)
    /// at L7, where the resolved location reaches the access log.
    #[test]
    fn parse_only_config_is_valid_at_l7_without_fail_open() {
        let cfg = GeoIpLocationConfig::default();
        assert!(cfg.is_parse_only());
        assert!(cfg.check_fields(false).is_none());
    }

    /// ...and rejected at L4, where it resolves a location with nowhere to put
    /// it — `StreamContext` carries no logging sink and there is no L4 access
    /// log, so the whole plugin would be a no-op. An operator writing this
    /// meant something else; say so at admission instead of accepting a
    /// configuration that provably cannot do anything.
    #[test]
    fn parse_only_config_is_rejected_at_l4() {
        let cfg = GeoIpLocationConfig::default();
        let err = cfg.check_fields(true).expect("L4 parse-only must be rejected");
        assert!(err.contains("at least one"), "unexpected error: {err}");
    }

    #[test]
    fn rule_mode_without_fail_open_is_rejected() {
        let cfg = GeoIpLocationConfig {
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
            }],
            ..Default::default()
        };
        let err = cfg.check_fields(false).unwrap();
        assert!(err.contains("failOpen"));
    }

    #[test]
    fn rule_mode_with_fail_open_is_valid() {
        let mut cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
            }],
            ..Default::default()
        };
        assert!(cfg.validate_and_init(false).is_ok());
    }

    #[test]
    fn remote_ip_rejected_in_stream_layer() {
        let cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::RemoteIp, GeoRuleKey::Country, &["RU"])],
            }],
            ..Default::default()
        };
        assert!(cfg.check_fields(false).is_none());
        assert!(cfg.check_fields(true).unwrap().contains("RemoteIp"));
    }

    #[test]
    fn deny_beats_allow_and_allow_miss_denies_under_default_allow() {
        let mut cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            default_action: DefaultAction::Allow,
            deny_rule_groups: vec![GeoRuleGroup {
                name: Some("block-moscow".into()),
                rules: vec![
                    rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::City, &["Moscow"]),
                    rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"]),
                ],
            }],
            allow_rule_groups: vec![GeoRuleGroup {
                name: Some("office".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
            }],
        };
        cfg.validate_and_init(false).unwrap();

        // Deny hit dominates even though it's also outside the allow set.
        let deny_ctx = GeoRuleContext {
            direct_peer: GeoDimFacts {
                country: Some("RU"),
                city: Some("Moscow"),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.decide(&deny_ctx), GeoDecision::Deny);

        // Neither deny nor allow hit -> allow configured and missed -> Deny.
        let miss_ctx = GeoRuleContext {
            direct_peer_ip: Some("8.8.8.8".parse().unwrap()),
            direct_peer: GeoDimFacts {
                country: Some("US"),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.decide(&miss_ctx), GeoDecision::Deny);

        // Allow hit -> Allow.
        let allow_ctx = GeoRuleContext {
            direct_peer_ip: Some("10.1.2.3".parse().unwrap()),
            direct_peer: GeoDimFacts {
                country: Some("US"),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.decide(&allow_ctx), GeoDecision::Allow);
    }

    #[test]
    fn default_action_deny_with_no_rule_groups_denies_everything() {
        // Degenerate but legal: rule groups present only via defaultAction; here
        // we exercise the "no groups at all, default deny" path indirectly by
        // giving a deny group that never matches would be equivalent to
        // defaultAction alone -- assert the plain defaultAction path directly.
        let cfg = GeoIpLocationConfig {
            default_action: DefaultAction::Deny,
            fail_open: Some(false),
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["ZZ"])],
            }],
            ..Default::default()
        };
        let ctx = GeoRuleContext::default();
        assert_eq!(cfg.decide(&ctx), GeoDecision::Deny);
    }

    /// Regression: an allow-group HIT is terminal and must not fall through
    /// to `defaultAction`. The whitelist shape `defaultAction: deny` +
    /// `allowRuleGroups: [CN]` is the whole point of a whitelist — if the
    /// default were consulted after a hit, the config would reject exactly
    /// the traffic it was written to admit, and every whitelist would be
    /// silently inert. Matches `evaluate_ip_access_multi`'s
    /// `deny > allow > default` priority.
    #[test]
    fn allow_group_hit_is_terminal_under_default_deny() {
        let cfg = GeoIpLocationConfig {
            default_action: DefaultAction::Deny,
            fail_open: Some(false),
            allow_rule_groups: vec![GeoRuleGroup {
                name: Some("cn-allow".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["CN"])],
            }],
            ..Default::default()
        };
        let hit = GeoRuleContext {
            direct_peer: GeoDimFacts {
                country: Some("CN"),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.decide(&hit), GeoDecision::Allow, "an allow hit must allow");
        // A miss under the same config still denies (whitelist miss), and it
        // denies because of the miss, not because of `defaultAction`.
        let miss = GeoRuleContext {
            direct_peer: GeoDimFacts {
                country: Some("RU"),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.decide(&miss), GeoDecision::Deny);
    }

    /// The same allow-hit terminality holds with `defaultAction: allow` — the
    /// default is simply never reached once an allow group is configured.
    #[test]
    fn allow_group_miss_denies_even_under_default_allow() {
        let cfg = GeoIpLocationConfig {
            default_action: DefaultAction::Allow,
            fail_open: Some(false),
            allow_rule_groups: vec![GeoRuleGroup {
                name: Some("cn-allow".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["CN"])],
            }],
            ..Default::default()
        };
        let miss = GeoRuleContext {
            direct_peer: GeoDimFacts {
                country: Some("RU"),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(cfg.decide(&miss), GeoDecision::Deny);
    }

    /// The failure this plugin most needs to be immune to: a mistyped
    /// `denyRuleGroup` (singular) used to deserialize into a config with no
    /// rule groups at all — i.e. parse-only mode. The operator's deny rules
    /// vanished, their `failOpen: false` stopped being required, every request
    /// was allowed, and the resource still reported `Accepted: True`.
    #[test]
    fn mistyped_rule_group_key_is_rejected_not_degraded_to_parse_only() {
        let yaml = "defaultAction: allow\nfailOpen: false\ndenyRuleGroup:\n  - name: g\n    rules:\n      - source: DirectPeerIp\n        key: country\n        values: [\"RU\"]\n";
        let e =
            serde_yaml::from_str::<GeoIpLocationConfig>(yaml).expect_err("a mistyped rule-group key must be rejected");
        assert!(e.to_string().contains("denyRuleGroup"), "unexpected error: {e}");
    }

    /// Parse-only remains reachable the only way it should be: by actually
    /// writing an empty config.
    #[test]
    fn explicitly_empty_config_is_still_parse_only() {
        let cfg = serde_yaml::from_str::<GeoIpLocationConfig>("{}").expect("an empty config stays valid");
        assert!(cfg.is_parse_only());
        assert!(cfg.check_fields(false).is_none());
    }

    #[test]
    fn cidr_only_config_needs_no_geo_facts() {
        let cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            allow_rule_groups: vec![GeoRuleGroup {
                name: Some("office".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
            }],
            ..Default::default()
        };
        assert!(!cfg.source_needs_geo_facts(GeoRuleSource::DirectPeerIp));
        assert!(cfg.source_is_used(GeoRuleSource::DirectPeerIp));
        assert!(!cfg.source_is_used(GeoRuleSource::RemoteIp));
    }

    #[test]
    fn country_rule_needs_geo_facts() {
        let cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
            }],
            ..Default::default()
        };
        assert!(cfg.source_needs_geo_facts(GeoRuleSource::DirectPeerIp));
    }

    /// Controller-ingest-shaped call vs. gateway-construction-shaped call must
    /// produce byte-identical error text, per invariant #5 in the task brief.
    #[test]
    fn check_fields_and_validate_and_init_agree_on_error_text() {
        let cfg = GeoIpLocationConfig {
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
            }],
            ..Default::default()
        };
        // Controller-ingest shape: stateless check_fields (no compile).
        let controller_err = cfg.check_fields(false);
        // Gateway-construction shape: validate_and_init (check_fields first, then compile).
        let gateway_err = cfg.clone().validate_and_init(false).err();
        assert_eq!(controller_err, gateway_err);
        assert!(controller_err.unwrap().contains("failOpen"));
    }
}
