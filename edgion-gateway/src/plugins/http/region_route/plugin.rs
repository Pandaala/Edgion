//! RegionRoute HTTP plugin — base + overlay region routing.
//!
//! # Base + overlay model (S3)
//! `RegionRouteConfig` (in EdgionPlugins) holds ALL routing logic plus the default region table.
//! At request time, if `override_ref` is set, the plugin looks up a `RegionRouteOverride`
//! EdgionConfigData from the global `EdgionConfigDataStore` (ArcSwap, lock-free) and applies a typed
//! overlay on top of the base `regions` via `apply_override` (D6 type guard + whole-replace).
//!
//! # Fail-to-base semantics
//! - **`overlay_missing`**: key not found in the EdgionConfigData store → base regions used.
//! - **`overlay_type_mismatch`**: entry found but wrong `ConfigEntry` variant → base regions used.
//!
//! Both cases are recorded via `record_region_route_overlay_event`. No data-plane tracing
//! (warn/error) is used — all signals go through Prometheus counters.
//!
//! # Request-time resolution
//! `compute_effective_regions` is called per request: one ArcSwap load + one hashmap lookup.
//! The plugin struct itself caches only the base config (Arc) and the optional overlay store key.

use std::sync::Arc;

use async_trait::async_trait;

use crate::observe::metrics::record_region_route_overlay_event;
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::{ExternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_config_data::{
    apply_override, merge_region_route_override, validate_regions, ConfigEntry, OverlaySignal,
};
use edgion_resources::resources::edgion_plugins::{HashAlgorithm, RegionDef, RegionRouteConfig, RouteRule};

// ── Hash helpers ──────────────────────────────────────────────────────────────

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

// ── Plugin struct ─────────────────────────────────────────────────────────────

/// HTTP plugin: routes requests to a region based on ordered route rules.
///
/// Holds a clone of the base `RegionRouteConfig` from EdgionPlugins, plus an optional
/// EdgionConfigData store key ("namespace/name") for a `RegionRouteOverride` overlay.
pub struct RegionRoute {
    name: String,
    /// Full base config: all routing logic + default region table.
    /// `Arc` avoids a clone per request; the config is read-only after creation.
    base: Arc<RegionRouteConfig>,
    /// Optional EdgionConfigData store key ("namespace/name") for a `RegionRouteOverride`.
    /// `None` means no overlay is configured; base regions are used directly.
    override_key: Option<String>,
}

impl RegionRoute {
    /// Create the plugin from a full `RegionRouteConfig` (from EdgionPlugins).
    ///
    /// `ep_namespace` is the EdgionPlugins namespace, used as the default namespace
    /// for `override_ref` entries that do not specify their own namespace.
    pub fn create(ep_config: &RegionRouteConfig, ep_namespace: &str) -> Box<dyn RequestFilter> {
        let override_key = ep_config
            .override_ref
            .as_ref()
            .filter(|r| r.ref_denied.is_none())
            .map(|r| {
                let ns = r.namespace.as_deref().unwrap_or(ep_namespace);
                format!("{}/{}", ns, r.name)
            });

        Box::new(Self {
            name: "RegionRoute".to_string(),
            base: Arc::new(ep_config.clone()),
            override_key,
        })
    }

    /// Resolve the effective region table for this request (per-request, lock-free).
    ///
    /// Control flow:
    /// 1. **No overlay configured** (`override_key` is `None`) → return base regions directly.
    /// 2. **Envelope kill-switch** (`spec.enable == false`) → emit `overlay_disabled`, return base.
    /// 3. **Type guard (D6)** → `apply_override` rejects wrong `ConfigEntry` variants.
    /// 4. **`apply_override`** merges overlay onto base (whole-replace if `regions` is `Some`).
    /// 5. **Post-merge validation** (H2) → `validate_regions` on the merged result; on failure
    ///    emit `overlay_invalid` and return base.
    ///
    /// All non-`Applied` paths emit a Prometheus counter; no `tracing::warn!/error!`.
    fn compute_effective_regions(&self, log: &mut PluginLog) -> Vec<RegionDef> {
        let Some(ref key) = self.override_key else {
            // No overlay configured — base regions used as-is.
            return self.base.regions.clone();
        };

        let store = get_global_edgion_config_data_store();
        let raw_ed = store.get_raw_by_key(key);

        // H1 — Envelope kill-switch: spec.enable=false means the operator has disabled
        // this EdgionConfigData entirely. Treat as no overlay: use base, emit overlay_disabled.
        if raw_ed.as_ref().map(|ed| !ed.spec.enable).unwrap_or(false) {
            log.push_fmt("overlay_disabled (key={}), using base", la![sensitive(key)]);
            record_region_route_overlay_event("overlay_disabled");
            return self.base.regions.clone();
        }

        let overlay_entry = raw_ed.as_deref().map(|ed| &ed.spec.data);

        let (effective, signal) = apply_override(
            self.base.regions.clone(),
            overlay_entry,
            // D6 type guard: only accept `RegionRouteOverride` variant.
            |entry: &ConfigEntry| match entry {
                ConfigEntry::RegionRouteOverride(o) => Some(o),
                _ => None,
            },
            // Merge: whole-replace `regions` if Some; keep base otherwise.
            merge_region_route_override,
        );

        match signal {
            OverlaySignal::Absent => {
                log.push_fmt("overlay_missing (key={}), using base", la![sensitive(key)]);
                record_region_route_overlay_event("overlay_missing");
            }
            OverlaySignal::TypeMismatch => {
                log.push_fmt("overlay_type_mismatch (key={}), using base", la![sensitive(key)]);
                record_region_route_overlay_event("overlay_type_mismatch");
            }
            OverlaySignal::Applied => {
                // H2 — Validate the merged effective region table. Guards against malformed
                // overlays that slipped past controller validation (race / fed_sync /
                // disabled controller validation).
                if validate_regions(&effective).is_err() {
                    log.push_fmt("overlay_invalid (key={}), using base", la![sensitive(key)]);
                    record_region_route_overlay_event("overlay_invalid");
                    return self.base.regions.clone();
                }
            }
        }

        effective
    }

    // ── Key extraction ────────────────────────────────────────────────────────

    /// Extract the first non-empty value from the given key sources.
    async fn extract_first<'s>(
        &self,
        session: &mut dyn PluginSession,
        sources: &'s [edgion_resources::common::KeyGet],
    ) -> Option<(String, &'s edgion_resources::common::KeyGet)> {
        for source in sources {
            if let Some(v) = session.key_get(source).await {
                if !v.is_empty() {
                    return Some((v, source));
                }
            }
        }
        None
    }

    // ── Region lookup helpers ─────────────────────────────────────────────────

    fn lookup_by_hash(regions: &[RegionDef], hash_val: u16) -> Option<(usize, &RegionDef)> {
        regions
            .iter()
            .enumerate()
            .find(|(_, r)| hash_val >= r.hash_range[0] && hash_val <= r.hash_range[1])
    }

    fn lookup_by_name<'r>(regions: &'r [RegionDef], name: &str) -> Option<(usize, &'r RegionDef)> {
        regions.iter().enumerate().find(|(_, r)| r.name == name)
    }

    // ── Failover redirect ─────────────────────────────────────────────────────

    /// Resolve the `failoverTo` redirect for a region, returning the final `(idx, region)`.
    ///
    /// Only one level of redirect is followed. A self-referencing or dangling `failoverTo`
    /// is treated as an error and the original region is returned unchanged.
    fn apply_failover_redirect<'r>(
        regions: &'r [RegionDef],
        idx: usize,
        region: &'r RegionDef,
        log: &mut PluginLog,
    ) -> (usize, &'r RegionDef) {
        let Some(ref target_name) = region.failover_to else {
            return (idx, region);
        };

        if target_name == &region.name {
            log.push_err("failover_self_ref");
            return (idx, region);
        }

        match Self::lookup_by_name(regions, target_name) {
            Some((target_idx, target_region)) => {
                log.push_fmt(
                    "failover {}→{}",
                    la![sensitive(&region.name), sensitive(&target_region.name)],
                );
                (target_idx, target_region)
            }
            None => {
                log.push_err("failover_target_not_found");
                (idx, region)
            }
        }
    }

    // ── Upstream routing ──────────────────────────────────────────────────────

    /// Route the request to the given region by setting an external upstream jump.
    ///
    /// If this gateway's own region (`my_region`) matches the target, passes through
    /// without redirecting (local handling).
    async fn route_to(
        &self,
        session: &mut dyn PluginSession,
        region: &RegionDef,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Pass through when the target region is this gateway's own region.
        if !self.base.my_region.is_empty() && self.base.my_region == region.name {
            log.push("local_region, pass");
            return PluginRunningResult::GoodNext;
        }

        let (host, port) = region.parse_endpoint();

        // SNI: strip port from the incoming Host header (IPv6-bracket-safe).
        let raw_host = session.header_value("Host").unwrap_or_default();
        let sni = crate::util::net::host_without_port(&raw_host).to_string();

        log.push_fmt(
            "→{} {}:{} sni={}",
            la![
                sensitive(&region.name),
                sensitive(host),
                sensitive(&port.to_string()),
                sensitive(&sni)
            ],
        );

        match session.set_upstream_jump(UpstreamJumpTarget::External(ExternalJumpPreset {
            domain: host.to_string(),
            port,
            use_tls: region.tls,
            sni,
            // Cross-region hop: the peer gateway handles its own path normalization.
            preserve_raw_path: true,
        })) {
            SetJumpOutcome::Set => {}
            SetJumpOutcome::Rejected => {
                log.push_err("jump_rejected");
            }
        }

        PluginRunningResult::GoodNext
    }

    // ── RouteByHashRange ──────────────────────────────────────────────────────

    /// Execute the `RouteByHashRange` rule: try pre-computed hash passthrough first,
    /// then compute via `hash_calc`. Returns `Some((idx, region))` on a match.
    ///
    /// This rule is terminal (as documented in `RouteRule::RouteByHashRange`): if neither
    /// the passthrough nor the computation yields a region, the caller must return
    /// `GoodNext` without evaluating further rules.
    async fn exec_hash_range<'r>(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        regions: &'r [RegionDef],
    ) -> Option<(usize, &'r RegionDef)> {
        let modulo = self.base.hash_calc.as_ref().map(|c| c.modulo).unwrap_or(1000);

        // Fast path: try pre-computed hash from hash_key_get.
        if let Some(ref hkg) = self.base.hash_key_get {
            if let Some((raw, src)) = self.extract_first(session, hkg).await {
                if let Ok(v) = raw.parse::<u16>() {
                    if v < modulo {
                        log.push_fmt(
                            "hash_pass={} src={}",
                            la![sensitive(&v.to_string()), sensitive(&src.as_log_str())],
                        );
                        return Self::lookup_by_hash(regions, v);
                    }
                }
            }
        }

        // Compute hash via hash_calc.
        let calc = match self.base.hash_calc.as_ref() {
            Some(c) => c,
            None => {
                log.push("no_hash_calc, pass");
                return None;
            }
        };

        if self.base.key_get.is_empty() {
            log.push("no_key_get, pass");
            return None;
        }

        let (key, src) = match self.extract_first(session, &self.base.key_get).await {
            Some(v) => v,
            None => {
                log.push("key_missing, pass");
                return None;
            }
        };

        let hash_val = compute_hash(&key, &calc.algorithm, calc.modulo);

        log.push_fmt(
            "hash key={} src={} val={}",
            la![
                sensitive(&key),
                sensitive(&src.as_log_str()),
                sensitive(&hash_val.to_string())
            ],
        );

        // Write back computed hash if configured.
        if !calc.write_back.is_empty() {
            let mut wb_targets = String::new();
            for wb in &calc.write_back {
                let _ = session.key_set(wb, Some(hash_val.to_string()));
                if !wb_targets.is_empty() {
                    wb_targets.push(',');
                }
                wb_targets.push_str(&wb.as_log_str());
            }
            log.push_fmt("hash_wb→{}", la![sensitive(&wb_targets)]);
        }

        let result = Self::lookup_by_hash(regions, hash_val);
        if result.is_none() {
            log.push_fmt("hash_val={} no_range, pass", la![sensitive(&hash_val.to_string())]);
        }
        result
    }
}

