//! IP Restriction plugin evaluated after TLSRoute matching.
//!
//! Uses the HTTP-layer `IpRestrictionConfig` (with protocol-level `message`/`status` support),
//! distinct from the L4 `StreamIpRestriction` which uses `ConnectionIpRestrictionRules`.
//! Terminated TLS runs post-handshake; passthrough runs from inspected
//! ClientHello/SNI context before forwarding the end-to-end handshake.

use async_trait::async_trait;
use edgion_resources::refs::ip_list_fields::HttpIpListEntry;
use edgion_resources::refs::ReferenceError;
use smallvec::SmallVec;

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::plugins::runtime::edgion_config_data::{
    capture_global_edgion_config_data_snapshot_for_refs, resolve_bound_ip_list_keys,
};
use crate::plugins::stream::stream_plugin_trait::access_decision_to_stream_result;
use crate::plugins::stream::StreamPluginResult;
use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use edgion_resources::resources::edgion_plugins::{IpRestrictionConfig, IpSource};

pub struct TlsRouteIpRestriction {
    config: IpRestrictionConfig,
    init_failed: bool,
    allow_ref_keys: Vec<String>,
    deny_ref_keys: Vec<String>,
}

impl TlsRouteIpRestriction {
    pub(crate) fn from_bound(entry: HttpIpListEntry<'_>) -> Result<Self, ReferenceError> {
        let (allow_ref_keys, deny_ref_keys) = resolve_bound_ip_list_keys(entry.references())?;
        let config = entry.config();
        let mut config = config.clone();
        let init_failed = if let Err(e) = config.validate_and_init() {
            tracing::error!(
                error = %e,
                label = "TLS route plugin",
                "Failed to initialize IpRestrictionConfig; all connections will be denied",
            );
            true
        } else {
            false
        };
        // Loud signal: a skipped inline deny entry is a blacklist hole (fail-open direction).
        if let Some(deny) = config.deny.as_ref() {
            if !edgion_resources::matcher::ip_cidr_helpers::collect_entry_errors(deny).is_empty() {
                crate::observe::metrics::record_ip_deny_entries_skipped("tls_route");
            }
        }
        Ok(Self {
            config,
            init_failed,
            allow_ref_keys,
            deny_ref_keys,
        })
    }
}

#[async_trait]
impl TlsRoutePlugin for TlsRouteIpRestriction {
    async fn on_tls_route(&self, ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
        if self.init_failed {
            return StreamPluginResult::Deny("ip init fail".to_string());
        }
        let snapshot = capture_global_edgion_config_data_snapshot_for_refs(&self.deny_ref_keys, &self.allow_ref_keys);
        let mut deny_refs: SmallVec<[&IpRadixMatcher; 4]> = SmallVec::new();
        let mut allow_refs: SmallVec<[&IpRadixMatcher; 4]> = SmallVec::new();
        if let Some(snapshot) = snapshot.as_ref() {
            snapshot.append_ip_list_matchers(&self.deny_ref_keys, &mut deny_refs);
            snapshot.append_ip_list_matchers(&self.allow_ref_keys, &mut allow_refs);
        }
        let ip = match self.config.ip_source {
            IpSource::RemoteIp => &ctx.remote_ip,
            IpSource::DirectPeerIp => &ctx.direct_peer_ip,
        };
        access_decision_to_stream_result(
            self.config.check_ip_access_with_refs(ip, &deny_refs, &allow_refs),
            || {},
        )
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/stream/tls_route/ip_restriction_test.rs"]
mod tests;
