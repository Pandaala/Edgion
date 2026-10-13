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
use crate::plugins::common::geo_ip_refs::CompiledGeoIpRefs;
use crate::plugins::runtime::edgion_config_data::capture_global_edgion_config_data_snapshot;
use crate::plugins::stream::stream_plugin_trait::{StreamContext, StreamPlugin, StreamPluginResult};
use edgion_resources::refs::ip_list_fields::BoundGeoIpEntry;
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

pub struct GeoIpLocationStreamPlugin {
    config: GeoIpLocationConfig,
    needs_facts: bool,
    refs: CompiledGeoIpRefs,
}

impl GeoIpLocationStreamPlugin {
    /// Bind actual owning CIDR references before normalizing the private geographic
    /// rule copy. L4 rejects RemoteIp through the shared admission validator.
    pub fn from_bound(entry: BoundGeoIpEntry<'_>) -> Result<Self, String> {
        if let Some(error) = entry.config().check_fields(true) {
            return Err(error);
        }
        let authored = entry.config();
        let refs = CompiledGeoIpRefs::from_stream(entry)
            .map_err(|_| "GeoIpLocation owning IP references are invalid".to_string())?;
        let mut config = authored.clone();
        config
            .validate_and_init(true)
            .map_err(|e| format!("GeoIpLocation config validation failed: {e}"))?;
        let needs_facts = config.source_needs_geo_facts(GeoRuleSource::DirectPeerIp);
        CompiledGeoIpRefs::sanitize(&mut config);
        Ok(Self {
            config,
            needs_facts,
            refs,
        })
    }
}

#[async_trait]
impl StreamPlugin for GeoIpLocationStreamPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        let view = self.refs.has_refs().then(capture_global_edgion_config_data_snapshot);
        if view.as_ref().is_some_and(|view| !self.refs.available_in(view)) {
            record_geoip_denied(ctx.layer, "ref_unavailable");
            return StreamPluginResult::Deny("geo:deny:ref_unavailable".into());
        }
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

        match self.config.decide(&rule_ctx, |side, group, rule, ip| {
            view.as_ref()
                .is_some_and(|view| self.refs.contains_in(view, side, group, rule, ip))
        }) {
            GeoDecision::Allow => StreamPluginResult::Allow,
            GeoDecision::Deny => {
                // The documented `reason` label enum has no distinct bucket for
                // "defaultAction: Deny with nothing configured to explain why" —
                // fold that (rare, degenerate) case into `deny_country` too
                // rather than adding an undeclared fourth reason value.
                let reason = if self.config.denies(&rule_ctx, |side, group, rule, ip| {
                    view.as_ref()
                        .is_some_and(|view| self.refs.contains_in(view, side, group, rule, ip))
                }) {
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
#[path = "../../../../tests/unit/plugins/stream/geoip_location/plugin_test.rs"]
mod tests;