// ── RequestFilter impl ────────────────────────────────────────────────────────

#[async_trait]
impl RequestFilter for RegionRoute {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Short-circuit: nothing configured at all.
        if self.base.route_rules.is_empty() && self.base.regions.is_empty() {
            log.push("no_region_route_config, pass");
            return PluginRunningResult::GoodNext;
        }

        // Resolve the effective region table: base + optional overlay (fail-to-base on error).
        let effective_regions = self.compute_effective_regions(log);

        if effective_regions.is_empty() {
            log.push("no_regions, pass");
            return PluginRunningResult::GoodNext;
        }

        // Evaluate route rules in order — first match wins.
        for rule in &self.base.route_rules {
            match rule {
                // ── RouteByDirectRegion ────────────────────────────────────────
                RouteRule::RouteByDirectRegion { key_from } => {
                    let (name, src) = match self.extract_first(session, key_from).await {
                        Some(v) => v,
                        None => continue,
                    };
                    match Self::lookup_by_name(&effective_regions, &name) {
                        Some((idx, region)) => {
                            log.push_fmt("direct={} src={}", la![sensitive(&name), sensitive(&src.as_log_str())]);
                            let (_, region) = Self::apply_failover_redirect(&effective_regions, idx, region, log);
                            return self.route_to(session, region, log).await;
                        }
                        None => {
                            log.push_fmt("direct={} unknown, fallthrough", la![sensitive(&name)]);
                            continue;
                        }
                    }
                }

                // ── RouteByKeyConfMatch ────────────────────────────────────────
                RouteRule::RouteByKeyConfMatch { match_map: rule_map } => {
                    if self.base.key_get.is_empty() {
                        log.push("conf_match: no key_get, skip");
                        continue;
                    }
                    let (value, src) = match self.extract_first(session, &self.base.key_get).await {
                        Some(v) => v,
                        None => continue,
                    };

                    // Use cluster override map if set, non-empty, and not bypassed.
                    let effective_map = {
                        let bypassed = self
                            .base
                            .route_by_key_conf_match
                            .as_ref()
                            .map(|kmc| kmc.bypass_cluster_keys.contains(&value))
                            .unwrap_or(false);

                        if !bypassed {
                            if let Some(kmc) = &self.base.route_by_key_conf_match {
                                if !kmc.match_map.is_empty() {
                                    &kmc.match_map
                                } else {
                                    rule_map
                                }
                            } else {
                                rule_map
                            }
                        } else {
                            rule_map
                        }
                    };

                    match effective_map.get(&value) {
                        Some(region_name) => match Self::lookup_by_name(&effective_regions, region_name) {
                            Some((idx, region)) => {
                                log.push_fmt(
                                    "conf_match key={} src={} →{}",
                                    la![sensitive(&value), sensitive(&src.as_log_str()), sensitive(&region.name)],
                                );
                                let (_, region) = Self::apply_failover_redirect(&effective_regions, idx, region, log);
                                return self.route_to(session, region, log).await;
                            }
                            None => {
                                log.push_fmt(
                                    "conf_match key={} →{} unknown, fallthrough",
                                    la![sensitive(&value), sensitive(region_name)],
                                );
                                continue;
                            }
                        },
                        None => {
                            log.push_fmt(
                                "conf_match key={} src={} no_entry, fallthrough",
                                la![sensitive(&value), sensitive(&src.as_log_str())],
                            );
                            continue;
                        }
                    }
                }

                // ── RouteByHashRange ──────────────────────────────────────────
                RouteRule::RouteByHashRange => {
                    // RouteByHashRange is terminal: None (no range match) → pass through without
                    // evaluating further rules.
                    match self.exec_hash_range(session, log, &effective_regions).await {
                        Some((idx, region)) => {
                            let (_, region) = Self::apply_failover_redirect(&effective_regions, idx, region, log);
                            return self.route_to(session, region, log).await;
                        }
                        None => {
                            return PluginRunningResult::GoodNext;
                        }
                    }
                }
            }
        }

