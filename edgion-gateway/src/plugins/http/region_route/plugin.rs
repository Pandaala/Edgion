//! RegionRoute HTTP plugin — base + overlay region routing.
//!
//! # Base + overlay model (S3)
//! `RegionRouteConfig` (in EdgionPlugins) holds ALL routing logic plus the default region table.
//! At request time, if `override_ref` is set, the plugin looks up a `RegionRouteOverride`
//! EdgionConfigData and its validation proof from one global store snapshot (ArcSwap, lock-free).
//! `EffectiveRegions` holds the accepted generation and borrows its whole replacement table.
//!
//! # Fail-to-base semantics
//! - **`overlay_missing`**: key not found in the EdgionConfigData store → base regions used.
//! - **`overlay_type_mismatch`**: entry found but wrong `ConfigEntry` variant → base regions used.
//!
//! Both cases are recorded via `record_region_route_overlay_event`. No data-plane tracing
//! (warn/error) is used; PluginLog and DyeOutcome carry request-level fallback evidence.
//!
//! # Request-time resolution
//! `compute_effective_regions` is called per request: one ArcSwap load with paired raw/proof lookups.
//! The plugin struct itself caches only the base config (Arc) and the optional overlay store key.

use edgion_resources::refs::config_data_fields::HttpConfigDataEntry;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use std::{num::NonZeroU16, sync::Arc};

use async_trait::async_trait;

use crate::observe::metrics::record_region_route_overlay_event;
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use crate::{ExternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigData};
use edgion_resources::resources::edgion_plugins::{
    HashAlgorithm, PluginValidation, RegionDef, RegionRouteConfig, RouteRule,
};

/// Own the overlay generation while borrowing its table; never clone a table
/// just to inspect it on the request path.
struct EffectiveRegions<'a> {
    base: &'a [RegionDef],
    overlay: Option<Arc<EdgionConfigData>>,
}

impl std::ops::Deref for EffectiveRegions<'_> {
    type Target = [RegionDef];

    fn deref(&self) -> &Self::Target {
        if let Some(resource) = &self.overlay {
            if let ConfigEntry::RegionRouteOverride(overlay) = &resource.spec.data {
                if let Some(regions) = &overlay.regions {
                    return regions;
                }
            }
        }
        self.base
    }
}

// ── Hash helpers ──────────────────────────────────────────────────────────────

