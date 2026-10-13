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
use bytes::Bytes;
use std::net::IpAddr;

use crate::geoip::{GeoInfo, GeoLookup};
use crate::plugins::common::geo_ip_refs::CompiledGeoIpRefs;
use crate::plugins::runtime::conditions::ip_refs::IpConditionCompiler;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    GeoDecision, GeoDimFacts, GeoIpLocationConfig, GeoRuleContext, GeoRuleSource,
};

const PLUGIN_NAME: &str = "GeoIpLocation";
const DENY_BODY: Bytes = Bytes::from_static(br#"{"message":"Access denied"}"#);

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
    refs: CompiledGeoIpRefs,
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
    pub(crate) fn create(
        config: &GeoIpLocationConfig,
        compiler: &IpConditionCompiler<'_>,
    ) -> Result<Box<dyn RequestFilter>, String> {
        // Validate authored shape before clearing fields in the private runtime copy.
        if let Some(error) = config.check_fields(false) {
            return Err(error);
        }
        let refs = CompiledGeoIpRefs::from_http(config, compiler)
            .map_err(|_| "GeoIpLocation owning IP references are invalid".to_string())?;
        let mut config = config.clone();
        config.validate_and_init(false)?;
        CompiledGeoIpRefs::sanitize(&mut config);

        let parse_only = config.is_parse_only();
        let needs_remote_ip = parse_only || config.source_is_used(GeoRuleSource::RemoteIp);
        let needs_direct_facts = parse_only || config.source_needs_geo_facts(GeoRuleSource::DirectPeerIp);
        let needs_remote_facts = parse_only || config.source_needs_geo_facts(GeoRuleSource::RemoteIp);

        Ok(Box::new(GeoIpLocation {
            config,
            refs,
            needs_remote_ip,
            needs_direct_facts,
            needs_remote_facts,
        }))
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
            body: Some(DENY_BODY),
        }
    }
}

#[async_trait]
impl RequestFilter for GeoIpLocation {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if self.refs.has_refs() {
            session.ensure_config_data_view();
        }
        if !self.refs.available(session) {
            plugin_log.push_err("refs:unavailable fail-closed");
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
                RuntimeDecision::Rule(self.config.decide(&ctx, |side, group, rule, ip| {
                    self.refs.contains(session, side, group, rule, ip)
                }))
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
#[path = "../../../../tests/unit/plugins/http/geoip_location/plugin_test.rs"]
mod tests;
