//! RegionRoute plugin implementation.
//!
//! See [`RegionRouteConfig`] for the full routing model and field semantics.
//! Route rules are evaluated in order — first match wins, all miss → pass through.
//! When a key is configured but absent from the request, the plugin passes through
//! without modifying the upstream (fail-open, not fail-close).

use std::cmp::Ordering;
use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::common::{KeyGet, KeySet};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{
    HashAlgorithm, OnAllDown, RegionDef, RegionRouteConfig, RouteRule,
};
use crate::types::ExternalJumpPreset;

// ── Hash function ─────────────────────────────────────────────────────────────

/// Compute hash of `key` using the given algorithm, return value in [0, modulo).
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

// ── Health table ──────────────────────────────────────────────────────────────

/// Per-region health state, stored as f32 bits in AtomicU32 for lock-free hot-path reads.
///
/// Probe result: 1.0 = DNS returned ≥1 IP, 0.0 = DNS empty or failed.
/// Initialised to 1.0 so traffic flows normally before the first probe fires.
struct RegionHealthTable {
    ratios: Vec<AtomicU32>,
}

impl RegionHealthTable {
    fn new(count: usize) -> Self {
        let ratios = (0..count).map(|_| AtomicU32::new(1.0f32.to_bits())).collect();
        Self { ratios }
    }

    fn store(&self, idx: usize, ratio: f32) {
        if let Some(cell) = self.ratios.get(idx) {
            cell.store(ratio.to_bits(), AtomicOrdering::Relaxed);
        }
    }

    fn load(&self, idx: usize) -> f32 {
        self.ratios
            .get(idx)
            .map(|c| f32::from_bits(c.load(AtomicOrdering::Relaxed)))
            .unwrap_or(0.0)
    }
}

// ── Plugin struct ─────────────────────────────────────────────────────────────

/// Per-region static redirect state, computed at creation time.
#[derive(Debug, Clone)]
enum RegionRedirectState {
    /// No failoverTo configured.
    NoRedirect,
    /// Valid redirect — routes to this region index.
    Valid(usize),
    /// failoverTo was configured but is invalid (target not found, self-ref, or cycle).
    /// Noted in per-request pluginLog; plugin otherwise operates normally.
    Invalid,
}

pub struct RegionRoute {
    name: String,
    config: Arc<RegionRouteConfig>,
    health: Arc<RegionHealthTable>,
    /// Cached at creation to avoid Arc → Option traversal on every hot-path call.
    healthy_threshold: f32,
    /// Index of myRegion in config.regions. None = feature disabled.
    my_region_idx: Option<usize>,
    /// Per-region redirect state. Invalid failoverTo entries are surfaced via pluginLog.
    region_redirect: Vec<RegionRedirectState>,
    /// Keeps the probe task alive for the lifetime of this plugin instance.
    _probe_handle: Option<JoinHandle<()>>,
}

impl RegionRoute {
    pub fn create(config: &RegionRouteConfig) -> Box<dyn RequestFilter> {
        let mut validated = config.clone();
        validated.validate();

        let n = validated.regions.len();
        let healthy_threshold = validated
            .auto_failover
            .as_ref()
            .filter(|f| f.enabled)
            .map(|f| f.healthy_threshold)
            .unwrap_or(0.0);
        // myRegion: if configured but not found in regions, silently disable the feature.
        let my_region_idx = if validated.my_region.is_empty() {
            None
        } else {
            validated.regions.iter().position(|r| r.name == validated.my_region)
        };

        // failoverTo: resolve each region's redirect target.
        // Invalid config (target not found, self-ref, cycle) → RegionRedirectState::Invalid.
        // Invalid state is surfaced per-request via pluginLog; plugin works normally otherwise.
        let region_redirect: Vec<RegionRedirectState> = validated.regions.iter().enumerate().map(|(i, region)| {
            let target = match region.failover_to.as_deref() {
                Some(t) => t,
                None => return RegionRedirectState::NoRedirect,
            };
            // Self-reference check
            if target == region.name {
                return RegionRedirectState::Invalid;
            }
            // Target must exist in regions
            let target_idx = match validated.regions.iter().position(|r| r.name == target) {
                Some(idx) => idx,
                None => return RegionRedirectState::Invalid,
            };
            // Cycle detection (n≤5, simple walk)
            let mut seen = vec![i];
            let mut cur = target_idx;
            for _ in 0..validated.regions.len() {
                if seen.contains(&cur) {
                    return RegionRedirectState::Invalid;
                }
                seen.push(cur);
                match validated.regions[cur].failover_to.as_deref()
                    .and_then(|t| validated.regions.iter().position(|r| r.name == t))
                {
                    Some(next) => cur = next,
                    None => break,
                }
            }
            RegionRedirectState::Valid(target_idx)
        }).collect();

        let config = Arc::new(validated);
        let health = Arc::new(RegionHealthTable::new(n));

        let probe_handle = match config.auto_failover.as_ref() {
            Some(f) if f.enabled => {
                let c = config.clone();
                let h = health.clone();
                Some(tokio::spawn(probe_loop(c, h)))
            }
            _ => None,
        };

        Box::new(Self {
            name: "RegionRoute".to_string(),
            config,
            health,
            healthy_threshold,
            my_region_idx,
            region_redirect,
            _probe_handle: probe_handle,
        })
    }