        PluginRunningResult::GoodNext
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::log::PluginLog;
    use edgion_resources::resources::edgion_plugins::{RegionDef, RegionRouteConfig};

    fn make_region(name: &str, low: u16, high: u16, endpoint: &str) -> RegionDef {
        RegionDef {
            name: name.to_string(),
            hash_range: [low, high],
            backend_endpoint: endpoint.to_string(),
            tls: false,
            failover_to: None,
        }
    }

    /// A default (empty) config must not panic and must yield no routing logic.
    #[test]
    fn default_config_has_no_routing() {
        let config = RegionRouteConfig::default();
        assert!(config.route_rules.is_empty());
        assert!(config.regions.is_empty());
        assert!(config.override_ref.is_none());
    }

    /// When no override_key is set, `compute_effective_regions` returns the base regions.
    #[test]
    fn base_regions_used_when_no_override_ref() {
        let config = RegionRouteConfig {
            regions: vec![
                make_region("us-east", 0, 499, "10.0.0.1:8080"),
                make_region("us-west", 500, 999, "10.0.0.2:8080"),
            ],
            override_ref: None,
            ..Default::default()
        };
        let plugin = RegionRoute {
            name: "RegionRoute".to_string(),
            base: Arc::new(config),
            override_key: None,
        };

        let mut log = PluginLog::new("RegionRoute");
        let regions = plugin.compute_effective_regions(&mut log);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].name, "us-east");
        assert_eq!(regions[1].name, "us-west");
    }

    /// When override_key is set but not found in the global store, the plugin must
    /// fall back to the base regions (fail-to-base, not fail-closed).
    #[test]
    fn overlay_missing_falls_back_to_base() {
        let config = RegionRouteConfig {
            regions: vec![make_region("eu-west", 0, 999, "10.0.1.1:8080")],
            ..Default::default()
        };
        let plugin = RegionRoute {
            name: "RegionRoute".to_string(),
            base: Arc::new(config),
            // Key that is not present in the global store in this unit test.
            override_key: Some("unit-test-ns/rr-override-absent-key".to_string()),
        };

        let mut log = PluginLog::new("RegionRoute");
        let regions = plugin.compute_effective_regions(&mut log);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].name, "eu-west");
    }

    /// `lookup_by_hash` returns the first region whose hash_range covers the given value.
    #[test]
    fn lookup_by_hash_returns_correct_region() {
        let regions = [make_region("r1", 0, 499, "h1:80"), make_region("r2", 500, 999, "h2:80")];
        assert_eq!(
            RegionRoute::lookup_by_hash(&regions, 0).map(|(_, r)| r.name.as_str()),
            Some("r1")
        );
        assert_eq!(
            RegionRoute::lookup_by_hash(&regions, 499).map(|(_, r)| r.name.as_str()),
            Some("r1")
        );
        assert_eq!(
            RegionRoute::lookup_by_hash(&regions, 500).map(|(_, r)| r.name.as_str()),
            Some("r2")
        );
        assert_eq!(
            RegionRoute::lookup_by_hash(&regions, 999).map(|(_, r)| r.name.as_str()),
            Some("r2")
        );
    }

    /// `apply_failover_redirect` with a valid target returns the target region.
    #[test]
    fn apply_failover_redirect_follows_one_level() {
        let regions = [
            RegionDef {
                name: "primary".to_string(),
                hash_range: [0, 499],
                backend_endpoint: "h1:80".to_string(),
                tls: false,
                failover_to: Some("backup".to_string()),
            },
            RegionDef {
                name: "backup".to_string(),
                hash_range: [500, 999],
                backend_endpoint: "h2:80".to_string(),
                tls: false,
                failover_to: None,
            },
        ];
        let mut log = PluginLog::new("RegionRoute");
        let (_, r) = RegionRoute::apply_failover_redirect(&regions, 0, &regions[0], &mut log);
        assert_eq!(r.name, "backup");
    }

    /// `apply_failover_redirect` with a self-referencing `failoverTo` returns the original.
    #[test]
    fn apply_failover_redirect_self_ref_returns_original() {
        let regions = [RegionDef {
            name: "only".to_string(),
            hash_range: [0, 999],
            backend_endpoint: "h1:80".to_string(),
            tls: false,
            failover_to: Some("only".to_string()),
        }];
        let mut log = PluginLog::new("RegionRoute");
        let (_, r) = RegionRoute::apply_failover_redirect(&regions, 0, &regions[0], &mut log);
        assert_eq!(r.name, "only");
    }

    /// `apply_failover_redirect` with an unknown target returns the original.
    #[test]
    fn apply_failover_redirect_unknown_target_returns_original() {
        let regions = [RegionDef {
            name: "a".to_string(),
            hash_range: [0, 999],
            backend_endpoint: "h1:80".to_string(),
            tls: false,
            failover_to: Some("nonexistent".to_string()),
        }];
        let mut log = PluginLog::new("RegionRoute");
        let (_, r) = RegionRoute::apply_failover_redirect(&regions, 0, &regions[0], &mut log);
        assert_eq!(r.name, "a");
    }

    /// H1: When the referenced EdgionConfigData has `spec.enable=false` (envelope kill-switch),
    /// the overlay must NOT be applied — base regions must be returned.
    ///
    /// The overlay contains regions that differ from base; if the kill-switch were ignored,
    /// the base regions would be replaced. Asserting base regions verifies H1.
    #[test]
    fn overlay_envelope_disabled_falls_back_to_base() {
        use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
        use edgion_resources::resources::edgion_config_data::{
            ConfigEntry, EdgionConfigData, EdgionConfigDataSpec, RegionRouteOverride,
        };
        use std::sync::Arc;

        // Unique key — no other test inserts or depends on this key.
        let test_key = "unit-test-ns/rr-overlay-envelope-disabled-h1-test";

        // Build an EdgionConfigData with spec.enable=false and regions that DIFFER from base.
        // If the kill-switch is honoured, these regions must NOT replace base.
        let override_regions = vec![make_region("override-only", 0, 999, "override-ep:9090")];
        let mut ed = EdgionConfigData::new(
            "rr-overlay-envelope-disabled-h1-test",
            EdgionConfigDataSpec {
                enable: false, // envelope kill-switch
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                    enable: None,
                    active: None,
                    regions: Some(override_regions),
                }),
            },
        );
        ed.metadata.namespace = Some("unit-test-ns".to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

        // Base: single region distinct from the override.
        let config = RegionRouteConfig {
            regions: vec![make_region("base-region", 0, 999, "base-ep:8080")],
            ..Default::default()
        };
        let plugin = RegionRoute {
            name: "RegionRoute".to_string(),
            base: Arc::new(config),
            override_key: Some(test_key.to_string()),
        };

        let mut log = PluginLog::new("RegionRoute");
        let regions = plugin.compute_effective_regions(&mut log);

        assert_eq!(regions.len(), 1, "base has exactly 1 region");
        assert_eq!(
            regions[0].name, "base-region",
            "envelope disabled: must return base regions, not override"
        );
    }

    /// H2: When the overlay is applied but the merged effective regions are invalid
    /// (duplicate region names), the plugin must fall back to base regions.
    #[test]
    fn overlay_invalid_regions_falls_back_to_base() {
        use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
        use edgion_resources::resources::edgion_config_data::{
            ConfigEntry, EdgionConfigData, EdgionConfigDataSpec, RegionRouteOverride,
        };
        use std::sync::Arc;

        let test_key = "unit-test-ns/rr-overlay-invalid-dup-names-h2-test";

        // Build an EdgionConfigData whose overlay has duplicate region names (invalid).
        let dup_regions = vec![
            make_region("dup-name", 0, 499, "ep-a:8080"),
            make_region("dup-name", 500, 999, "ep-b:8080"), // duplicate — must trigger overlay_invalid
        ];
        let mut ed = EdgionConfigData::new(
            "rr-overlay-invalid-dup-names-h2-test",
            EdgionConfigDataSpec {
                enable: true, // envelope enabled — the validation must catch the bad regions
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                    enable: None,
                    active: None,
                    regions: Some(dup_regions),
                }),
            },
        );
        ed.metadata.namespace = Some("unit-test-ns".to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(test_key, Arc::new(ed));

        let config = RegionRouteConfig {
            regions: vec![make_region("valid-base", 0, 999, "base:8080")],
            ..Default::default()
        };
        let plugin = RegionRoute {
            name: "RegionRoute".to_string(),
            base: Arc::new(config),
            override_key: Some(test_key.to_string()),
        };

        let mut log = PluginLog::new("RegionRoute");
        let regions = plugin.compute_effective_regions(&mut log);

        assert_eq!(regions.len(), 1, "base has exactly 1 region");
        assert_eq!(
            regions[0].name, "valid-base",
            "invalid overlay: must fall back to base regions"
        );
    }
}
