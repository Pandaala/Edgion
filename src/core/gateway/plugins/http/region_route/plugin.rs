//! RegionRoute plugin implementation.
//!
//! See [`RegionRouteConfig`] for the full routing-priority model and field semantics.
//! When a key is configured but absent from the request, the plugin passes through
//! without modifying the upstream (fail-open, not fail-close).

use std::cmp::Ordering;
use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::common::KeyGet;
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{OnAllDown, RegionDef, RegionRouteConfig};
use crate::types::ExternalJumpPreset;

// ── Hash function ─────────────────────────────────────────────────────────────

/// Compute SHA-256 of `key`, take first 8 bytes as big-endian u64, mod 1001 → [0, 1000].
///
/// SHA-256 is used because it is already a project dependency (sha2 crate).
/// The algorithm is deterministic and has excellent distribution.
///
/// Pre-computed values (via region_key_hash passthrough) bypass this entirely.
fn compute_region_hash(key: &str) -> u16 {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    let h = u64::from_be_bytes(digest[..8].try_into().expect("sha256 >= 8 bytes"));
    (h % 1001) as u16
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

pub struct RegionRoute {
    name: String,
    config: Arc<RegionRouteConfig>,
    health: Arc<RegionHealthTable>,
    /// Cached at creation to avoid Arc → Option traversal on every hot-path call.
    healthy_threshold: f32,
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

    // ── Hash resolution ───────────────────────────────────────────────────────

    /// Resolve the 0-1000 hash value for this request.
    ///
    /// Returns `None` when the plugin should be a no-op:
    /// `region_key` is empty, or no key was found in any configured source.
    async fn resolve_hash_value(&self, session: &dyn PluginSession, log: &mut PluginLog) -> Option<u16> {
        // Priority 1: pre-computed passthrough
        if let Some(ref pt) = self.config.region_key_hash {
            if let Some((raw, src)) = self.extract_first(session, &pt.key_from).await {
                if let Ok(v) = raw.parse::<u16>() {
                    if v <= 1000 {
                        log.push(&format!("hash_pass={} from:{}; ", v, src.as_log_str()));
                        return Some(v);
                    }
                }
            }
        }

        // Priority 2: compute from region_key
        if self.config.region_key.is_empty() {
            log.push("no region_key, pass; ");
            return None;
        }

        match self.extract_first(session, &self.config.region_key).await {
            Some((key, src)) => {
                let v = compute_region_hash(&key);
                // Log source only, not value — key may contain sensitive identifiers.
                log.push(&format!("key={} hash={}; ", src.as_log_str(), v));
                Some(v)
            }
            None => {
                log.push("region_key missing, pass; ");
                None
            }
        }
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

    /// Set `ExternalJumpPreset` so Pingora DNS-resolves the backend_endpoint host to an IP,
    /// while keeping the original request's SNI and Host header unchanged.
    async fn route_to(&self, session: &mut dyn PluginSession, region: &RegionDef, log: &mut PluginLog) -> PluginRunningResult {
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

        log.push(&format!("→{} {}:{} sni={}; ", region.name, host, port, sni));

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

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if self.config.get_validation_error().is_some() {
            log.push("config invalid, pass; ");
            return PluginRunningResult::GoodNext;
        }

        // Step 1: direct_region (optional, highest priority)
        if let Some(ref dr) = self.config.direct_region {
            if let Some((name, src)) = self.extract_first(session, &dr.key_from).await {
                match self.find_region(&name) {
                    Some((idx, region)) if self.is_healthy(idx) => {
                        log.push(&format!("direct={}(from:{}); ", name, src.as_log_str()));
                        return self.route_to(session, region, log).await;
                    }
                    Some((idx, _)) => {
                        log.push(&format!("direct={}(from:{}) unhealthy; ", name, src.as_log_str()));
                        return self.do_failover(session, log, Some(idx)).await;
                    }
                    None => {
                        // Not a valid region name → fall through to hash routing
                        log.push(&format!("direct unknown '{}', fallthrough; ", name));
                    }
                }
            }
        }

        // Step 2: resolve hash value (passthrough or compute)
        let hash_val = match self.resolve_hash_value(session, log).await {
            Some(v) => v,
            None => return PluginRunningResult::GoodNext,
        };

        // Step 3: lookup region → route or failover
        match self.lookup_region(hash_val) {
            Some((idx, region)) if self.is_healthy(idx) => self.route_to(session, region, log).await,
            Some((idx, region)) => {
                log.push(&format!("hash={}→{} unhealthy; ", hash_val, region.name));
                self.do_failover(session, log, Some(idx)).await
            }
            None => {
                log.push(&format!("hash={} no range; ", hash_val));
                PluginRunningResult::GoodNext
            }
        }
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
