//! RegionRoute plugin implementation.
//!
//! Route rules are evaluated in order — first match wins, all miss → pass through.
//! When a key is configured but absent from the request, the plugin passes through
//! without modifying the upstream (fail-open, not fail-close).
//!
//! At request time, the plugin resolves the PluginMetaData chain from the global
//! `PluginMetaDataGatewayStore` (ArcSwap, lock-free):
//!   EP.serviceRegionRouteRef → ServiceRegionRoute PluginMetaData
//!     → clusterRef → ClusterRegionRoute PluginMetaData
//!
//! No config is cached — every request sees the latest PluginMetaData.

use std::sync::Arc;

use async_trait::async_trait;

use crate::core::common::utils::net::host_without_port;
use crate::core::gateway::plugins::runtime::log::{la, sensitive};
use crate::core::gateway::plugins::runtime::plugin_metadata::get_global_plugin_metadata_store;
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::core::gateway::{ExternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::common::KeyGet;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{HashAlgorithm, RegionDef, RegionRouteConfig, RouteRule};
use edgion_resources::resources::plugin_metadata::{ClusterRegionRouteConfig, ServiceRegionRouteConfig};

// Hash function
fn compute_hash(key: &str, algorithm: &HashAlgorithm, modulo: u16) -> u16 {
    match algorithm {
        HashAlgorithm::Crc32 => {
            let n = crc32fast::hash(key.as_bytes()) as u64;
            (n % modulo as u64) as u16
        }
        HashAlgorithm::Sha256 => {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(key.as_bytes());
            let d = h.finalize();
            let n = u64::from_be_bytes(
                d[..8]
                    .try_into()
                    .expect("SHA-256 output is always 32 bytes; d[..8] is exactly 8"),
            );
            (n % modulo as u64) as u16
        }
    }
}

// Plugin
pub struct RegionRoute {
    name: String,
    /// Namespace of the parent EdgionPlugins resource.
    ep_namespace: String,
    /// Name of the ServiceRegionRoute PluginMetaData to look up.
    service_ref_name: String,
    /// Pre-computed "namespace/name" key for the service lookup.
    service_lookup_key: String,
}

/// Outcome of resolving the PluginMetaData reference chain.
///
/// `NotConfigured` (no `serviceRegionRouteRef`) is a legitimate no-op — the
/// plugin passes the request through. `BrokenRef` (a configured reference that
/// does not resolve) is a real misconfiguration and must fail closed: the sync
/// race is already gated by `ConfigClient::is_ready()` before listeners bind,
/// so an unresolved reference at request time is not eventual consistency.
enum ResolveOutcome {
    NotConfigured,
    BrokenRef,
    Resolved(Arc<ClusterRegionRouteConfig>, Arc<ServiceRegionRouteConfig>),
}

impl RegionRoute {
    pub fn create(ep_config: &RegionRouteConfig, ep_namespace: &str) -> Box<dyn RequestFilter> {
        let service_ref_name = ep_config.service_region_route_ref.clone().unwrap_or_default();
        let service_lookup_key = format!("{}/{}", ep_namespace, service_ref_name);

        Box::new(Self {
            name: "RegionRoute".to_string(),
            ep_namespace: ep_namespace.to_string(),
            service_ref_name,
            service_lookup_key,
        })
    }

    /// Resolve the PluginMetaData chain.
    /// Returns `NotConfigured` when no reference is set, `BrokenRef` when a
    /// configured reference does not resolve, `Resolved` on success.
    fn resolve_config(&self, log: &mut PluginLog) -> ResolveOutcome {
        if self.service_ref_name.is_empty() {
            log.push("no region-route configured, pass");
            return ResolveOutcome::NotConfigured;
        }

        let store = get_global_plugin_metadata_store();

        let svc = match store.get_service_region_route_config_by_key(&self.service_lookup_key) {
            Some(s) => s,
            None => {
                log.push_err("service region-route ref unresolved");
                return ResolveOutcome::BrokenRef;
            }
        };

        let cluster_ref = match &svc.cluster_ref {
            Some(r) => r,
            None => {
                log.push_err("service region-route missing clusterRef");
                return ResolveOutcome::BrokenRef;
            }
        };

        let cluster_ns = cluster_ref.namespace.as_deref().unwrap_or(&self.ep_namespace);
        let cluster_key = format!("{}/{}", cluster_ns, cluster_ref.name);
        let cluster = match store.get_cluster_region_route_config_by_key(&cluster_key) {
            Some(g) => g,
            None => {
                log.push_err("cluster region-route ref unresolved");
                return ResolveOutcome::BrokenRef;
            }
        };

        ResolveOutcome::Resolved(cluster, svc)
    }

    // Key extraction
    async fn extract_first<'a>(
        &self,
        session: &dyn PluginSession,
        sources: &'a [KeyGet],
    ) -> Option<(String, &'a KeyGet)> {
        for source in sources {
            if let Some(v) = session.key_get(source).await {
                if !v.is_empty() {
                    return Some((v, source));
                }
            }
        }
        None
    }

    // Region lookup (on borrowed ClusterRegionRouteConfig)
    fn lookup_region(regions: &[RegionDef], hash_val: u16) -> Option<(usize, &RegionDef)> {
        regions
            .iter()
            .enumerate()
            .find(|(_, r)| hash_val >= r.hash_range[0] && hash_val <= r.hash_range[1])
    }

    fn find_region<'a>(regions: &'a [RegionDef], name: &str) -> Option<(usize, &'a RegionDef)> {
        regions.iter().enumerate().find(|(_, r)| r.name == name)
    }

    // Failover redirect (inline, no pre-computation)
    /// Resolve effective failoverTo for a region, considering ServiceRegionRoute override.
    /// Returns the final (idx, region) after redirect. Only one level of redirect.
    fn apply_redirect<'a>(
        regions: &'a [RegionDef],
        idx: usize,
        region: &'a RegionDef,
        svc: &ServiceRegionRouteConfig,
        log: &mut PluginLog,
    ) -> (usize, &'a RegionDef) {
        // Service override takes priority over cluster failoverTo
        let effective = svc
            .regions
            .iter()
            .find(|r| r.name == region.name)
            .and_then(|r| r.failover_to.as_deref())
            .or(region.failover_to.as_deref());

        let target_name = match effective {
            Some(t) => t,
            None => return (idx, region),
        };

        if target_name == region.name {
            log.push_err("failover to invalid");
            return (idx, region);
        }

        match regions.iter().enumerate().find(|(_, r)| r.name == target_name) {
            Some((target_idx, target_region)) => {
                log.push_fmt(
                    "redirect {}→{}",
                    la![sensitive(&region.name), sensitive(&target_region.name)],
                );
                (target_idx, target_region)
            }
            None => {
                log.push_err("failover to invalid");
                (idx, region)
            }
        }
    }

    // Route to region
    async fn route_to(
        &self,
        session: &mut dyn PluginSession,
        region: &RegionDef,
        cluster: &ClusterRegionRouteConfig,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        // myRegion check: pass through if this is the local region
        if !cluster.my_region.is_empty() && cluster.my_region == region.name {
            log.push("local, pass");
            return PluginRunningResult::GoodNext;
        }

        let (host, port) = region.parse_endpoint();

        // IPv6-safe Host port stripping: a naive split(':') would mangle a
        // bracketed IPv6 literal (`[::1]:8080` -> `[`) into a corrupt SNI.
        let sni = host_without_port(&session.header_value("Host").unwrap_or_default()).to_string();

        log.push("proxy pass");
        log.push_fmt(
            "→{} {}:{}",
            la![sensitive(&region.name), sensitive(host), sensitive(&port.to_string())],
        );

        match session.set_upstream_jump(UpstreamJumpTarget::External(ExternalJumpPreset {
            domain: host.to_string(),
            port,
            use_tls: region.tls,
            sni,
            // Cross-region gateway-to-gateway forwarding: the peer gateway runs
            // its own path normalization, so forward raw to avoid double work.
            preserve_raw_path: true,
        })) {
            SetJumpOutcome::Set => { /* "proxy pass" already logged above */ }
            SetJumpOutcome::Rejected => {
                log.push("jump locked, rejected");
            }
        }

        PluginRunningResult::GoodNext
    }

    // Hash range logic
    async fn exec_hash_range<'a>(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        cluster: &'a ClusterRegionRouteConfig,
    ) -> Option<(usize, &'a RegionDef)> {
        let modulo = cluster.hash_calc.as_ref().map(|c| c.modulo).unwrap_or(1000);

        if let Some(ref hkg) = cluster.hash_key_get {
            if let Some((raw, src)) = self.extract_first(session, hkg).await {
                if let Ok(v) = raw.parse::<u16>() {
                    if v < modulo {
                        let src_s = src.as_log_str();
                        log.push_fmt(
                            "hash_pass={} from:{}",
                            la![sensitive(&v.to_string()), sensitive(&src_s)],
                        );
                        return Self::lookup_region(&cluster.regions, v);
                    }
                }
            }
        }

        let calc = match cluster.hash_calc.as_ref() {
            Some(c) => c,
            None => {
                log.push("no hashCalc, pass");
                return None;
            }
        };

        if cluster.key_get.is_empty() {
            log.push("no keyGet, pass");
            return None;
        }

        let (key, src) = match self.extract_first(session, &cluster.key_get).await {
            Some(v) => v,
            None => {
                log.push("key missing, pass");
                return None;
            }
        };

        let hash_val = compute_hash(&key, &calc.algorithm, calc.modulo);
        let src_s = src.as_log_str();
        log.push_fmt(
            "key={} hash={}",
            la![sensitive(&src_s), sensitive(&hash_val.to_string())],
        );

        if !calc.write_back.is_empty() {
            let mut targets = String::new();
            for wb in &calc.write_back {
                let _ = session.key_set(wb, Some(hash_val.to_string()));
                if !targets.is_empty() {
                    targets.push(',');
                }
                targets.push_str(&wb.as_log_str());
            }
            log.push_fmt("wb→{}", la![sensitive(&targets)]);
        }

        let result = Self::lookup_region(&cluster.regions, hash_val);
        if result.is_none() {
            log.push_fmt("hash={} no_range, pass", la![sensitive(&hash_val.to_string())]);
        }
        result
    }
}

