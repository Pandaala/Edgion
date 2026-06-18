# Metrics rules

> Core principle of metrics: the Gateway **data plane** avoids metrics explosion (no Histogram + strict label cardinality + unified `GatewayMetrics` management). Data-plane exceptions do not get a separate counter/tracing — they go through the access log (`AccessLogEntry.errors` + `EdgionStatus`). The Controller **control plane** also restrains adding metrics; control-plane events go through `tracing::{info,warn,error}!` + status condition first, and only get a metric when the case is "second-level alert threshold" or "core SLI for long-term Grafana trend". Detailed observation → access log / structured tracing log aggregation; do not extend metrics in Prometheus.

## Core principles

- **No Histogram type in the Gateway data plane** (no `_bucket` / `_sum` / `_count` triple)
  - **Scope**: every per-request hot-path metric under `edgion-gateway/src/`. Because the data plane is hit on every request, the Cartesian product of histogram buckets × multiple labels easily explodes (a single histogram emits `N_buckets × high cardinality` time series), and Prometheus storage and scrape cannot cope.
  - **The only existing exception**: `edgion_gateway_request_duration_ms` — Gateway end-to-end request latency, a core SLI (p50/p95/p99 are the main basis for production alerts). The Gateway data plane has only this single histogram; it is gated by the `MetricsConfig::request_duration` config item, off by default and written only with explicit `enabled=true`. Buckets are injected explicitly via `set_buckets_for_metric` (see `gateway/observe/metrics/api.rs`), and labels are per-label togglable.
  - **To add a second histogram on the data plane**: first revise this rule + reasoning (why counter/gauge cannot express it, why access log aggregation is insufficient), and treat it as an explicit topic in code review.
