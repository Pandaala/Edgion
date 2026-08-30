//! `GeoIpLocation` runtime plugin (L7 request stage).
//!
//! Two jobs, driven entirely by whether `config.is_parse_only()`:
//!
//! - **Parse-only**: resolve both `ctx.direct_peer_geo()` and `ctx.remote_geo()` (so
//!   the access log's `geo` block is populated) and pass through. No decision,
//!   no `failOpen` requirement.
//! - **Rule mode**: resolve `ctx.direct_peer_geo()` always, and `ctx.remote_geo()`
//!   only when a configured rule actually references `RemoteIp` (avoids a
//!   needless second MMDB lookup). If any CONSULTED lookup is
//!   `GeoLookup::Unavailable`, this plugin's own `failOpen` decides — `false`
//!   denies, `true` treats this plugin as having no opinion (`GoodNext`); it
//!   never influences any other plugin on the chain. Otherwise the resolved
//!   facts feed `GeoIpLocationConfig::decide`.
//!
//! HTTP geo denials are deliberately NOT metered (`edgion_geoip_denied_total`
//! is L4-only, see its doc comment) — they already appear in the access log's
//! `geo` block plus this plugin's log line.

use async_trait::async_trait;
use std::net::IpAddr;

use crate::geoip::{GeoInfo, GeoLookup};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    GeoDecision, GeoDimFacts, GeoIpLocationConfig, GeoRuleContext, GeoRuleSource,
};

fn default_status() -> u16 {
    403
}

/// Split a `GeoLookup` into matchable facts plus an "unavailable" flag. Both
/// `NotFound` and `Unavailable` yield empty facts — the caller distinguishes
/// them via the second element, since only `Unavailable` triggers `failOpen`.
fn facts_of(lookup: &GeoLookup) -> (GeoDimFacts<'_>, bool) {
    match lookup {
        GeoLookup::Unavailable => (GeoDimFacts::default(), true),
        GeoLookup::NotFound => (GeoDimFacts::default(), false),
        GeoLookup::Found(info) => (dim_facts(info), false),
    }
}

fn dim_facts(info: &GeoInfo) -> GeoDimFacts<'_> {
    GeoDimFacts {
        country: info.country_iso.as_deref(),
        continent: info.continent.as_deref(),
        subdivision: info.subdivision.as_deref(),
        city: info.city.as_deref(),
    }
}

/// `GeoIpLocation` L7 plugin: geo resolution, plus an optional deny/allow decision.
pub struct GeoIpLocation {
    name: String,
    config: GeoIpLocationConfig,
    /// True when `validate_and_init` failed at construction (should not happen
    /// — the controller and this same code path both validate first via the
    /// shared `check_fields`; see the config's module doc). Requests then fail
    /// CLOSED (non-auth default-close policy) rather than run against
    /// uncompiled matchers.
    init_failed: bool,
    /// Any configured rule references `RemoteIp` (any key, including `Cidr`)
    /// — the remote address itself is needed. Always true in parse-only mode.
    needs_remote_ip: bool,
    /// Some configured `DirectPeerIp` rule needs resolved geo dimensions (a
    /// key other than `Cidr`) — worth resolving `ctx.direct_peer_geo()` and
    /// letting its `Unavailable` state drive `failOpen`. A CIDR-only config
    /// needs no GeoIP database at all. Always true in parse-only mode
    /// (maximize log usefulness, its whole purpose).
    needs_direct_facts: bool,
    /// Same as `needs_direct_facts`, for `RemoteIp` rules / `ctx.remote_geo()`.
    needs_remote_facts: bool,
}

impl GeoIpLocation {
    pub fn create(config: &GeoIpLocationConfig) -> Box<dyn RequestFilter> {
        let mut config = config.clone();
        let init_failed = if let Err(e) = config.validate_and_init(false) {
            tracing::error!(
                error = %e,
                label = "HTTP plugin",
                "Failed to initialize GeoIpLocationConfig; failing CLOSED",
            );
            true
        } else {
            false
        };

        let parse_only = config.is_parse_only();
        let needs_remote_ip = parse_only || config.source_is_used(GeoRuleSource::RemoteIp);
        let needs_direct_facts = parse_only || config.source_needs_geo_facts(GeoRuleSource::DirectPeerIp);
        let needs_remote_facts = parse_only || config.source_needs_geo_facts(GeoRuleSource::RemoteIp);

        Box::new(GeoIpLocation {
            name: "GeoIpLocation".to_string(),
            config,
            init_failed,
            needs_remote_ip,
            needs_direct_facts,
            needs_remote_facts,
        })
    }

