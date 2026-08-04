//! Pre-resolved metric handles attached to route units.
//!
//! See `docs/superpowers/specs/2026-06-10-observe-f002-route-unit-metric-handles-design.md`.
//! Hot-path contract: a request that finds its matrix entry pays 0 allocations,
//! 0 hash lookups, 0 locks — only a handful of `&str` equality checks plus two
//! atomic ops.

use metrics::{counter, histogram, Counter, Histogram, Label};

use crate::cli::config::{
    get_backend_requests_config, get_request_duration_config, BackendRequestsLabels, RequestDurationConfig,
    RequestDurationLabels,
};
use crate::observe::metrics::registry::names;

/// Bounded protocol enum reflected in the `protocol` label.
pub const N_PROTOCOL: usize = 3;
/// Bounded status-group enum reflected in the `status` label.
pub const N_STATUS: usize = 5;
/// Flat-array stride for `(protocol, status)` combinations.
pub const N_PS: usize = N_PROTOCOL * N_STATUS;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ProtocolIdx {
    Http = 0,
    Grpc = 1,
    Websocket = 2,
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StatusIdx {
    S2xx = 0,
    S3xx = 1,
    S4xx = 2,
    S5xx = 3,
    Failed = 4,
}

#[inline]
pub fn ps_index(p: ProtocolIdx, s: StatusIdx) -> usize {
    (p as usize) * N_STATUS + (s as usize)
}

/// Map a runtime protocol string to the bounded enum. Returns `None` for
/// values that fall outside the documented `{http, grpc, websocket}` set
/// so the hot path falls back rather than indexing past the matrix.
#[inline]
pub fn protocol_to_idx(p: &str) -> Option<ProtocolIdx> {
    match p {
        "http" => Some(ProtocolIdx::Http),
        "grpc" => Some(ProtocolIdx::Grpc),
        "websocket" => Some(ProtocolIdx::Websocket),
        _ => None,
    }
}

/// Map an HTTP status code to the same status-group enum the existing
/// `status_group(...)` returns as a string.
#[inline]
pub fn status_to_idx(status: Option<u16>) -> StatusIdx {
    match status {
        Some(s) if (200..300).contains(&s) => StatusIdx::S2xx,
        Some(s) if (300..400).contains(&s) => StatusIdx::S3xx,
        Some(s) if (400..500).contains(&s) => StatusIdx::S4xx,
        Some(s) if (500..600).contains(&s) => StatusIdx::S5xx,
        _ => StatusIdx::Failed,
    }
}

pub struct GatewayMatrixEntry {
    pub gateway_ns: String,
    pub gateway_name: String,
    pub counters: Vec<Option<[Counter; N_PS]>>,
    pub histograms: Option<Vec<Option<[Histogram; N_PS]>>>,
}

impl GatewayMatrixEntry {
    /// Record one request into the precomputed handles at `(backend_idx, ps_idx)`.
    ///
    /// Returns `true` on success. Returns `false` when `backend_idx` falls
    /// outside the counter array (precomputed handles built against a stale set
    /// of `backend_refs`). The caller then falls back to the dynamic metrics
    /// path, so the request still emits its time series and the drift is
    /// observable from the access log. No panic / `debug_assert!(false)` on the
    /// data-plane hot path (see `skills/03-coding/02-rust-coding-rules.md` Rule 5).
    #[inline]
    pub fn record(&self, backend_idx: usize, ps_idx: usize, duration_ms: f64) -> bool {
        if backend_idx >= self.counters.len() {
            return false;
        }
        let Some(counters) = self.counters[backend_idx].as_ref() else {
            return false;
        };
        counters[ps_idx].increment(1);
        if let Some(hs) = self.histograms.as_ref() {
            let Some(histograms) = hs[backend_idx].as_ref() else {
                return false;
            };
            histograms[ps_idx].record(duration_ms);
        }
        true
    }
}

pub struct RouteMetricHandles {
    matrices: Vec<GatewayMatrixEntry>,
}

impl RouteMetricHandles {
    /// Empty handles stub: the hot path's gateway lookup misses and
    /// falls back to the dynamic metrics path.
    pub fn empty() -> Self {
        Self { matrices: Vec::new() }
    }

    #[inline]
    pub fn lookup(&self, gateway_ns: &str, gateway_name: &str) -> Option<&GatewayMatrixEntry> {
        self.matrices
            .iter()
            .find(|e| e.gateway_ns == gateway_ns && e.gateway_name == gateway_name)
    }

    /// Production entry point. Reads the global label configs (init-once
    /// `OnceLock`s — see spec §6) and delegates to `build_with`.
    pub fn build(
        attached_gateways: &[(String, String)],
        route_ns: &str,
        route_name: &str,
        backends: &[(String, String)],
    ) -> Self {
        let indexed_backends = backends.iter().cloned().map(Some).collect::<Vec<_>>();
        Self::build_sparse(attached_gateways, route_ns, route_name, &indexed_backends)
    }

    /// Build handles aligned to raw backendRef slots. `None` slots register no
    /// Service metric handles and force the caller onto the dynamic fallback.
    pub fn build_sparse(
        attached_gateways: &[(String, String)],
        route_ns: &str,
        route_name: &str,
        backends: &[Option<(String, String)>],
    ) -> Self {
        Self::build_sparse_with(
            attached_gateways,
            route_ns,
            route_name,
            backends,
            &get_backend_requests_config().labels,
            get_request_duration_config(),
        )
    }

    /// Same as `build`, but takes the configs explicitly. Used by tests
    /// that need to control label shape without touching the globals.
    pub fn build_with(
        attached_gateways: &[(String, String)],
        route_ns: &str,
        route_name: &str,
        backends: &[(String, String)],
        backend_cfg: &BackendRequestsLabels,
        duration_cfg: &RequestDurationConfig,
    ) -> Self {
        let indexed_backends = backends.iter().cloned().map(Some).collect::<Vec<_>>();
        Self::build_sparse_with(
            attached_gateways,
            route_ns,
            route_name,
            &indexed_backends,
            backend_cfg,
            duration_cfg,
        )
    }

    fn build_sparse_with(
        attached_gateways: &[(String, String)],
        route_ns: &str,
        route_name: &str,
        backends: &[Option<(String, String)>],
        backend_cfg: &BackendRequestsLabels,
        duration_cfg: &RequestDurationConfig,
    ) -> Self {
        let matrices = attached_gateways
            .iter()
            .map(|(gw_ns, gw_name)| {
                build_gateway_matrix(
                    gw_ns,
                    gw_name,
                    route_ns,
                    route_name,
                    backends,
                    backend_cfg,
                    duration_cfg,
                )
            })
            .collect();
        Self { matrices }
    }
}

fn build_gateway_matrix(
    gw_ns: &str,
    gw_name: &str,
    route_ns: &str,
    route_name: &str,
    backends: &[Option<(String, String)>],
    backend_cfg: &BackendRequestsLabels,
    duration_cfg: &RequestDurationConfig,
) -> GatewayMatrixEntry {
    let mut counters: Vec<Option<[Counter; N_PS]>> = Vec::with_capacity(backends.len());
    let mut histograms: Option<Vec<Option<[Histogram; N_PS]>>> = if duration_cfg.enabled {
        Some(Vec::with_capacity(backends.len()))
    } else {
        None
    };

    for backend in backends {
        let Some((b_ns, b_name)) = backend else {
            counters.push(None);
            if let Some(hs) = histograms.as_mut() {
                hs.push(None);
            }
            continue;
        };
        let counter_arr = std::array::from_fn(|cell_idx| {
            let (p, s) = unpack_ps(cell_idx);
            let labels = build_counter_labels(
                backend_cfg,
                gw_ns,
                gw_name,
                route_ns,
                route_name,
                b_ns,
                b_name,
                protocol_label_value(p),
                status_label_value(s),
            );
            counter!(names::BACKEND_REQUESTS_TOTAL, labels)
        });
        counters.push(Some(counter_arr));

        if let Some(hs) = histograms.as_mut() {
            let h_arr = std::array::from_fn(|cell_idx| {
                let (p, s) = unpack_ps(cell_idx);
                let labels = build_histogram_labels(
                    &duration_cfg.labels,
                    gw_ns,
                    gw_name,
                    route_ns,
                    route_name,
                    b_ns,
                    b_name,
                    protocol_label_value(p),
                    status_label_value(s),
                );
                histogram!(names::GATEWAY_REQUEST_DURATION_MS, labels)
            });
            hs.push(Some(h_arr));
        }
    }

    GatewayMatrixEntry {
        gateway_ns: gw_ns.to_string(),
        gateway_name: gw_name.to_string(),
        counters,
        histograms,
    }
}

#[inline]
fn unpack_ps(cell: usize) -> (ProtocolIdx, StatusIdx) {
    let p = match cell / N_STATUS {
        0 => ProtocolIdx::Http,
        1 => ProtocolIdx::Grpc,
        _ => ProtocolIdx::Websocket,
    };
    let s = match cell % N_STATUS {
        0 => StatusIdx::S2xx,
        1 => StatusIdx::S3xx,
        2 => StatusIdx::S4xx,
        3 => StatusIdx::S5xx,
        _ => StatusIdx::Failed,
    };
    (p, s)
}

#[inline]
fn protocol_label_value(p: ProtocolIdx) -> &'static str {
    match p {
        ProtocolIdx::Http => "http",
        ProtocolIdx::Grpc => "grpc",
        ProtocolIdx::Websocket => "websocket",
    }
}