// RequestFilter
#[async_trait]
impl RequestFilter for RegionRoute {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Resolve PluginMetaData chain: ServiceRegionRoute → ClusterRegionRoute
        let (cluster, svc) = match self.resolve_config(log) {
            ResolveOutcome::Resolved(cluster, svc) => (cluster, svc),
            // No reference configured — legitimate no-op, pass through.
            ResolveOutcome::NotConfigured => return PluginRunningResult::GoodNext,
            // A configured reference did not resolve — fail closed.
            ResolveOutcome::BrokenRef => {
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: Some(r#"{"message":"region-route reference unresolved"}"#.to_string()),
                };
            }
        };

        // Use cluster's route_by_key_conf_match.match_map for RouteByKeyConfMatch rules
        let cluster_match_map = cluster
            .route_by_key_conf_match
            .as_ref()
            .filter(|kmc| !kmc.match_map.is_empty())
            .map(|kmc| &kmc.match_map);

        for rule in &cluster.route_rules {
            match rule {
                RouteRule::RouteByDirectRegion { key_from } => {
                    let (name, src) = match self.extract_first(session, key_from).await {
                        Some(v) => v,
                        None => continue,
                    };
                    match Self::find_region(&cluster.regions, &name) {
                        Some((idx, region)) => {
                            let src_s = src.as_log_str();
                            log.push_fmt("direct={}(from:{})", la![sensitive(&name), sensitive(&src_s)]);
                            let (_, region) = Self::apply_redirect(&cluster.regions, idx, region, &svc, log);
                            return self.route_to(session, region, &cluster, log).await;
                        }
                        None => {
                            log.push_fmt("direct unknown '{}', fallthrough", la![sensitive(&name)]);
                            continue;
                        }
                    }
                }

                RouteRule::RouteByKeyConfMatch { match_map } => {
                    let (value, src) = match self.extract_first(session, &cluster.key_get).await {
                        Some(v) => v,
                        None => continue,
                    };
                    // Use cluster matchMap if available, otherwise inline rule matchMap
                    let effective_map = cluster_match_map.unwrap_or(match_map);
                    match effective_map.get(&value) {
                        Some(region_name) => match Self::find_region(&cluster.regions, region_name) {
                            Some((idx, region)) => {
                                let src_s = src.as_log_str();
                                log.push_fmt("match={}→{}", la![sensitive(&src_s), sensitive(&region.name)]);
                                let (_, region) = Self::apply_redirect(&cluster.regions, idx, region, &svc, log);
                                return self.route_to(session, region, &cluster, log).await;
                            }
                            None => {
                                let src_s = src.as_log_str();
                                log.push_fmt("match={}→{} unknown", la![sensitive(&src_s), sensitive(region_name)]);
                                continue;
                            }
                        },
                        None => {
                            let src_s = src.as_log_str();
                            log.push_fmt("match={} no conf", la![sensitive(&src_s)]);
                            continue;
                        }
                    }
                }

                RouteRule::RouteByHashRange => match self.exec_hash_range(session, log, &cluster).await {
                    Some((idx, region)) => {
                        let (_, region) = Self::apply_redirect(&cluster.regions, idx, region, &svc, log);
                        return self.route_to(session, region, &cluster, log).await;
                    }
                    None => return PluginRunningResult::GoodNext,
                },
            }
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_config_distinguishes_not_configured_from_broken_ref() {
        // service_ref_name empty -> NotConfigured (legitimate no-op).
        let not_configured = RegionRoute {
            name: "RegionRoute".to_string(),
            ep_namespace: "default".to_string(),
            service_ref_name: String::new(),
            service_lookup_key: "default/".to_string(),
        };
        let mut log = PluginLog::with_alias("RegionRoute", None);
        assert!(matches!(
            not_configured.resolve_config(&mut log),
            ResolveOutcome::NotConfigured
        ));

        // service_ref_name set but nothing in the global store -> BrokenRef.
        // The global PM store is process-wide; use a key no real config or sibling test would ever register, so this stays BrokenRef.
        let broken = RegionRoute {
            name: "RegionRoute".to_string(),
            ep_namespace: "default".to_string(),
            service_ref_name: "region-route-unit-test-broken-ref-do-not-register".to_string(),
            service_lookup_key: "default/region-route-unit-test-broken-ref-do-not-register".to_string(),
        };
        let mut log2 = PluginLog::with_alias("RegionRoute", None);
        assert!(matches!(broken.resolve_config(&mut log2), ResolveOutcome::BrokenRef));
        // The Resolved path requires populating the global PM store and is covered by integration tests.
    }
}