- **Controller control plane proactively restrains new metrics**: scope is `edgion-controller/src/`, controller-side federation helpers in `edgion-common/src/observe/fed_metrics.rs`, etc. While control-plane cardinality risk is low (low trigger frequency, label values driven by deployment scale), "easy to add" is not a reason "to add" — control-plane events **go through `tracing::{info,warn,error}!` structured logs + K8s status condition (`Accepted/Ready/Programmed`) by default**, and the log-collection side aggregates the answer to "how many times this kind of event occurred / which resources are problematic". A metric is only considered when the event satisfies one of:
  - **Needs to act as a second-level alert threshold** (e.g., `edgion_fed_watch_errors_total` triggers a Prometheus alert)
  - **Core SLI for long-term Grafana trend** (low-dimensional, high-value health signal, e.g., `edgion_fed_connected_controllers`)
  - **Cross-process aggregation** (single-instance logs can't see the whole picture; needs Prometheus sum/rate)

  **Histograms off by default**: the control plane currently has 0 histograms; keep it that way. If one is to be added: (a) explain why a `_seconds_last` gauge / `_total` counter cannot express the distribution; (b) explain why log aggregation is insufficient; (c) set buckets explicitly to avoid the `+Inf` trap (see the original `MIRROR_DURATION_MS` incident).

  **Counter/gauge off by default**: discrete events like validate rejection, resource conflict, illegal config are sufficiently covered by `tracing::warn!(ns, name, field, reason, "...")` + status condition; do not "casually" add a counter like `edgion_controller_validate_rejected_total{reason}` — every reason is a cardinality decision and easily expands without discipline.
- **Finer observation goes through access log aggregation, not metric expansion**: most "what happened on this request / how long it took / which backend it went to" questions already have raw data in the access/mirror/ssl/tls/tcp/udp access logs (JSON structured fields). Aggregation in Loki / Elasticsearch / ClickHouse at query time produces percentile / rate / distribution along any dimension. Prometheus is for **second-level alerts + low-cardinality aggregation**; the access log is for **high-cardinality detail replay**. Their division of labor is clear:
  - "What is the gateway error rate now?" → Prometheus counter `edgion_backend_requests_total{status=~"5xx|failed"}` (the gateway-level `edgion_requests_failed_total` was removed: it was never wired up, and the backend-labeled counter is a strictly superior signal)
  - "What exactly happened on that 5xx just now? x-trace-id is abc123" → access log
  - "What's mirror's p99 latency?" → aggregate the `et` field of the mirror access log; **do not** add a histogram
  - "What's a backend's 2xx ratio?" → counter rate, already exists
- **Strictly control label cardinality**: label values must be bounded and predictable (namespace, name, status group, etc.). **Never** use high-cardinality values like path, user_id, trace_id as labels. This rule applies to both data plane and control plane.
- **Hybrid registration policy**: the handles in `GatewayMetrics` are split into two categories by "call hotness":
  - **Hot (per-request path)** → `Counter`/`Gauge` stored directly; eagerly registered in `new()` via `counter!()/gauge!()`. Zero per-call overhead; a brief 0 value before the first request is acceptable (ctx_created, ctx_active, gateway_request_bytes, gateway_response_bytes)
  - **Cold / failure path / low-frequency control plane** → stored as `OnceLock<Counter>/<Gauge>`, registered only on the first record call. Untriggered metrics do not appear in `/metrics`, avoiding zero-value noise and scrape waste
  - Free functions with labels (`record_backend_request` / `record_cb_*` / `record_mirror_*` / `record_acme_*`) are naturally lazily registered — uninvoked label combinations have no time series
- **Before adding a metric, ask**: can an existing metric express it? Can it be derived without storing extra? Can a field already in the access log express it via aggregation (usually yes)?
- **Naming convention**: `edgion_<component>_<what>_<unit>_total / _active`, all lowercase with underscores

---

## Steps to add a new metric

`edgion-gateway/src/observe/metrics/registry.rs` — the only metrics definition file; all metrics are unified here. The Prometheus exposure entry is `edgion-gateway/src/observe/metrics/api.rs`.

**1. Define the constant name in the `names` mod**

```rust
pub mod names {
    pub const MY_NEW_COUNTER: &str = "edgion_gateway_my_event_total";
}
```

**2. Decide Hot vs Cold first; the field type follows**

Criterion: **per-request path = Hot** (definitely non-zero within seconds); **failure / low frequency / may never trigger = Cold**.

```rust
pub struct GatewayMetrics {
    // --- Hot per-request handles (eager registration) ---
    my_hot_counter: Counter,              // Counter / Gauge only — no Histogram

    // --- Cold / rare-event handles (lazy registration via OnceLock) ---
    my_cold_counter: OnceLock<Counter>,
}
```

**3. Initialize by category in `GatewayMetrics::new()`**

```rust
fn new() -> Self {
    Self {
        // Hot: eager
        my_hot_counter: counter!(names::MY_HOT_COUNTER),
        // Cold: lazy
        my_cold_counter: OnceLock::new(),
    }
}
```

**4. Add an `#[inline]` record method**

```rust
// Hot: directly .increment(), zero extra overhead
#[inline]
pub fn my_hot_event(&self) {
    self.my_hot_counter.increment(1);
}

// Cold: go through the get_or_register_counter / get_or_register_gauge helper
#[inline]
pub fn my_cold_event(&self) {
    get_or_register_counter(&self.my_cold_counter, names::MY_COLD_COUNTER).increment(1);
}
```

> **Gauge symmetry**: A cold gauge's decrement must be preceded by a corresponding increment that triggers registration; otherwise -1 drift can occur. See the `gateway_connected` / `gateway_disconnected` pairing for `controller_connected_gateways`. Hot gauges are unaffected (registered to 0 at startup).

**5. Use it at the call site**

```rust
use crate::core::gateway::observe::metrics::global_metrics;

global_metrics().my_event();
```

---

## Prohibitions

| Forbidden | Scope | Reason |
|------|--------|------|
| `metrics::histogram!(...)` | **Gateway data plane** | The histogram × label Cartesian product on the hot path easily explodes. **Only the existing `edgion_gateway_request_duration_ms` is exempt** |
| Using path / user_id / ip etc. as label values | Data plane + control plane | Causes metrics explosion (time-series count = number of label combinations) |
| Each plugin registers its own metrics | Gateway data plane | Distributed management is hard to audit; unified through `GatewayMetrics` |
| Counter incremented with floats | Data plane + control plane | Use the integer `increment(n: u64)` |
| Adding a latency histogram for fire-and-forget / shadow traffic | Data plane + control plane | Failure signal goes via counter (e.g., `channel_full`, `timeout` status); detailed latency is sufficient via access log aggregation |

---

## Reasonable label usage

Labels are only for bounded enums (cardinality ≤ tens):

```rust
// Reasonable label: bounded enum
counter!(
    names::BACKEND_REQUESTS_TOTAL,
    "status" => status_group(ctx.request_info.status),   // "2xx"/"3xx"/"4xx"/"5xx"/"failed"
    "protocol" => "grpc",
    "gateway_name" => gateway_name,                       // bounded number of instances
)

// Dangerous label: high cardinality
counter!(
    "edgion_requests",
    "path" => request_path,    // unbounded paths → cardinality explosion
    "user_id" => user_id,      // same
)
```

### Counters with multi-dimensional resource labels: use config + conditional push

For metrics like `edgion_backend_requests_total` and `edgion_gateway_request_duration_ms` that simultaneously carry gateway / route / backend / protocol / status multi-dimensional labels, **do not push all labels in one go**; follow the pattern of `record_request_duration` / `record_backend_request`:

1. Define an `XxxLabels` struct in `gateway/cli/config.rs` whose fields are a bool toggle per resource category. **Do not split `*_namespace` and `*_name` into two toggles** — K8s resource identity is naturally paired; one toggle controls the pair
2. Hang the config off `MetricsConfig`; a global `OnceLock<...Config>` is initialized at startup (see `init_request_duration_config` / `init_backend_requests_config`)
3. The first parameter of the record function takes `&XxxLabels`; internally use `Vec::with_capacity(N)` + `labels.push(...)` conditionally accumulated by toggle, and finally `counter!(..., &labels[..]).increment(1)`
4. Default values are conservative: enable the gateway identity + bounded enum labels (protocol/status); do not enable route/backend. Let operators **explicitly opt-in** to drill-down dimensions
5. For optional labels gated by a runtime flag (test_key / test_data), even if not controlled by a struct field, they must still **conditionally push (push only when non-empty)**, to avoid time series with `label_name=""`

Any new "per-request path × multi-dimensional resource label" counter/gauge must follow this pattern; otherwise cardinality may be blown by a single misconfiguration.

## Acceptable metric types

| Type | Gateway data plane | Controller / Center control plane | Examples |
|------|-------------|-------------------------|------|
| `Counter` | Yes | Yes | Total requests, total errors, bytes |
| `Gauge` | Yes | Yes | Active connection count, connected gateway count |
| `Histogram` | Forbidden (only the SLI exception) | Allowed but restrained, 0 by default | Data plane: `edgion_gateway_request_duration_ms` (existing SLI) |

---

## Gauge symmetry

When using a Gauge, increments and decrements must be symmetric; otherwise the metric drifts long-term and becomes meaningless.

For Gauge operations, prefer enforcing symmetry in a `Drop` implementation or an explicit RAII guard (see the `Drop for EdgionHttpContext` impl for the `ctx_active` Gauge).

---

## Controller / Center Metrics

The Controller and Center are the control plane and do not run on the data-plane hot path. While cardinality risk is low, **proactively restrain new metrics** (not only histograms but also counter/gauge) — control-plane events go through `tracing` structured logs + status condition by default, and log-collection-side aggregation answers most "how many times happened / which resources have issues" questions.

### Decision threshold for adding a control-plane metric

Before adding a control-plane metric, ask three questions; **any "no" means use tracing/status instead**:

1. **Is this signal needed as a second-level alert threshold?** E.g., `edgion_fed_watch_errors_total > 0` triggers oncall — yes → add a counter; no → `tracing::warn!` + log alert
2. **Is this a core SLI for long-term Grafana trends?** E.g., "the number of controllers connected to the federation" — yes → add a gauge; no → just count K8s resources / `kubectl get` at query time
3. **Can the whole picture not be seen in single-instance logs?** E.g., aggregating a certain class of events across all controllers — yes → add a metric; no → query structured-log fields

**Typical signals that should NOT get a metric** (use tracing + status condition):
- Validate-rejection events (illegal match_type, unknown allowedRoutes.from, schema field mismatch, etc.) → `tracing::warn!(ns, name, field, reason)` + Accepted:False condition; aggregating `{reason="UnsupportedMatchType"}` on the log side is enough
- Specific cause classification of resource parse / reconcile failure → already in the status condition; do not restate via a counter
- "How many times this config combination was used" → log at debug/info; aggregate at query time

Implementation rules:

1. **Unified entry**: install the global `PrometheusRecorder` via `edgion-common/src/observe/metrics_api.rs::install_global_recorder(RecorderConfig)`. Each in-repo service binary calls it once near `run()` (for example, `service: "edgion-controller"` or `"edgion-gateway"`)
2. **`/metrics` reuses the admin router**: call `crate::core::common::observe::metrics_api::metrics_handler` and mount it on the `/metrics` path of each axum Router; do not open a separate port
3. **Auth skip**: `/metrics` is hard-coded in `PUBLIC_PROBE_PATHS` of `edgion-controller/src/api/public_paths.rs` and is exempt from auth alongside `/health` and `/ready` (it is also served on a dedicated metrics listener). Do not modify this static public set when adding sensitive routes
4. **Registration style**: follow the style of `edgion-controller/src/conf_mgr/sync_runtime/metrics.rs` — direct `counter!/gauge!` macros without `OnceLock` lazy wrapping (control-plane call frequency is low; eager registration suffices)
5. **Federation metrics**: Controller-side federation metrics go through `edgion-common/src/observe/fed_metrics.rs` (`edgion_fed_*` prefix). Center-side implementation lives in the separate EdgionCenter repository. Label values **must** be `&'static str` constants from the `labels::` submodule; do not splice strings directly
6. **If a histogram really must be added**: control plane allows it, but: (a) explain why a `_seconds_last` gauge / counter cannot express it; (b) explain why access log aggregation is insufficient; (c) set buckets explicitly via `set_buckets_for_metric` or inject through `RecorderConfig.histogram_buckets` (avoid the default seconds buckets mismatching ms-recorded values, which dumps everything into `+Inf` — see the original `MIRROR_DURATION_MS` incident)

## Test Metrics exception

`edgion-gateway/src/observe/metrics/test_metrics.rs` is a test-only data-collection module designed for integration tests, **not subject to the production metrics rules above**.

### Characteristics

- Only activated when `--integration-testing-mode` is on
- Explicitly enabled via Gateway annotation (`edgion.io/metrics-test-type`); not triggered in production
- The `TestType` enum controls the data type collected (`Lb` / `Retry` / `Latency`)
- Data is attached as a `test_data` JSON label on `backend_requests_total` for test assertions

### Adding a new test data type

In `metrics/test_metrics.rs`:

1. Add a variant to `TestType`
2. Add a corresponding field in `TestData` (`#[serde(skip_serializing_if = "Option::is_none")]`)
3. Add a `set_xxx_test_data()` function
4. Handle it in the `match test_type` branch of `build_test_data()` in `pg_logging.rs`

### Scenario: monitor the frequency of a class of events

First check whether `metrics/registry.rs` already has a suitable Counter. If not:
1. Confirm that cardinality is bounded
2. Add it uniformly in `metrics/registry.rs` per the steps above
3. Do not create floating metrics directly with the `metrics::counter!()` macro inside the local module

### Scenario: integration test needs to verify a specific behavior

Extend `TestData` in `metrics/test_metrics.rs`; do not pollute the production access log or metrics. Test data is explicitly activated via Gateway annotation; production has zero overhead.
