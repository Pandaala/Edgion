//! `GeoIpLocation` stream plugin — L4 ConnectionFilter stage (Stage 1, pre-TLS).
//!
//! Same config shape and matching core as the HTTP `GeoIpLocation`
//! (`edgion_resources::resources::edgion_plugins::plugin_configs::GeoIpLocationConfig`),
//! constrained to `DirectPeerIp`-sourced rules only (enforced at validation —
//! see the config's `check_fields(is_stream: true)`).
//!
//! Unlike the HTTP side, L4 has no access log `geo` block to populate (see
//! `StreamContext`'s doc — L4 geo denials are never logged, only metered), so
//! **parse-only mode at L4 is a true no-op**: no GeoIP lookup is even
//! attempted, since nothing would consume the result. Rule mode performs
//! exactly one lookup (`ctx.client_ip`, the direct peer — L4 has no other
//! address) and reports every deny via `edgion_geoip_denied_total{layer,reason}`,
//! since that metric is the only record a geo-denied L4 connection leaves.

use async_trait::async_trait;

use crate::geoip::{GeoInfo, GeoIpProvider, GeoLookup};
use crate::observe::metrics::record_geoip_denied;
use crate::plugins::stream::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    GeoDecision, GeoDimFacts, GeoIpLocationConfig, GeoRuleContext, GeoRuleSource,
};

const PLUGIN_NAME: &str = "GeoIpLocation";

fn dim_facts(info: &GeoInfo) -> GeoDimFacts<'_> {
    GeoDimFacts {
        country: info.country_iso.as_deref(),
        continent: info.continent.as_deref(),
        subdivision: info.subdivision.as_deref(),
        city: info.city.as_deref(),
    }
}

#[derive(Debug)]
pub struct GeoIpLocationStreamPlugin {
    config: GeoIpLocationConfig,
}

impl GeoIpLocationStreamPlugin {
    /// Construct from a `GeoIpLocationConfig`, compiling every rule's matcher
    /// (`is_stream: true` — a `RemoteIp` rule fails here, though it should
    /// already have been rejected at controller ingest).
    pub fn new(config: &GeoIpLocationConfig) -> Result<Self, String> {
        let mut config = config.clone();
        config
            .validate_and_init(true)
            .map_err(|e| format!("GeoIpLocation config validation failed: {e}"))?;
        Ok(Self { config })
    }
}

#[async_trait]
impl StreamPlugin for GeoIpLocationStreamPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        // No parse-only branch here: `check_fields(true)` rejects a rule-less
        // L4 config at admission, because there is no L4 access log for a
        // resolved location to reach. Every config that gets this far has at
        // least one rule group.

        // A CIDR-only config needs no GeoIP database at all — skip the lookup
        // (and the Unavailable fail-open/close branch below) entirely when no
        // configured rule actually needs resolved dimensions.
        let needs_facts = self.config.source_needs_geo_facts(GeoRuleSource::DirectPeerIp);
        let lookup = needs_facts.then(|| GeoIpProvider::global().lookup(ctx.client_ip));
        let (facts, unavailable) = match &lookup {
            None => (GeoDimFacts::default(), false),
            Some(GeoLookup::Unavailable) => (GeoDimFacts::default(), true),
            Some(GeoLookup::NotFound) => (GeoDimFacts::default(), false),
            Some(GeoLookup::Found(info)) => (dim_facts(info), false),
        };

        if unavailable {
            // Required by validation whenever rule groups are configured.
            let fail_open = self.config.fail_open.unwrap_or(false);
            if fail_open {
                return StreamPluginResult::Allow;
            }
            record_geoip_denied(ctx.layer, "db_unavailable");
            return StreamPluginResult::Deny("geo:deny:db-unavailable".to_string());
        }

        let rule_ctx = GeoRuleContext {
            direct_ip: Some(ctx.client_ip),
            direct: facts,
            remote_ip: None,
            remote: GeoDimFacts::default(),
        };

        match self.config.decide(&rule_ctx) {
            GeoDecision::Allow => StreamPluginResult::Allow,
            GeoDecision::Deny => {
                // The documented `reason` label enum has no distinct bucket for
                // "defaultAction: Deny with nothing configured to explain why" —
                // fold that (rare, degenerate) case into `deny_country` too
                // rather than adding an undeclared fourth reason value.
                let reason = if !self.config.allow_rule_groups.is_empty() && !self.config.allows(&rule_ctx) {
                    "allow_miss"
                } else {
                    "deny_country"
                };
                record_geoip_denied(ctx.layer, reason);
                StreamPluginResult::Deny(format!("geo:deny:{reason}"))
            }
        }
    }
}