    // ── Key extraction ────────────────────────────────────────────────────────

    /// Extract the first non-empty value from an ordered list of KeyGet sources.
    /// Returns `(value, source)` so callers can include the source in log output.
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

    // ── Region lookup ─────────────────────────────────────────────────────────

    fn lookup_region(&self, hash_val: u16) -> Option<(usize, &RegionDef)> {
        self.config
            .regions
            .iter()
            .enumerate()
            .find(|(_, r)| hash_val >= r.hash_range[0] && hash_val <= r.hash_range[1])
    }

    fn find_region(&self, name: &str) -> Option<(usize, &RegionDef)> {
        self.config.regions.iter().enumerate().find(|(_, r)| r.name == name)
    }

    // ── Health check ──────────────────────────────────────────────────────────

    #[inline]
    fn is_healthy(&self, idx: usize) -> bool {
        self.health.load(idx) >= self.healthy_threshold
    }

    // ── Routing ───────────────────────────────────────────────────────────────

    /// Apply static failoverTo redirect for the given region index.
    /// Returns the final (idx, region) after at most one level of redirect.
    /// Logs the outcome so ops can diagnose misconfigured failoverTo from access log alone.
    fn apply_static_redirect<'a>(&'a self, idx: usize, region: &'a RegionDef, log: &mut PluginLog) -> (usize, &'a RegionDef) {
        match self.region_redirect.get(idx) {
            Some(RegionRedirectState::Valid(target_idx)) => {
                if let Some(target_region) = self.config.regions.get(*target_idx) {
                    log.push(&format!("redirect {}→{}; ", region.name, target_region.name));
                    return (*target_idx, target_region);
                }
                // target_idx was resolved from config.regions at construction — this is unreachable.
                unreachable!("region_redirect index {} out of bounds", target_idx)
            }
            Some(RegionRedirectState::Invalid) => {
                // failoverTo was configured but invalid (target not found, self-ref, or cycle).
                // Log as a config error in pluginLog so it appears in the access log.
                log.push("err:failoverTo invalid; ");
                (idx, region)
            }
            Some(RegionRedirectState::NoRedirect) | None => (idx, region),
        }
    }

    /// Set `ExternalJumpPreset` so Pingora DNS-resolves the backend_endpoint host to an IP,
    /// while keeping the original request's SNI and Host header unchanged.
    async fn route_to(
        &self,
        session: &mut dyn PluginSession,
        region: &RegionDef,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        // myRegion check: pass through if this is the local region.
        // my_region_idx is always a valid index into config.regions (resolved at construction).
        if let Some(my_idx) = self.my_region_idx {
            if self.config.regions[my_idx].name == region.name {
                log.push("local, pass; ");
                return PluginRunningResult::GoodNext;
            }
        }

        let (host, port) = region.parse_endpoint();

        // Use the incoming Host header as upstream SNI (strip optional ":port").
        // This preserves the original TLS server name — only the IP changes.
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

    /// Choose the healthiest alternative region and route there.
    async fn do_failover(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        skip_idx: Option<usize>,
    ) -> PluginRunningResult {
        let failover = match self.config.auto_failover.as_ref() {
            Some(f) if f.enabled => f,
            _ => {
                log.push("failover disabled, pass; ");
                return PluginRunningResult::GoodNext;
            }
        };

        let best = self
            .config
            .regions
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != skip_idx)
            .filter_map(|(i, r)| {
                let ratio = self.health.load(i);
                if ratio >= self.healthy_threshold {
                    Some((i, r, ratio))
                } else {
                    None
                }
            })
            .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(Ordering::Equal));

        match best {
            Some((_, region, ratio)) => {
                log.push(&format!("failover→{} ({:.0}%); ", region.name, ratio * 100.0));
                if let Some(ref h) = failover.notify_header {
                    let _ = session.set_response_header(h, &region.name);
                }
                self.route_to(session, region, log).await
            }
            None => {
                log.push("all_regions_down; ");
                match failover.on_all_regions_down {
                    OnAllDown::Pass => PluginRunningResult::GoodNext,
                    OnAllDown::Reject => PluginRunningResult::ErrResponse {
                        status: 503,
                        body: Some(r#"{"message":"Service Unavailable"}"#.to_string()),
                    },
                }
            }
        }
    }

    // ── RouteByHashRange logic ────────────────────────────────────────────────

    /// Execute RouteByHashRange: try hashKeyGet passthrough first, then hashCalc compute.
    ///
    /// Returns `Some((idx, region))` when a region is found, or `None` for pass-through.
    /// This rule is terminal — callers must return GoodNext when None is returned.
    async fn exec_hash_range(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Option<(usize, &RegionDef)> {
        let modulo = self.config.hash_calc.as_ref().map(|c| c.modulo).unwrap_or(1000);

        // Step 1: try hashKeyGet passthrough
        if let Some(ref hkg) = self.config.hash_key_get {
            if let Some((raw, src)) = self.extract_first(session, hkg).await {
                if let Ok(v) = raw.parse::<u16>() {
                    if v < modulo {
                        log.push(&format!("hash_pass={} from:{}; ", v, src.as_log_str()));
                        return self.lookup_region(v);
                    }
                    // out of range → fall through to compute
                }
            }
        }

        // Step 2: try hashCalc compute
        let calc = match self.config.hash_calc.as_ref() {
            Some(c) => c,
            None => {
                log.push("no hashCalc, pass; ");
                return None; // terminal pass through
            }
        };

        // Extract key from shared keyGet
        if self.config.key_get.is_empty() {
            log.push("no keyGet, pass; ");
            return None;
        }

        let (key, src) = match self.extract_first(session, &self.config.key_get).await {
            Some(v) => v,
            None => {
                log.push("key missing, pass; ");
                return None;
            }
        };

        // Compute hash (log SOURCE only, not value — key may contain sensitive data)
        let hash_val = compute_hash(&key, &calc.algorithm, calc.modulo);
        log.push(&format!("key={} hash={}; ", src.as_log_str(), hash_val));

        // Write back to configured targets
        self.do_write_back(session, &calc.write_back, hash_val, log);

        self.lookup_region(hash_val)
    }

    /// Write the computed hash value to all configured write-back targets.
    fn do_write_back(&self, session: &mut dyn PluginSession, write_back: &[KeySet], hash_val: u16, log: &mut PluginLog) {
        if write_back.is_empty() {
            return;
        }
        let mut targets = String::new();
        for wb in write_back {
            let _ = session.key_set(wb, Some(hash_val.to_string()));
            if !targets.is_empty() {
                targets.push(',');
            }
            targets.push_str(&wb.as_log_str());
        }
        log.push(&format!("wb→{}; ", targets));
    }
}