    fn parse_ip(addr: &str) -> Option<IpAddr> {
        if addr.is_empty() {
            return None;
        }
        addr.parse::<IpAddr>().ok()
    }

    fn deny(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
        log_reason: &str,
    ) -> PluginRunningResult {
        plugin_log.push(log_reason);
        let _ = session.set_response_header("content-type", "application/json");
        PluginRunningResult::ErrResponse {
            status: default_status(),
            body: Some(serde_json::json!({ "message": "Access denied" }).to_string()),
        }
    }
}

#[async_trait]
impl RequestFilter for GeoIpLocation {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if self.init_failed {
            return self.deny(session, plugin_log, "init fail: fail-closed");
        }

        // Resolve only what this config actually needs. Parse-only mode
        // forces both (its whole purpose is populating the access log);
        // otherwise a CIDR-only rule set on a side needs no GeoIP database at
        // all — see `needs_direct_facts`/`needs_remote_facts`.
        let direct_lookup = if self.needs_direct_facts {
            Some(session.ctx().direct_peer_geo().clone())
        } else {
            None
        };
        let remote_lookup = if self.needs_remote_facts {
            Some(session.ctx().remote_geo().clone())
        } else {
            None
        };

        if self.config.is_parse_only() {
            plugin_log.push("parse:ok");
            return PluginRunningResult::GoodNext;
        }

        let (direct_facts, direct_unavailable) = match &direct_lookup {
            Some(l) => facts_of(l),
            None => (GeoDimFacts::default(), false),
        };
        let (remote_facts, remote_unavailable) = match &remote_lookup {
            Some(l) => facts_of(l),
            None => (GeoDimFacts::default(), false),
        };

        if direct_unavailable || remote_unavailable {
            let fail_open = self.config.fail_open.unwrap_or(false);
            if fail_open {
                plugin_log.push("db:unavailable fail-open");
                return PluginRunningResult::GoodNext;
            }
            return self.deny(session, plugin_log, "db:unavailable fail-closed");
        }

        let ctx = GeoRuleContext {
            direct_peer_ip: Self::parse_ip(session.direct_peer_ip()),
            direct_peer: direct_facts,
            remote_ip: if self.needs_remote_ip {
                Self::parse_ip(session.remote_ip())
            } else {
                None
            },
            remote: remote_facts,
        };

        match self.config.decide(&ctx) {
            GeoDecision::Allow => {
                plugin_log.push("allow");
                PluginRunningResult::GoodNext
            }
            GeoDecision::Deny => self.deny(session, plugin_log, "deny"),
        }
    }
}