#[cfg(test)]
// See the identical note in the HTTP `GeoIpLocation` plugin's test module:
// the lock must stay held through the plugin call to actually serialize
// against `GeoIpProvider::global()`.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::geoip::GEO_TEST_LOCK;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        DefaultAction, GeoRule, GeoRuleGroup, GeoRuleKey, GeoRuleSource,
    };

    fn rule(source: GeoRuleSource, key: GeoRuleKey, values: &[&str]) -> GeoRule {
        GeoRule {
            source,
            key,
            values: values.iter().map(|s| s.to_string()).collect(),
            cidr_matcher: None,
        }
    }

    fn stream_ctx(ip: &str) -> StreamContext {
        StreamContext::new(ip.parse().unwrap(), 8080).with_layer("stage1")
    }

    /// A rule-less L4 config never reaches `on_connection` — it fails at
    /// construction, the same gate the controller's ingest path uses. Parse-only
    /// is meaningful at L7 (it fills the access log) and provably inert here.
    #[tokio::test]
    async fn parse_only_config_is_rejected_at_construction() {
        let err = GeoIpLocationStreamPlugin::new(&GeoIpLocationConfig::default()).unwrap_err();
        assert!(err.contains("at least one"), "got: {err}");
    }

    #[tokio::test]
    async fn remote_ip_source_is_rejected_at_construction() {
        let cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::RemoteIp, GeoRuleKey::Country, &["RU"])],
            }],
            ..Default::default()
        };
        let err = GeoIpLocationStreamPlugin::new(&cfg).unwrap_err();
        assert!(err.contains("RemoteIp"), "got: {err}");
    }

    #[tokio::test]
    async fn db_unavailable_fail_closed_denies_and_meters() {
        let _lock = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();
        let cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            default_action: DefaultAction::Allow,
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
            }],
            ..Default::default()
        };
        let plugin = GeoIpLocationStreamPlugin::new(&cfg).unwrap();
        let result = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
        assert!(matches!(result, StreamPluginResult::Deny(_)));
    }

    #[tokio::test]
    async fn db_unavailable_fail_open_allows() {
        let _lock = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();
        let cfg = GeoIpLocationConfig {
            fail_open: Some(true),
            default_action: DefaultAction::Allow,
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
            }],
            ..Default::default()
        };
        let plugin = GeoIpLocationStreamPlugin::new(&cfg).unwrap();
        let result = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
        assert!(matches!(result, StreamPluginResult::Allow));
    }

    /// A CIDR-only config must not be affected by the GeoIP database being
    /// unavailable — CIDR matching needs no MMDB, so this must NOT hit the
    /// db-unavailable fail-open/close branch at all.
    #[tokio::test]
    async fn cidr_only_config_is_unaffected_by_db_unavailability() {
        let _lock = GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();
        let cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            // An allow-hit does not itself return Allow — it only escapes the
            // "allow configured and missed" auto-deny; the actual verdict
            // still falls through to `defaultAction` (per the decision order
            // in the task brief). Whitelist semantics need `defaultAction:
            // Allow` here: hit -> falls through -> Allow; miss -> denied by
            // the allow-miss branch regardless of `defaultAction`.
            default_action: DefaultAction::Allow,
            allow_rule_groups: vec![GeoRuleGroup {
                name: Some("office".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
            }],
            ..Default::default()
        };
        let plugin = GeoIpLocationStreamPlugin::new(&cfg).unwrap();
        // Inside the CIDR -> allow group hit, despite no GeoIP db loaded.
        let allowed = plugin.on_connection(&stream_ctx("10.1.2.3")).await;
        assert!(matches!(allowed, StreamPluginResult::Allow));
        // Outside the CIDR -> allow configured and missed -> deny.
        let denied = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
        assert!(matches!(denied, StreamPluginResult::Deny(_)));
    }
}
