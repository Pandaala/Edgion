//! IP Restriction plugin for TLS route stage (Stage 2, post-handshake).
//!
//! Uses the HTTP-layer `IpRestrictionConfig` (with protocol-level `message`/`status` support),
//! distinct from the L4 `StreamIpRestriction` which uses `ConnectionIpRestrictionRules`.
//! Applied post-TLS handshake, so it has access to SNI-routed context.

use async_trait::async_trait;

use super::tls_route_plugin_trait::{TlsRouteContext, TlsRoutePlugin};
use crate::plugins::runtime::edgion_config_data::{resolve_ip_list_matchers, resolve_ip_list_ref_keys};
use crate::plugins::stream::stream_plugin_trait::access_decision_to_stream_result;
use crate::plugins::stream::StreamPluginResult;
use edgion_resources::matcher::ip_radix_tree::IpRadixMatcher;
use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;

const PLUGIN_NAME: &str = "TlsRouteIpRestriction";

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
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_tls_route(&self, ctx: &TlsRouteContext) -> StreamPluginResult {
        if self.init_failed {
            return StreamPluginResult::Deny("ip init fail".to_string());
        }
        let deny_arcs = resolve_ip_list_matchers(&self.deny_ref_keys);
        let allow_arcs = resolve_ip_list_matchers(&self.allow_ref_keys);
        let deny_refs: Vec<&IpRadixMatcher> = deny_arcs.iter().map(|a| a.as_ref()).collect();
        let allow_refs: Vec<&IpRadixMatcher> = allow_arcs.iter().map(|a| a.as_ref()).collect();
        access_decision_to_stream_result(
            self.config
                .check_ip_access_with_refs(&ctx.client_ip, &deny_refs, &allow_refs),
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

    fn ctx(ip: &str) -> TlsRouteContext {
        TlsRouteContext {
            client_ip: ip.parse::<IpAddr>().unwrap(),
            listener_port: 443,
            sni: "test.example.com".to_string(),
            tls_id: Some("t".to_string()),
            rsc: crate::Rsc::new("TLSRoute", "default", "r"),
            is_mtls: false,
            log: crate::observe::logs::LogBuffer::new(),
        }
    }

    #[tokio::test]
    async fn deny_refs_block_referenced_ip() {
        let ns = "tls-ipr-denyref-unit";
        let spec = EdgionConfigDataSpec {
            enable: true,
            active: None,
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
            }]),
            ip_source: IpSource::ClientIp,
            message: None,
            status: 403,
            allow_matcher: None,
            deny_matcher: None,
        };
        let plugin = TlsRouteIpRestriction::new(&config, ns);
        assert!(matches!(
            plugin.on_tls_route(&ctx("9.9.9.9")).await,
            StreamPluginResult::Deny(_)
        ));
        assert!(matches!(
            plugin.on_tls_route(&ctx("8.8.8.8")).await,
            StreamPluginResult::Allow
        ));
    }
}