#[cfg(test)]
// The GEO_TEST_LOCK guard is deliberately held across `.await` in these
// tests: it serializes every test in the crate that touches the
// process-global `GeoIpProvider::global()` singleton (see its doc), so
// dropping the guard before the plugin actually performs its lookup would
// reopen exactly the race it exists to prevent (another test loading a
// fixture db mid-assertion). These async fns never yield to the executor on
// real I/O, so there is no deadlock risk from holding a sync `parking_lot`
// guard across their `.await` points.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::geoip::GeoIpProvider;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::EdgionHttpContext;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        DefaultAction, GeoRule, GeoRuleGroup, GeoRuleKey,
    };

    fn rule(source: GeoRuleSource, key: GeoRuleKey, values: &[&str]) -> GeoRule {
        GeoRule {
            source,
            key,
            values: values.iter().map(|s| s.to_string()).collect(),
            cidr_matcher: None,
        }
    }

    fn parse_only_config() -> GeoIpLocationConfig {
        GeoIpLocationConfig::default()
    }

    fn rule_mode_config(fail_open: bool) -> GeoIpLocationConfig {
        GeoIpLocationConfig {
            default_action: DefaultAction::Allow,
            fail_open: Some(fail_open),
            deny_rule_groups: vec![GeoRuleGroup {
                name: Some("block-ru".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Country, &["RU"])],
            }],
            allow_rule_groups: vec![],
        }
    }

    /// A session whose `ctx()` returns a fresh `EdgionHttpContext` and whose
    /// `direct_peer_ip`/`remote_ip` are set, so `direct_peer_geo()`/`remote_geo()`
    /// resolve live against `GeoIpProvider::global()` (cleared by the caller
    /// under `crate::geoip::GEO_TEST_LOCK` for db-unavailable determinism).
    fn session_with_ips(direct_peer_ip: &'static str, remote_ip: &'static str) -> MockPluginSession {
        let mut ctx = EdgionHttpContext::new();
        // `ctx.direct_peer_geo()`/`remote_geo()` resolve off `ctx.request_info`, a
        // separate field from the canonical PluginSession methods mocked
        // below — both must be set for
        // the two to agree, as the real session wrapper keeps them in sync.
        ctx.request_info.direct_peer_ip = direct_peer_ip.to_string();
        ctx.request_info.remote_ip = remote_ip.to_string();

        let mut s = MockPluginSession::new();
        s.expect_ctx().return_const(ctx);
        s.expect_direct_peer_ip().return_const(direct_peer_ip.to_string());
        s.expect_remote_ip().return_const(remote_ip.to_string());
        s
    }

    #[tokio::test]
    async fn parse_only_mode_never_denies_and_needs_no_fail_open() {
        let _lock = crate::geoip::GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();
        let plugin = GeoIpLocation::create(&parse_only_config());
        let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
        let mut log = PluginLog::new("GeoIpLocation");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("parse:ok"));
    }

    #[tokio::test]
    async fn db_unavailable_fail_closed_denies() {
        let _lock = crate::geoip::GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();
        let plugin = GeoIpLocation::create(&rule_mode_config(false));
        let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
        session.expect_set_response_header().returning(|name, value| {
            assert_eq!((name, value), ("content-type", "application/json"));
            Ok(())
        });
        let mut log = PluginLog::new("GeoIpLocation");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: Some(r#"{"message":"Access denied"}"#.to_string()),
            }
        );
        assert!(log.contains("db:unavailable fail-closed"));
    }

    #[tokio::test]
    async fn db_unavailable_fail_open_passes() {
        let _lock = crate::geoip::GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();
        let plugin = GeoIpLocation::create(&rule_mode_config(true));
        let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
        let mut log = PluginLog::new("GeoIpLocation");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("db:unavailable fail-open"));
    }

    #[tokio::test]
    async fn cidr_only_config_is_unaffected_by_db_unavailability() {
        let _lock = crate::geoip::GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();
        let cfg = GeoIpLocationConfig {
            default_action: DefaultAction::Allow,
            fail_open: Some(false),
            allow_rule_groups: vec![GeoRuleGroup {
                name: Some("office".into()),
                rules: vec![rule(GeoRuleSource::DirectPeerIp, GeoRuleKey::Cidr, &["10.0.0.0/8"])],
            }],
            ..Default::default()
        };
        let plugin = GeoIpLocation::create(&cfg);

        let mut allowed_session = session_with_ips("10.1.2.3", "10.1.2.3");
        let mut log = PluginLog::new("GeoIpLocation");
        let allowed = plugin.run_request(&mut allowed_session, &mut log).await;
        assert_eq!(allowed, PluginRunningResult::GoodNext);

        let mut denied_session = session_with_ips("8.8.8.8", "8.8.8.8");
        denied_session.expect_set_response_header().returning(|name, value| {
            assert_eq!((name, value), ("content-type", "application/json"));
            Ok(())
        });
        let mut log2 = PluginLog::new("GeoIpLocation");
        let denied = plugin.run_request(&mut denied_session, &mut log2).await;
        assert_eq!(
            denied,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: Some(r#"{"message":"Access denied"}"#.to_string()),
            }
        );
    }

    #[tokio::test]
    async fn init_failure_fails_closed() {
        let _lock = crate::geoip::GEO_TEST_LOCK.lock();
        GeoIpProvider::global().clear();
        // Rule mode without failOpen fails validate_and_init at construction.
        let mut cfg = rule_mode_config(false);
        cfg.fail_open = None;
        let plugin = GeoIpLocation::create(&cfg);
        let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
        session.expect_set_response_header().returning(|name, value| {
            assert_eq!((name, value), ("content-type", "application/json"));
            Ok(())
        });
        let mut log = PluginLog::new("GeoIpLocation");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 403,
                body: Some(r#"{"message":"Access denied"}"#.to_string()),
            }
        );
        assert!(log.contains("init fail: fail-closed"));
    }
}
