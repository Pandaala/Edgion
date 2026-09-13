//! IP Restriction plugin evaluated after TLSRoute matching.
//!
//! Uses the HTTP-layer `IpRestrictionConfig` (with protocol-level `message`/`status` support),
//! distinct from the L4 `StreamIpRestriction` which uses `ConnectionIpRestrictionRules`.
//! Terminated TLS runs post-handshake; passthrough runs from inspected
//! ClientHello/SNI context before forwarding the end-to-end handshake.

use async_trait::async_trait;
use smallvec::SmallVec;

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::plugins::runtime::edgion_config_data::{
    capture_global_edgion_config_data_snapshot_for_refs, resolve_ip_list_ref_keys,
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
    pub fn new(config: &IpRestrictionConfig, namespace: &str) -> Self {
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
        let allow_ref_keys = resolve_ip_list_ref_keys(config.allow_refs.as_ref(), namespace);
        let deny_ref_keys = resolve_ip_list_ref_keys(config.deny_refs.as_ref(), namespace);
        Self {
            config,
            init_failed,
            allow_ref_keys,
            deny_ref_keys,
        }
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
mod tests {
    use super::super::tls_route_plugin_trait::TlsRouteContext;
    use super::*;
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigData, EdgionConfigDataRef, EdgionConfigDataSpec, IpGroup, IpListData,
    };
    use edgion_resources::resources::edgion_plugins::{DefaultAction, IpSource};
    use std::net::IpAddr;
    use std::sync::Arc;

    fn ctx<'a>(ip: &str, rsc: &'a crate::Rsc) -> TlsRouteContext<'a> {
        ctx_with_ips(ip, ip, rsc)
    }

    fn ctx_with_ips<'a>(direct_peer_ip: &str, remote_ip: &str, rsc: &'a crate::Rsc) -> TlsRouteContext<'a> {
        TlsRouteContext {
            direct_peer_ip: direct_peer_ip.parse::<IpAddr>().unwrap(),
            remote_ip: remote_ip.parse::<IpAddr>().unwrap(),
            listener_port: 443,
            sni: "test.example.com",
            tls_id: Some("t"),
            rsc,
            is_mtls: false,
        }
    }

    #[tokio::test]
    async fn source_selector_distinguishes_direct_peer_and_remote_ip() {
        let base = IpRestrictionConfig {
            allow: None,
            deny: Some(vec![IpGroup {
                name: "blocked".into(),
                description: None,
                cidrs: vec!["10.0.0.7/32".into()],
            }]),
            default_action: DefaultAction::Allow,
            allow_refs: None,
            deny_refs: None,
            ip_source: IpSource::DirectPeerIp,
            message: None,
            status: 403,
            allow_matcher: None,
            deny_matcher: None,
        };
        let rsc = crate::Rsc::new("TLSRoute", "default", "r");
        let addresses = ctx_with_ips("10.0.0.7", "203.0.113.9", &rsc);
        let direct = TlsRouteIpRestriction::new(&base, "default");
        assert!(matches!(
            direct.on_tls_route(&addresses).await,
            StreamPluginResult::Deny(_)
        ));

        let mut remote_config = base;
        remote_config.ip_source = IpSource::RemoteIp;
        let remote = TlsRouteIpRestriction::new(&remote_config, "default");
        assert!(matches!(
            remote.on_tls_route(&addresses).await,
            StreamPluginResult::Allow
        ));
    }

    #[tokio::test]
    async fn deny_refs_block_referenced_ip() {
        let ns = "tls-ipr-denyref-unit";
        let spec = EdgionConfigDataSpec {
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "blk".into(),
                    description: None,
                    cidrs: vec!["9.9.9.9/32".into()],
                }],
            }),
        };
        let mut ed = EdgionConfigData::new("blk", spec);
        ed.metadata.namespace = Some(ns.to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(&format!("{ns}/blk"), Arc::new(ed));

        let config = IpRestrictionConfig {
            allow: None,
            deny: None,
            default_action: DefaultAction::Allow,
            allow_refs: None,
            deny_refs: Some(vec![EdgionConfigDataRef {
                name: "blk".into(),
                namespace: Some(ns.into()),
                ..Default::default()
            }]),
            ip_source: IpSource::RemoteIp,
            message: None,
            status: 403,
            allow_matcher: None,
            deny_matcher: None,
        };
        let plugin = TlsRouteIpRestriction::new(&config, ns);
        let rsc = crate::Rsc::new("TLSRoute", "default", "r");
        assert!(matches!(
            plugin.on_tls_route(&ctx("9.9.9.9", &rsc)).await,
            StreamPluginResult::Deny(_)
        ));
        assert!(matches!(
            plugin.on_tls_route(&ctx("8.8.8.8", &rsc)).await,
            StreamPluginResult::Allow
        ));
    }

    #[tokio::test]
    async fn tls_route_denied_allow_ref_contributes_no_key() {
        use edgion_resources::resources::edgion_config_data::EdgionConfigDataRef;
        let ns = "tls-ipr-ref-denied-unit";
        let config = IpRestrictionConfig {
            allow: None,
            deny: None,
            default_action: DefaultAction::Allow,
            allow_refs: Some(vec![EdgionConfigDataRef {
                name: "corp".into(),
                namespace: Some(ns.into()),
                ref_denied: Some(edgion_resources::resources::common::RefDenied {
                    target_namespace: ns.into(),
                    target_name: "corp".into(),
                    reason: None,
                }),
            }]),
            deny_refs: None,
            ip_source: IpSource::RemoteIp,
            message: None,
            status: 403,
            allow_matcher: None,
            deny_matcher: None,
        };
        let plugin = TlsRouteIpRestriction::new(&config, ns);
        assert!(
            plugin.allow_ref_keys.is_empty(),
            "denied allow_ref must yield no key in TLS route"
        );
        // Evaluate path: whitelist mode + zero allow matchers → deny-all (fail-closed).
        // Empty key lists keep the snapshot absent, so on_tls_route reaches
        // check_ip_access_with_refs with empty ref slices — a whitelist miss → Deny.
        let rsc = crate::Rsc::new("TLSRoute", "default", "r");
        assert!(
            matches!(
                plugin.on_tls_route(&ctx("8.8.8.8", &rsc)).await,
                StreamPluginResult::Deny(_)
            ),
            "denied allow_ref → empty allow matchers → non-matching IP must be DENIED (deny-all fail-closed)",
        );
    }
}
