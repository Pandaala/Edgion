//! Prepared dynamic metric descriptors attached to route units.
//!
//! See `docs/superpowers/specs/2026-06-10-observe-f002-route-unit-metric-handles-design.md`.
//! Hot-path contract: a request that finds its matrix entry pays 0 allocations,
//! 0 name/label construction or registry lookups after first use, plus short
//! per-metric gates that coordinate accumulation with idle eviction.

use edgion_common::observe::metrics_facade::{
    dynamic_counter as counter, dynamic_histogram as histogram, DynamicCounter as Counter,
    DynamicHistogram as Histogram,
};
use metrics::Label;

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
    /// Returns `true` on success. Returns `false` when either index is out of
    /// bounds or the precomputed counter/histogram slots are incomplete. The
    /// caller then falls back to the dynamic metrics path, so the request still
    /// emits its time series. All slots are validated before either metric is
    /// recorded, keeping the fallback atomic and preventing double-counting.
    /// No panic / `debug_assert!(false)` on the data-plane hot path (see
    /// `skills/03-coding/02-rust-coding-rules.md` Rule 5).
    #[inline]
    pub fn record(&self, backend_idx: usize, ps_idx: usize, duration_ms: f64) -> bool {
        let Some(counter) = self
            .counters
            .get(backend_idx)
            .and_then(Option::as_ref)
            .and_then(|counters| counters.get(ps_idx))
        else {
            return false;
        };

        let histogram = match self.histograms.as_ref() {
            Some(histograms) => {
                let Some(histogram) = histograms
                    .get(backend_idx)
                    .and_then(Option::as_ref)
                    .and_then(|histograms| histograms.get(ps_idx))
                else {
                    return false;
                };
                Some(histogram)
            }
            None => None,
        };

        counter.increment(1);
        if let Some(histogram) = histogram {
            histogram.record(duration_ms);
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
#[path = "../../../tests/unit/observe/metrics/route_handles_test.rs"]
mod tests;
