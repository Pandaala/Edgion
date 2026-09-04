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
//!   `GeoLookup::Unavailable` or `GeoLookup::Failed`, this plugin's own
//!   `failOpen` decides — `false` denies, `true` treats this plugin as having
//!   no opinion (`GoodNext`); it never influences any other plugin on the
//!   chain. Otherwise the resolved facts feed `GeoIpLocationConfig::decide`.
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

const PLUGIN_NAME: &str = "GeoIpLocation";
const DENY_BODY: &str = r#"{"message":"Access denied"}"#;

fn default_status() -> u16 {
    403
}

#[derive(Clone, Copy)]
enum RuntimeDecision {
    DatabaseUnavailable,
    LookupFailed,
    Rule(GeoDecision),
}

/// Classify a lookup for rule evaluation. `NotFound` is ordinary missingness
/// (empty facts). Only `Unavailable` and `Failed` trigger `failOpen`.
enum LookupClass<'a> {
    Ready(GeoDimFacts<'a>),
    Unavailable,
    Failed,
}

fn classify(lookup: &GeoLookup) -> LookupClass<'_> {
    match lookup {
        GeoLookup::Unavailable => LookupClass::Unavailable,
        GeoLookup::Failed => LookupClass::Failed,
        GeoLookup::NotFound => LookupClass::Ready(GeoDimFacts::default()),
        GeoLookup::Found(info) => LookupClass::Ready(dim_facts(info)),
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
    /// letting its `Unavailable`/`Failed` state drive `failOpen`. A CIDR-only config
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

    fn deny_response(&self, session: &mut dyn PluginSession) -> PluginRunningResult {
        let _ = session.set_response_header("content-type", "application/json");
        PluginRunningResult::ErrResponse {
            status: default_status(),
            body: Some(DENY_BODY.to_owned()),
        }
    }
}

#[async_trait]
impl RequestFilter for GeoIpLocation {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if self.init_failed {
            plugin_log.push_err("init fail: fail-closed");
            return self.deny_response(session);
        }

        if self.config.is_parse_only() {
            // Parse-only mode intentionally initializes both lazy access-log
            // slots but does not retain or clone either lookup.
            let ctx = session.ctx();
            let _ = ctx.direct_peer_geo();
            let _ = ctx.remote_geo();
            plugin_log.push("parse:ok");
            return PluginRunningResult::GoodNext;
        }

        // Resolve only what this config actually needs. The lookup borrows are
        // kept inside this scope; only the copyable decision escapes it. A
        // CIDR-only side therefore still performs no GeoIP database lookup.
        let decision = {
            let request_ctx = session.ctx();
            let direct_lookup = self.needs_direct_facts.then(|| request_ctx.direct_peer_geo());
            let remote_lookup = self.needs_remote_facts.then(|| request_ctx.remote_geo());

            let (direct_facts, mut unavailable, mut failed) = match direct_lookup {
                Some(lookup) => match classify(lookup) {
                    LookupClass::Ready(facts) => (facts, false, false),
                    LookupClass::Unavailable => (GeoDimFacts::default(), true, false),
                    LookupClass::Failed => (GeoDimFacts::default(), false, true),
                },
                None => (GeoDimFacts::default(), false, false),
            };
            let (remote_facts, remote_unavailable, remote_failed) = match remote_lookup {
                Some(lookup) => match classify(lookup) {
                    LookupClass::Ready(facts) => (facts, false, false),
                    LookupClass::Unavailable => (GeoDimFacts::default(), true, false),
                    LookupClass::Failed => (GeoDimFacts::default(), false, true),
                },
                None => (GeoDimFacts::default(), false, false),
            };
            unavailable |= remote_unavailable;
            failed |= remote_failed;

            if failed {
                RuntimeDecision::LookupFailed
            } else if unavailable {
                RuntimeDecision::DatabaseUnavailable
            } else {
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
                RuntimeDecision::Rule(self.config.decide(&ctx))
            }
        };

        match decision {
            RuntimeDecision::LookupFailed if self.config.fail_open.unwrap_or(false) => {
                plugin_log.push_err("lookup:failed fail-open");
                PluginRunningResult::GoodNext
            }
            RuntimeDecision::LookupFailed => {
                plugin_log.push_err("lookup:failed fail-closed");
                self.deny_response(session)
            }
            RuntimeDecision::DatabaseUnavailable if self.config.fail_open.unwrap_or(false) => {
                plugin_log.push_err("db:unavailable fail-open");
                PluginRunningResult::GoodNext
            }
            RuntimeDecision::DatabaseUnavailable => {
                plugin_log.push_err("db:unavailable fail-closed");
                self.deny_response(session)
            }
            RuntimeDecision::Rule(GeoDecision::Allow) => {
                plugin_log.push("allow");
                PluginRunningResult::GoodNext
            }
            RuntimeDecision::Rule(GeoDecision::Deny) => {
                plugin_log.push("deny");
                self.deny_response(session)
            }
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
    use crate::geoip::{inject_lookup_fault, GeoIpProvider, GeoLookup, LookupFault};
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

    #[test]
    fn name_is_static_across_instances() {
        let first = GeoIpLocation::create(&parse_only_config());
        let second = GeoIpLocation::create(&parse_only_config());

        assert_eq!(first.name(), PLUGIN_NAME);
        assert_eq!(second.name(), PLUGIN_NAME);
        assert!(std::ptr::eq(first.name(), second.name()));
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
        assert_eq!(log.render_for_test(), "parse:ok");
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
        assert_eq!(log.render_for_test(), "err: db:unavailable fail-closed");
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
        assert_eq!(log.render_for_test(), "err: db:unavailable fail-open");
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
        assert_eq!(log.render_for_test(), "allow");

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
        assert_eq!(log2.render_for_test(), "deny");
    }

    #[tokio::test]
    async fn cidr_only_config_is_unaffected_by_lookup_failure() {
        let _lock = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture_db();
        let _fault = inject_lookup_fault(LookupFault::Decode);
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
        assert_eq!(log.render_for_test(), "allow");

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
        assert_eq!(log2.render_for_test(), "deny");
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
        assert_eq!(log.render_for_test(), "err: init fail: fail-closed");
    }

    fn load_fixture_db() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb");
        GeoIpProvider::global()
            .load_from(&path)
            .expect("city fixture must load");
    }

    #[tokio::test]
    async fn lookup_failed_fail_closed_denies() {
        let _lock = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture_db();
        let _fault = inject_lookup_fault(LookupFault::Decode);
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
        assert_eq!(log.render_for_test(), "err: lookup:failed fail-closed");
    }

    #[tokio::test]
    async fn lookup_failed_fail_open_passes() {
        let _lock = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture_db();
        let _fault = inject_lookup_fault(LookupFault::Decode);
        let plugin = GeoIpLocation::create(&rule_mode_config(true));
        let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
        let mut log = PluginLog::new("GeoIpLocation");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "err: lookup:failed fail-open");
    }

    #[tokio::test]
    async fn parse_only_mode_passes_through_lookup_failure() {
        let _lock = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture_db();
        let _fault = inject_lookup_fault(LookupFault::Decode);
        let plugin = GeoIpLocation::create(&parse_only_config());
        let mut session = session_with_ips("8.8.8.8", "8.8.8.8");
        let mut log = PluginLog::new("GeoIpLocation");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "parse:ok");
        let (remote, direct) = session.ctx().request_info.resolved_geo();
        assert_eq!(remote, Some(&GeoLookup::Failed));
        assert_eq!(direct, Some(&GeoLookup::Failed));
    }
}
