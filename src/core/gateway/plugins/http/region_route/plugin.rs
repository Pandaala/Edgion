//! RegionRoute plugin implementation.
//!
//! Route rules are evaluated in order — first match wins, all miss → pass through.
//! When a key is configured but absent from the request, the plugin passes through
//! without modifying the upstream (fail-open, not fail-close).
//!
//! At request time, the plugin resolves the PluginMetaData chain from the global
//! `PluginMetaDataGatewayStore` (ArcSwap, lock-free):
//!   EP.serviceRegionRouteRef → ServiceRegionRoute PluginMetaData
//!     → globalRef → GlobalRegionRoute PluginMetaData
//!
//! No config is cached — every request sees the latest PluginMetaData.

use std::sync::Arc;

use async_trait::async_trait;

use crate::core::gateway::plugins::runtime::plugin_metadata::get_global_plugin_metadata_store;
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::common::KeyGet;
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{HashAlgorithm, RegionDef, RegionRouteConfig, RouteRule};
use crate::types::resources::plugin_metadata::{GlobalRegionRouteConfig, ServiceRegionRouteConfig};
use crate::types::ExternalJumpPreset;

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
            let n = u64::from_be_bytes(d[..8].try_into().expect("sha256>=8"));
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

    /// Resolve the PluginMetaData chain and return (global_config, service_config).
    /// Returns None if any link in the chain is broken.
    fn resolve_config(
        &self,
        log: &mut PluginLog,
    ) -> Option<(Arc<GlobalRegionRouteConfig>, Arc<ServiceRegionRouteConfig>)> {
        if self.service_ref_name.is_empty() {
            log.push("config invalid, pass; ");
            return None;
        }

        let store = get_global_plugin_metadata_store();

        let svc = match store.get_service_region_route_config_by_key(&self.service_lookup_key) {
            Some(s) => s,
            None => {
                log.push("config invalid, pass; ");
                return None;
            }
        };

        let global_ref = match &svc.global_ref {
            Some(r) => r,
            None => {
                log.push("config invalid, pass; ");
                return None;
            }
        };

        let global_ns = global_ref.namespace.as_deref().unwrap_or(&self.ep_namespace);
        let global_key = format!("{}/{}", global_ns, global_ref.name);
        let global = match store.get_global_region_route_config_by_key(&global_key) {
            Some(g) => g,
            None => {
                log.push("config invalid, pass; ");
                return None;
            }
        };

        Some((global, svc))
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

    // Region lookup (on borrowed GlobalRegionRouteConfig)
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
        // Service override takes priority over global failoverTo
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
            log.push("err:failoverTo invalid; ");
            return (idx, region);
        }

        match regions.iter().enumerate().find(|(_, r)| r.name == target_name) {
            Some((target_idx, target_region)) => {
                log.push(&format!("redirect {}→{}; ", region.name, target_region.name));
                (target_idx, target_region)
            }
            None => {
                log.push("err:failoverTo invalid; ");
                (idx, region)
            }
        }
    }

    // Route to region
    async fn route_to(
        &self,
        session: &mut dyn PluginSession,
        region: &RegionDef,
        global: &GlobalRegionRouteConfig,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        // myRegion check: pass through if this is the local region
        if !global.my_region.is_empty() && global.my_region == region.name {
            log.push("local, pass; ");
            return PluginRunningResult::GoodNext;
        }

        let (host, port) = region.parse_endpoint();

        let sni = session
            .header_value("Host")
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string();

        log.push(&format!("→{} {}:{}; ", region.name, host, port));

        session.set_external_jump(ExternalJumpPreset {
            domain: host.to_string(),
            port,
            use_tls: region.tls,
            sni,
        });

        PluginRunningResult::GoodNext
    }

    // Hash range logic
    async fn exec_hash_range<'a>(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        global: &'a GlobalRegionRouteConfig,
    ) -> Option<(usize, &'a RegionDef)> {
        let modulo = global.hash_calc.as_ref().map(|c| c.modulo).unwrap_or(1000);

        if let Some(ref hkg) = global.hash_key_get {
            if let Some((raw, src)) = self.extract_first(session, hkg).await {
                if let Ok(v) = raw.parse::<u16>() {
                    if v < modulo {
                        log.push(&format!("hash_pass={} from:{}; ", v, src.as_log_str()));
                        return Self::lookup_region(&global.regions, v);
                    }
                }
            }
        }

        let calc = match global.hash_calc.as_ref() {
            Some(c) => c,
            None => {
                log.push("no hashCalc, pass; ");
                return None;
            }
        };

        if global.key_get.is_empty() {
            log.push("no keyGet, pass; ");
            return None;
        }

        let (key, src) = match self.extract_first(session, &global.key_get).await {
            Some(v) => v,
            None => {
                log.push("key missing, pass; ");
                return None;
            }
        };

        let hash_val = compute_hash(&key, &calc.algorithm, calc.modulo);
        log.push(&format!("key={} hash={}; ", src.as_log_str(), hash_val));

        if !calc.write_back.is_empty() {
            let mut targets = String::new();
            for wb in &calc.write_back {
                let _ = session.key_set(wb, Some(hash_val.to_string()));
                if !targets.is_empty() {
                    targets.push(',');
                }
                targets.push_str(&wb.as_log_str());
            }
            log.push(&format!("wb→{}; ", targets));
        }

        let result = Self::lookup_region(&global.regions, hash_val);
        if result.is_none() {
            log.push(&format!("hash={} no_range, pass; ", hash_val));
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
        // Resolve PluginMetaData chain: ServiceRegionRoute → GlobalRegionRoute
        let (global, svc) = match self.resolve_config(log) {
            Some(pair) => pair,
            None => return PluginRunningResult::GoodNext,
        };

        // Use global's route_by_key_conf_match.match_map for RouteByKeyConfMatch rules
        let global_match_map = global
            .route_by_key_conf_match
            .as_ref()
            .filter(|kmc| !kmc.match_map.is_empty())
            .map(|kmc| &kmc.match_map);

        for rule in &global.route_rules {
            match rule {
                RouteRule::RouteByDirectRegion { key_from } => {
                    let (name, src) = match self.extract_first(session, key_from).await {
                        Some(v) => v,
                        None => continue,
                    };
                    match Self::find_region(&global.regions, &name) {
                        Some((idx, region)) => {
                            log.push(&format!("direct={}(from:{}); ", name, src.as_log_str()));
                            let (_, region) = Self::apply_redirect(&global.regions, idx, region, &svc, log);
                            return self.route_to(session, region, &global, log).await;
                        }
                        None => {
                            log.push(&format!("direct unknown '{}', fallthrough; ", name));
                            continue;
                        }
                    }
                }

                RouteRule::RouteByKeyConfMatch { match_map } => {
                    let (value, src) = match self.extract_first(session, &global.key_get).await {
                        Some(v) => v,
                        None => continue,
                    };
                    // Use global matchMap if available, otherwise inline rule matchMap
                    let effective_map = global_match_map.unwrap_or(match_map);
                    match effective_map.get(&value) {
                        Some(region_name) => match Self::find_region(&global.regions, region_name) {
                            Some((idx, region)) => {
                                log.push(&format!("match={}→{}; ", src.as_log_str(), region.name));
                                let (_, region) = Self::apply_redirect(&global.regions, idx, region, &svc, log);
                                return self.route_to(session, region, &global, log).await;
                            }
                            None => {
                                log.push(&format!("match={}→{} unknown; ", src.as_log_str(), region_name));
                                continue;
                            }
                        },
                        None => {
                            log.push(&format!("match={} no conf; ", src.as_log_str()));
                            continue;
                        }
                    }
                }

                RouteRule::RouteByHashRange => match self.exec_hash_range(session, log, &global).await {
                    Some((idx, region)) => {
                        let (_, region) = Self::apply_redirect(&global.regions, idx, region, &svc, log);
                        return self.route_to(session, region, &global, log).await;
                    }
                    None => return PluginRunningResult::GoodNext,
                },
            }
        }

        PluginRunningResult::GoodNext
    }
}