// ── Drop ──────────────────────────────────────────────────────────────────────

impl Drop for RegionRoute {
    fn drop(&mut self) {
        // Tokio drops a JoinHandle by detaching (task keeps running), not by cancelling.
        // Abort explicitly so the probe loop stops when this plugin instance is replaced.
        if let Some(ref handle) = self._probe_handle {
            handle.abort();
        }
    }
}

// ── RequestFilter impl ────────────────────────────────────────────────────────

#[async_trait]
impl RequestFilter for RegionRoute {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if self.config.get_validation_error().is_some() {
            log.push("config invalid, pass; ");
            return PluginRunningResult::GoodNext;
        }

        for rule in &self.config.route_rules {
            match rule {
                RouteRule::RouteByDirectRegion { key_from } => {
                    let extracted = self.extract_first(session, key_from).await;
                    let (name, src) = match extracted {
                        Some(v) => v,
                        None => continue, // key absent → try next rule
                    };
                    match self.find_region(&name) {
                        Some((idx, region)) => {
                            log.push(&format!("direct={}(from:{}); ", name, src.as_log_str()));
                            let (idx, region) = self.apply_static_redirect(idx, region, log);
                            if self.is_healthy(idx) {
                                return self.route_to(session, region, log).await;
                            } else {
                                log.push(&format!("{} unhealthy; ", region.name));
                                return self.do_failover(session, log, Some(idx)).await;
                            }
                        }
                        None => {
                            // Not a valid region name → fall through to next rule
                            log.push(&format!("direct unknown '{}', fallthrough; ", name));
                            continue;
                        }
                    }
                }

                RouteRule::RouteByKeyConfMatch { match_map } => {
                    let extracted = self.extract_first(session, &self.config.key_get).await;
                    let (value, src) = match extracted {
                        Some(v) => v,
                        None => continue, // key absent → try next rule (no log, saves bytes)
                    };
                    match match_map.get(&value) {
                        Some(region_name) => {
                            match self.find_region(region_name) {
                                Some((idx, region)) => {
                                    // Log source and region (not value — may be sensitive)
                                    log.push(&format!(
                                        "match={}→{}; ",
                                        src.as_log_str(),
                                        region.name
                                    ));
                                    let (idx, region) = self.apply_static_redirect(idx, region, log);
                                    if self.is_healthy(idx) {
                                        return self.route_to(session, region, log).await;
                                    } else {
                                        log.push(&format!("{} unhealthy; ", region.name));
                                        return self.do_failover(session, log, Some(idx)).await;
                                    }
                                }
                                None => {
                                    // Region name in matchMap is not defined → fall through
                                    log.push(&format!(
                                        "match={}→{} unknown; ",
                                        src.as_log_str(),
                                        region_name
                                    ));
                                    continue;
                                }
                            }
                        }
                        None => {
                            // Value not in matchMap → try next rule
                            log.push(&format!("match={} no conf; ", src.as_log_str()));
                            continue;
                        }
                    }
                }

                RouteRule::RouteByHashRange => {
                    // RouteByHashRange is terminal: always stops rule evaluation
                    match self.exec_hash_range(session, log).await {
                        Some((idx, region)) => {
                            let (idx, region) = self.apply_static_redirect(idx, region, log);
                            if self.is_healthy(idx) {
                                return self.route_to(session, region, log).await;
                            } else {
                                log.push(&format!("{} unhealthy; ", region.name));
                                return self.do_failover(session, log, Some(idx)).await;
                            }
                        }
                        None => {
                            // Passthrough — terminal, no further rules
                            return PluginRunningResult::GoodNext;
                        }
                    }
                }
            }
        }

