//! `GeoIpLocation` stream plugin for L4 stream lifecycles.
//!
//! Listener attachment runs at Stage 1. TCP/UDP route
//! attachments run at their respective lifecycle points.
//!
//! Same config shape and matching core as the HTTP `GeoIpLocation`
//! (`edgion_resources::resources::edgion_plugins::plugin_configs::GeoIpLocationConfig`),
//! constrained to `DirectPeerIp`-sourced rules only (enforced at validation —
//! see the config's `check_fields(is_stream: true)`).
//!
//! Unlike the HTTP side, stream lifecycles do not populate an HTTP access-log
//! `geo` block, so L4 configuration must include at least one allow or deny rule
//! group; rule-less parse-only configuration is rejected because nothing would
//! consume the resolved facts. Valid rule mode performs at most one lookup
//! (`ctx.direct_peer_ip`, the physical direct-peer address); CIDR-only rule sets
//! perform zero lookups. Every deny is reported via
//! `edgion_geoip_denied_total{layer,reason}`. A listener Stage 1 denial is
//! otherwise metric-only, including on TLS listeners. TCP/UDP route denials retain their
//! canonical protocol logs, but those records contain neither GeoIP facts nor
//! the sensitive Geo denial reason.

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
    needs_facts: bool,
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
        let needs_facts = config.source_needs_geo_facts(GeoRuleSource::DirectPeerIp);
        Ok(Self { config, needs_facts })
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
        let lookup = self
            .needs_facts
            .then(|| GeoIpProvider::global().lookup(ctx.direct_peer_ip));
        let (facts, unavailable, failed) = match &lookup {
            None => (GeoDimFacts::default(), false, false),
            Some(GeoLookup::Unavailable) => (GeoDimFacts::default(), true, false),
            Some(GeoLookup::Failed) => (GeoDimFacts::default(), false, true),
            Some(GeoLookup::NotFound) => (GeoDimFacts::default(), false, false),
            Some(GeoLookup::Found(info)) => (dim_facts(info), false, false),
        };

        if failed || unavailable {
            let fail_open = self.config.fail_open.unwrap_or(false);
            if fail_open {
                return StreamPluginResult::Allow;
            }
            let reason = if failed { "lookup_failed" } else { "db_unavailable" };
            record_geoip_denied(ctx.layer, reason);
            return StreamPluginResult::Deny(format!(
                "geo:deny:{}",
                if failed { "lookup-failed" } else { "db-unavailable" }
            ));
        }

        let rule_ctx = GeoRuleContext {
            direct_peer_ip: Some(ctx.direct_peer_ip),
            direct_peer: facts,
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
                let reason = if self.config.denies(&rule_ctx) {
                    "deny_country"
                } else if !self.config.allow_rule_groups.is_empty() {
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
    use crate::geoip::{inject_lookup_fault, GeoIpProvider, LookupFault, GEO_TEST_LOCK};
    use crate::plugins::stream::stream_plugin_trait::StreamLayer;
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
        StreamContext::new(ip.parse().unwrap(), 8080, StreamLayer::Stage1)
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

    #[tokio::test]
    async fn cidr_only_config_is_unaffected_by_lookup_failure() {
        let _lock = GEO_TEST_LOCK.lock();
        load_fixture_db();
        let _fault = inject_lookup_fault(LookupFault::Decode);
        let cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            default_action: DefaultAction::Allow,
            allow_rule_groups: vec![GeoRuleGroup {
                name: Some("office".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
            }],
            ..Default::default()
        };
        let plugin = GeoIpLocationStreamPlugin::new(&cfg).unwrap();
        let allowed = plugin.on_connection(&stream_ctx("10.1.2.3")).await;
        assert!(matches!(allowed, StreamPluginResult::Allow));
        let denied = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
        assert!(matches!(denied, StreamPluginResult::Deny(_)));
    }

    #[tokio::test]
    async fn cidr_deny_hit_and_allow_miss_have_exact_reasons_and_metrics() {
        let cfg = GeoIpLocationConfig {
            fail_open: Some(false),
            default_action: DefaultAction::Allow,
            deny_rule_groups: vec![GeoRuleGroup {
                name: Some("blocked".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
            }],
            allow_rule_groups: vec![GeoRuleGroup {
                name: Some("office".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["192.168.0.0/16"])],
            }],
        };
        let plugin = GeoIpLocationStreamPlugin::new(&cfg).unwrap();
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        let deny_hit = metrics::with_local_recorder(&recorder, || {
            futures::FutureExt::now_or_never(plugin.on_connection(&stream_ctx("10.1.2.3")))
                .expect("CIDR-only GeoIpLocation stream path must not yield")
        });
        assert!(matches!(
            deny_hit,
            StreamPluginResult::Deny(reason) if reason == "geo:deny:deny_country"
        ));

        let allow_miss = metrics::with_local_recorder(&recorder, || {
            futures::FutureExt::now_or_never(plugin.on_connection(&stream_ctx("8.8.8.8")))
                .expect("CIDR-only GeoIpLocation stream path must not yield")
        });
        assert!(matches!(
            allow_miss,
            StreamPluginResult::Deny(reason) if reason == "geo:deny:allow_miss"
        ));

        let rendered = handle.render();
        assert!(
            rendered.contains("reason=\"deny_country\"") && rendered.contains("reason=\"allow_miss\""),
            "CIDR denials must retain their exact metric labels, got:\n{rendered}"
        );
    }

    fn load_fixture_db() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb");
        GeoIpProvider::global()
            .load_from(&path)
            .expect("city fixture must load");
    }

    fn country_deny_cfg(fail_open: bool) -> GeoIpLocationConfig {
        GeoIpLocationConfig {
            fail_open: Some(fail_open),
            default_action: DefaultAction::Allow,
            deny_rule_groups: vec![GeoRuleGroup {
                name: None,
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn lookup_failed_fail_closed_denies() {
        let _lock = GEO_TEST_LOCK.lock();
        load_fixture_db();
        let _fault = inject_lookup_fault(LookupFault::Decode);
        let plugin = GeoIpLocationStreamPlugin::new(&country_deny_cfg(false)).unwrap();
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let result = metrics::with_local_recorder(&recorder, || {
            futures::FutureExt::now_or_never(plugin.on_connection(&stream_ctx("8.8.8.8")))
                .expect("GeoIpLocation stream path must not yield")
        });
        match result {
            StreamPluginResult::Deny(reason) => {
                assert!(reason.contains("lookup-failed"), "got: {reason}");
            }
            other => panic!("expected Deny, got {other:?}"),
        }
        let rendered = handle.render();
        assert!(
            rendered.contains("edgion_geoip_denied_total") && rendered.contains("reason=\"lookup_failed\""),
            "fail-close lookup failure must meter lookup_failed, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("edgion_geoip_lookup_panic_total"),
            "decode failure must not increment the panic counter, got:\n{rendered}"
        );
    }

    #[tokio::test]
    async fn lookup_failed_fail_open_allows() {
        let _lock = GEO_TEST_LOCK.lock();
        load_fixture_db();
        let _fault = inject_lookup_fault(LookupFault::Decode);
        let plugin = GeoIpLocationStreamPlugin::new(&country_deny_cfg(true)).unwrap();
        let result = plugin.on_connection(&stream_ctx("8.8.8.8")).await;
        assert!(matches!(result, StreamPluginResult::Allow));
    }

    #[tokio::test]
    async fn lookup_panic_fail_closed_denies() {
        let _lock = GEO_TEST_LOCK.lock();
        load_fixture_db();
        let _fault = inject_lookup_fault(LookupFault::Panic);
        let plugin = GeoIpLocationStreamPlugin::new(&country_deny_cfg(false)).unwrap();
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let result = metrics::with_local_recorder(&recorder, || {
            futures::FutureExt::now_or_never(plugin.on_connection(&stream_ctx("8.8.8.8")))
                .expect("GeoIpLocation stream path must not yield")
        });
        match result {
            StreamPluginResult::Deny(reason) => {
                assert!(reason.contains("lookup-failed"), "got: {reason}");
            }
            other => panic!("expected Deny, got {other:?}"),
        }
        let rendered = handle.render();
        assert!(
            rendered.contains("reason=\"lookup_failed\""),
            "panic fail-close must meter lookup_failed, got:\n{rendered}"
        );
        assert!(
            rendered.contains("edgion_geoip_lookup_panic_total"),
            "panic seam must increment edgion_geoip_lookup_panic_total, got:\n{rendered}"
        );
    }
}