#[inline]
fn status_label_value(s: StatusIdx) -> &'static str {
    match s {
        StatusIdx::S2xx => "2xx",
        StatusIdx::S3xx => "3xx",
        StatusIdx::S4xx => "4xx",
        StatusIdx::S5xx => "5xx",
        StatusIdx::Failed => "failed",
    }
}

#[allow(clippy::too_many_arguments)]
fn build_counter_labels(
    cfg: &BackendRequestsLabels,
    gw_ns: &str,
    gw_name: &str,
    route_ns: &str,
    route_name: &str,
    backend_ns: &str,
    backend_name: &str,
    protocol: &'static str,
    status: &'static str,
) -> Vec<Label> {
    let mut labels = Vec::with_capacity(8);
    if cfg.gateway {
        labels.push(Label::new("gateway_namespace", gw_ns.to_string()));
        labels.push(Label::new("gateway_name", gw_name.to_string()));
    }
    if cfg.route {
        labels.push(Label::new("route_namespace", route_ns.to_string()));
        labels.push(Label::new("route_name", route_name.to_string()));
    }
    if cfg.backend {
        labels.push(Label::new("backend_namespace", backend_ns.to_string()));
        labels.push(Label::new("backend_name", backend_name.to_string()));
    }
    if cfg.protocol {
        labels.push(Label::new("protocol", protocol));
    }
    if cfg.status {
        labels.push(Label::new("status", status));
    }
    labels
}