        PluginRunningResult::GoodNext
    }
}

// ── Probe loop ────────────────────────────────────────────────────────────────

/// Background task: periodically DNS-resolves each region's backend_endpoint.
/// Updates the health table: 1.0 if ≥1 IP resolved, 0.0 on empty/error.
///
/// DNS reflects K8s Endpoints/EndpointSlice state via CoreDNS:
/// when pods become NotReady they are removed from EPS, CoreDNS stops
/// returning their IPs, and the next probe detects this.
async fn probe_loop(config: Arc<RegionRouteConfig>, health: Arc<RegionHealthTable>) {
    let interval = Duration::from_millis(
        config
            .auto_failover
            .as_ref()
            .map(|f| f.probe_interval_ms)
            .unwrap_or(5000),
    );

    loop {
        tokio::time::sleep(interval).await;

        for (i, region) in config.regions.iter().enumerate() {
            let ratio = probe_region(region).await;
            health.store(i, ratio);
            tracing::debug!(
                plugin = "RegionRoute",
                region = %region.name,
                endpoint = %region.backend_endpoint,
                healthy = ratio,
                "region health probe"
            );
        }
    }
}

/// Probe one region by DNS-resolving its backend_endpoint.
/// Returns 1.0 if at least one IP is returned, 0.0 otherwise.
async fn probe_region(region: &RegionDef) -> f32 {
    let (host, port) = region.parse_endpoint();
    match tokio::net::lookup_host((host, port)).await {
        Ok(mut addrs) => {
            if addrs.next().is_some() { 1.0 } else { 0.0 }
        }
        Err(_) => 0.0,
    }
}