fn compute_hash(key: &str, algorithm: &HashAlgorithm, modulo: NonZeroU16) -> u16 {
    let modulo = u64::from(modulo.get());
    match algorithm {
        HashAlgorithm::Crc32 => {
            let n = crc32fast::hash(key.as_bytes()) as u64;
            (n % modulo) as u16
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
            (n % modulo) as u16
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
    init_error: Option<&'static str>,
}

impl RegionRoute {
    pub(crate) fn create_bound(entry: HttpConfigDataEntry<'_>) -> Option<Box<dyn RequestFilter>> {
        let EdgionPlugin::RegionRoute(config) = entry.plugin() else {
            return None;
        };
        if entry.has_references() {
            entry.validate_owner().ok()?;
        }
        let key = entry.field().and_then(|field| field.runtime_key().ok().flatten());
        Some(Self::build(config, key))
    }

    pub(crate) fn create_inline(config: &RegionRouteConfig) -> Option<Box<dyn RequestFilter>> {
        if config.override_ref.is_some() {
            return None;
        }
        Some(Self::build(config, None))
    }

    fn build(ep_config: &RegionRouteConfig, override_key: Option<String>) -> Box<dyn RequestFilter> {
        Box::new(Self {
            name: "RegionRoute".to_string(),
            init_error: ep_config.validation_error().map(|_| {
                if ep_config.effective_modulo() < 2 {
                    "hash_modulo_invalid"
                } else {
                    "region_table_invalid"
                }
            }),
            base: Arc::new(ep_config.clone()),
            override_key,
        })
    }

    /// Resolve the effective region table for this request (per-request, lock-free).
    ///
    /// Control flow:
    /// 1. **No overlay configured** (`override_key` is `None`) → return base regions directly.
    /// 2. **Envelope kill-switch** (`spec.enable == false`) → emit `overlay_disabled`, return base.
    /// 3. Type-guard the raw entry and read its config-time validation proof from
    ///    the same immutable snapshot.
    /// 4. Check only the consumer's modulo; invalid overlays return base.
    /// 5. Hold the valid overlay generation for whole-table replacement.
    ///
    /// All non-`Applied` paths emit a Prometheus counter; no `tracing::warn!/error!`.
    fn compute_effective_regions(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> EffectiveRegions<'_> {
        let base = EffectiveRegions {
            base: &self.base.regions,
            overlay: None,
        };
        let Some(ref key) = self.override_key else {
            return base;
        };
        let store = get_global_edgion_config_data_store();
        let signal = match store.get_region_route_by_key(key) {
            None => {
                log.push_fmt("overlay_missing (key={}), using base", la![sensitive(key)]);
                "overlay_missing"
            }
            Some((resource, _)) if !resource.spec.enable => {
                log.push_fmt("overlay_disabled (key={}), using base", la![sensitive(key)]);
                "overlay_disabled"
            }
            Some((resource, proof)) => {
                if let ConfigEntry::RegionRouteOverride(overlay) = &resource.spec.data {
                    if overlay.regions.is_none() {
                        return base;
                    }
                    if proof.is_some_and(|proof| proof.accepts_modulo(self.base.effective_modulo())) {
                        return EffectiveRegions {
                            base: &self.base.regions,
                            overlay: Some(resource),
                        };
                    }
                    log.push_fmt("overlay_invalid (key={}), using base", la![sensitive(key)]);
                    "overlay_invalid"
                } else {
                    log.push_fmt("overlay_type_mismatch (key={}), using base", la![sensitive(key)]);
                    "overlay_type_mismatch"
                }
            }
        };
        record_region_route_overlay_event(signal);
        session.record_outcome(DyeOutcome::Fallback, "overlay-fail-to-base");
        base
    }

    // ── Key extraction ────────────────────────────────────────────────────────

    /// Extract the first non-empty value from the given key sources.
    fn extract_first<'s>(
        &self,
        session: &mut dyn PluginSession,
        sources: &'s [edgion_resources::common::KeyGet],
    ) -> Option<(String, &'s edgion_resources::common::KeyGet)> {
        for source in sources {
            if let Some(v) = session.key_get(source) {
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
            session.record_outcome(DyeOutcome::Success, &region.name);
            return PluginRunningResult::GoodNext;
        }

        let Ok((host, port)) = region.parse_endpoint() else {
            log.push_err("region_endpoint_invalid");
            return PluginRunningResult::GoodNext;
        };

        // SNI: strip port from the incoming Host header (IPv6-bracket-safe).
        let raw_host = session.header_str("Host").unwrap_or_default();
        let sni = crate::util::net::host_without_port(raw_host).to_string();

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
            upstream_read_timeout_override: None,
        })) {
            SetJumpOutcome::Set => {
                session.record_outcome(DyeOutcome::Success, &region.name);
            }
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
        let Some(modulo) = NonZeroU16::new(modulo).filter(|value| value.get() >= 2) else {
            log.push_err("hash_modulo_invalid");
            return None;
        };

        // Fast path: try pre-computed hash from hash_key_get.
        if let Some(ref hkg) = self.base.hash_key_get {
            if let Some((raw, src)) = self.extract_first(session, hkg) {
                if let Ok(v) = raw.parse::<u16>() {
                    if v < modulo.get() {
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
        let Some(calc) = self.base.hash_calc.as_ref() else {
            log.push("no_hash_calc, pass");
            return None;
        };

        if self.base.key_get.is_empty() {
            log.push("no_key_get, pass");
            return None;
        }

        let Some((key, src)) = self.extract_first(session, &self.base.key_get) else {
            log.push("key_missing, pass");
            return None;
        };

        let hash_val = compute_hash(&key, &calc.algorithm, modulo);

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
        if let Some(error) = self.init_error {
            log.push_err(error);
            return PluginRunningResult::GoodNext;
        }
        // Short-circuit: nothing configured at all.
        if self.base.route_rules.is_empty() && self.base.regions.is_empty() {
            log.push("no_region_route_config, pass");
            return PluginRunningResult::GoodNext;
        }

        // Resolve the effective region table: base + optional overlay (fail-to-base on error).
        let effective_regions = self.compute_effective_regions(session, log);

        if effective_regions.is_empty() {
            log.push("no_regions, pass");
            return PluginRunningResult::GoodNext;
        }

        // Evaluate route rules in order — first match wins.
        for rule in &self.base.route_rules {
            match rule {
                // ── RouteByDirectRegion ────────────────────────────────────────
                RouteRule::RouteByDirectRegion { key_from } => {
                    let Some((name, src)) = self.extract_first(session, key_from) else {
                        continue;
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
                    let Some((value, src)) = self.extract_first(session, &self.base.key_get) else {
                        continue;
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
#[path = "../../../../tests/unit/plugins/http/region_route/plugin_test.rs"]
mod tests;