#[allow(clippy::too_many_arguments)]
fn build_histogram_labels(
    cfg: &RequestDurationLabels,
    gw_ns: &str,
    gw_name: &str,
    route_ns: &str,
    route_name: &str,
    backend_ns: &str,
    backend_name: &str,
    protocol: &'static str,
    status: &'static str,
) -> Vec<Label> {
    let mut labels = Vec::with_capacity(8);
    if cfg.gateway_namespace {
        labels.push(Label::new("gateway_namespace", gw_ns.to_string()));
    }
    if cfg.gateway_name {
        labels.push(Label::new("gateway_name", gw_name.to_string()));
    }
    if cfg.route_namespace {
        labels.push(Label::new("route_namespace", route_ns.to_string()));
    }
    if cfg.route_name {
        labels.push(Label::new("route_name", route_name.to_string()));
    }
    if cfg.backend_namespace {
        labels.push(Label::new("backend_namespace", backend_ns.to_string()));
    }
    if cfg.backend_name {
        labels.push(Label::new("backend_name", backend_name.to_string()));
    }
    if cfg.protocol {
        labels.push(Label::new("protocol", protocol));
    }
    if cfg.status {
        labels.push(Label::new("status", status));
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::config::{BackendRequestsLabels, RequestDurationConfig};
    use metrics_util::debugging::{DebuggingRecorder, Snapshotter};

    #[test]
    fn ps_index_packs_protocol_and_status() {
        assert_eq!(ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 0);
        assert_eq!(ps_index(ProtocolIdx::Http, StatusIdx::Failed), 4);
        assert_eq!(ps_index(ProtocolIdx::Grpc, StatusIdx::S2xx), 5);
        assert_eq!(ps_index(ProtocolIdx::Websocket, StatusIdx::Failed), 14);
    }

    #[test]
    fn protocol_to_idx_accepts_known_values() {
        assert_eq!(protocol_to_idx("http"), Some(ProtocolIdx::Http));
        assert_eq!(protocol_to_idx("grpc"), Some(ProtocolIdx::Grpc));
        assert_eq!(protocol_to_idx("websocket"), Some(ProtocolIdx::Websocket));
    }

    #[test]
    fn protocol_to_idx_rejects_unknown_values() {
        assert_eq!(protocol_to_idx(""), None);
        assert_eq!(protocol_to_idx("HTTP"), None);
        assert_eq!(protocol_to_idx("kafka"), None);
    }

    #[test]
    fn status_to_idx_groups_codes() {
        assert_eq!(status_to_idx(Some(200)), StatusIdx::S2xx);
        assert_eq!(status_to_idx(Some(204)), StatusIdx::S2xx);
        assert_eq!(status_to_idx(Some(301)), StatusIdx::S3xx);
        assert_eq!(status_to_idx(Some(404)), StatusIdx::S4xx);
        assert_eq!(status_to_idx(Some(503)), StatusIdx::S5xx);
        assert_eq!(status_to_idx(Some(199)), StatusIdx::Failed);
        assert_eq!(status_to_idx(Some(600)), StatusIdx::Failed);
        assert_eq!(status_to_idx(None), StatusIdx::Failed);
    }

    #[test]
    fn lookup_misses_on_empty_handles() {
        let h = RouteMetricHandles::empty();
        assert!(h.lookup("ns", "gw").is_none());
    }

    fn run_with_recorder<F: FnOnce() -> R, R>(f: F) -> (Snapshotter, R) {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let r = metrics::with_local_recorder(&recorder, f);
        (snapshotter, r)
    }

    #[test]
    fn build_registers_one_counter_and_histogram_per_cell() {
        let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
        let backends = vec![
            ("ns1".to_string(), "svc_a".to_string()),
            ("ns1".to_string(), "svc_b".to_string()),
        ];
        // Enable backend labels so each backend produces a distinct label set;
        // without them both backends collapse to the same N_PS keys.
        let backend_cfg = BackendRequestsLabels {
            backend: true,
            ..BackendRequestsLabels::default()
        };
        let duration_cfg = RequestDurationConfig::default();

        let (snap, handles) = run_with_recorder(|| {
            RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg)
        });

        // Sanity: lookup hits.
        let entry = handles.lookup("gw_ns", "gw_a").expect("gateway entry registered");
        assert_eq!(entry.counters.len(), 2);
        assert_eq!(entry.counters[0].as_ref().unwrap().len(), N_PS);

        // Cardinality: 1 gw x 2 backends x 15 ps = 30 unique counter keys.
        // DebuggingRecorder deduplicates by key, so the snapshot size reflects unique registrations.
        let snapshot = snap.snapshot();
        let names: Vec<String> = snapshot
            .into_vec()
            .into_iter()
            .map(|(key, _, _, _)| key.key().name().to_string())
            .collect();
        let n_counter = names.iter().filter(|n| n.contains("backend_requests_total")).count();
        let n_hist = names.iter().filter(|n| n.contains("request_duration_ms")).count();
        let expected = 2 * N_PS;
        assert_eq!(n_counter, expected, "expected {} counters, got {}", expected, n_counter);
        let expected_hist = if duration_cfg.enabled { expected } else { 0 };
        assert_eq!(
            n_hist, expected_hist,
            "expected {} histograms, got {}",
            expected_hist, n_hist
        );
    }

    #[test]
    fn build_registers_histograms_when_duration_enabled() {
        let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
        let backends = vec![("ns1".to_string(), "svc_a".to_string())];
        let backend_cfg = BackendRequestsLabels {
            backend: true,
            ..Default::default()
        };
        let duration_cfg = RequestDurationConfig {
            enabled: true,
            ..Default::default()
        };

        let (snap, handles) = run_with_recorder(|| {
            RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg)
        });

        let entry = handles.lookup("gw_ns", "gw_a").unwrap();
        assert!(entry.histograms.is_some());
        assert_eq!(entry.histograms.as_ref().unwrap().len(), 1);

        let snapshot = snap.snapshot();
        let names: Vec<String> = snapshot
            .into_vec()
            .into_iter()
            .map(|(key, _, _, _)| key.key().name().to_string())
            .collect();
        let n_hist = names.iter().filter(|n| n.contains("request_duration_ms")).count();
        assert_eq!(n_hist, N_PS);
    }

    #[test]
    fn record_succeeds_for_in_bounds_backend_index() {
        let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
        let backends = vec![("ns1".to_string(), "svc_a".to_string())];
        let backend_cfg = BackendRequestsLabels {
            backend: true,
            ..Default::default()
        };
        let duration_cfg = RequestDurationConfig::default();

        let (_, ok) = run_with_recorder(|| {
            let handles =
                RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
            let entry = handles.lookup("gw_ns", "gw_a").unwrap();
            entry.record(0, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 1.0)
        });
        assert!(ok, "in-bounds backend index must record successfully");
    }

    #[test]
    fn record_falls_back_on_out_of_bounds_backend_index() {
        let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
        // A single backend → only index 0 is valid; index 1 is out of bounds.
        let backends = vec![("ns1".to_string(), "svc_a".to_string())];
        let backend_cfg = BackendRequestsLabels {
            backend: true,
            ..Default::default()
        };
        let duration_cfg = RequestDurationConfig::default();

        let (_, ok) = run_with_recorder(|| {
            let handles =
                RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
            let entry = handles.lookup("gw_ns", "gw_a").unwrap();
            // Out-of-bounds index must NOT panic and must return false.
            entry.record(1, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 1.0)
        });

        assert!(!ok, "out-of-bounds backend index must return false (graceful fallback)");
    }

    #[test]
    fn sparse_build_preserves_raw_slots_without_registering_non_service_metrics() {
        let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
        let backends = vec![None, Some(("ns1".to_string(), "svc_a".to_string()))];
        let backend_cfg = BackendRequestsLabels {
            backend: true,
            ..Default::default()
        };
        let duration_cfg = RequestDurationConfig::default();

        let (snap, (invalid_slot, service_slot)) = run_with_recorder(|| {
            let handles =
                RouteMetricHandles::build_sparse_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
            let entry = handles.lookup("gw_ns", "gw_a").unwrap();
            (
                entry.record(0, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 1.0),
                entry.record(1, ps_index(ProtocolIdx::Http, StatusIdx::S2xx), 1.0),
            )
        });

        assert!(!invalid_slot);
        assert!(service_slot);
        let registered = snap
            .snapshot()
            .into_vec()
            .into_iter()
            .filter(|(key, _, _, _)| key.key().name().contains("backend_requests_total"))
            .count();
        assert_eq!(registered, N_PS);
    }

    #[test]
    fn handle_path_emits_same_labels_as_fallback() {
        use crate::cli::config::{BackendRequestsLabels, RequestDurationConfig};
        use crate::observe::metrics::registry::record_backend_request;

        let gws = vec![("gw_ns".to_string(), "gw_a".to_string())];
        let backends = vec![("ns1".to_string(), "svc_a".to_string())];
        // Need backend label on so two emitters produce identical full label sets
        // (same shape as build_registers_one_counter_and_histogram_per_cell).
        let backend_cfg = BackendRequestsLabels {
            backend: true,
            ..Default::default()
        };
        let duration_cfg = RequestDurationConfig::default();

        let (snap, _) = run_with_recorder(|| {
            let handles =
                RouteMetricHandles::build_with(&gws, "rns", "route_a", &backends, &backend_cfg, &duration_cfg);
            // Drive both paths once for the same (gw, route, backend, protocol, status):
            let entry = handles.lookup("gw_ns", "gw_a").unwrap();
            entry.counters[0].as_ref().unwrap()[ps_index(ProtocolIdx::Http, StatusIdx::S2xx)].increment(1);
            // Fallback emits with the same input shape:
            record_backend_request(
                &backend_cfg,
                "gw_ns",
                "gw_a",
                "rns",
                "route_a",
                "ns1",
                "svc_a",
                "http",
                "2xx",
                "",
                "",
            );
        });

        // Both should land in the SAME time series — collect by key.
        // Only count non-zero entries: build_with pre-registers all N_PS cells in the
        // recorder (value = 0); we only want the cells that were actually incremented.
        let snapshot = snap.snapshot().into_vec();
        let counter_entries: Vec<_> = snapshot
            .iter()
            .filter(|(k, _, _, v)| {
                k.key().name() == crate::observe::metrics::registry::names::BACKEND_REQUESTS_TOTAL
                    && matches!(v, metrics_util::debugging::DebugValue::Counter(n) if *n > 0)
            })
            .collect();
        // Exactly one unique key with the same label set; value = 2.
        assert_eq!(
            counter_entries.len(),
            1,
            "handle path and fallback registered different keys: {:?}",
            counter_entries
                .iter()
                .map(|(k, _, _, _)| k.key().labels().collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
    }
}
