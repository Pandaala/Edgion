# Metrics rules

> Core principle: keep Gateway metrics bounded through strict label review and unified
> `GatewayMetrics` management. Most request detail belongs in access logs. Existing
> histograms are explicit exceptions, not a general pattern. The Controller similarly
> prefers structured tracing and status conditions unless a low-cardinality metric is
> needed for alerting, trends, or cross-process aggregation.

## Core principles

- **Do not add a Gateway histogram without explicit review** (`_bucket` / `_sum` / `_count` multiply every label combination)
  - **Scope**: every per-request hot-path metric under `edgion-gateway/src/`. Because the data plane is hit on every request, the Cartesian product of histogram buckets × multiple labels easily explodes (a single histogram emits `N_buckets × high cardinality` time series), and Prometheus storage and scrape cannot cope.
  - **Existing reviewed histograms**: `edgion_gateway_request_duration_ms` (off by default, configurable resource labels), `edgion_wasm_module_compile_duration_ms`, and `edgion_wasm_hook_duration_ms`. The Wasm families use a `module` label; keep deployed module identity bounded and monitor series count.
  - **To add another histogram**: explain why a counter/gauge and access-log aggregation are insufficient, define units and buckets, calculate the label × bucket bound, and make the exception an explicit review topic.
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
- **Strictly control label cardinality**: prefer compile-time enums such as status groups.
  Namespace/resource/module names scale with deployment size and require explicit opt-in,
  series-count estimates, and operational monitoring. Never use unbounded request values
  such as path, user ID, IP, or trace ID as labels.
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
use crate::observe::metrics::global_metrics;

global_metrics().my_event();
```

---

## Prohibitions

| Forbidden | Scope | Reason |
|------|--------|------|
| Unreviewed `metrics::histogram!(...)` | **Gateway data plane** | Existing request-duration and Wasm histograms are adjudicated exceptions; every new family needs bucket/cardinality review |
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
    "status" => status_group(ctx.request_info.status_option()), // "2xx"/"3xx"/"4xx"/"5xx"/"failed"
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

1. Define an `XxxLabels` struct in `edgion-gateway/src/cli/config.rs` whose fields are a bool toggle per resource category. **Do not split `*_namespace` and `*_name` into two toggles** — K8s resource identity is naturally paired; one toggle controls the pair
2. Hang the config off `MetricsConfig`; a global `OnceLock<...Config>` is initialized at startup (see `init_request_duration_config` / `init_backend_requests_config`)
3. The first parameter of the record function takes `&XxxLabels`; internally use `Vec::with_capacity(N)` + `labels.push(...)` conditionally accumulated by toggle, and finally `counter!(..., &labels[..]).increment(1)`
4. Default values are conservative: enable the gateway identity + bounded enum labels (protocol/status); do not enable route/backend. Let operators **explicitly opt-in** to drill-down dimensions
5. For optional labels gated by a runtime flag (test_key / test_data), even if not controlled by a struct field, they must still **conditionally push (push only when non-empty)**, to avoid time series with `label_name=""`

Any new "per-request path × multi-dimensional resource label" counter/gauge must follow this pattern; otherwise cardinality may be blown by a single misconfiguration.

## Acceptable metric types

| Type | Gateway data plane | Controller control plane | Examples |
|------|-------------|-------------------------|------|
| `Counter` | Yes | Yes | Total requests, total errors, bytes |
| `Gauge` | Yes | Yes | Active connection count, connected gateway count |
| `Histogram` | Exception-only | Allowed but restrained | Existing Gateway request-duration and two Wasm duration families |

---

## Gauge symmetry

When using a Gauge, increments and decrements must be symmetric; otherwise the metric drifts long-term and becomes meaningless.

For Gauge operations, prefer enforcing symmetry in a `Drop` implementation or an explicit RAII guard (see the `Drop for EdgionHttpContext` impl for the `ctx_active` Gauge).

---

## Controller Metrics and Federation Boundary

The Controller is control plane and does not run on the data-plane hot path. The Center
is maintained in the separate EdgionCenter repository. While Controller cardinality risk
is lower, proactively restrain all new metric types: structured logs plus status
conditions answer most event/detail questions.

### Decision threshold for adding a control-plane metric

Before adding a control-plane metric, ask three questions; **any "no" means use tracing/status instead**:

1. **Is this signal needed as a second-level alert threshold?** E.g., `edgion_fed_watch_errors_total > 0` triggers oncall — yes → add a counter; no → `tracing::warn!` + log alert
2. **Is this a core SLI for long-term Grafana trends?** E.g., "the number of controllers connected to the federation" — yes → add a gauge; no → just count K8s resources / `kubectl get` at query time
3. **Can the whole picture not be seen in single-instance logs?** E.g., aggregating a certain class of events across all controllers — yes → add a metric; no → query structured-log fields

**Typical signals that should NOT get a metric** (use tracing + status condition):
- Validate-rejection events (illegal match_type, unknown allowedRoutes.from, schema field mismatch, etc.) → `tracing::warn!(ns, name, field, reason)` + Accepted:False condition; aggregating `{reason="UnsupportedMatchType"}` on the log side is enough
- Specific cause classification of resource parse / reconcile failure → already in the status condition; do not restate via a counter
- "How many times this config combination was used" → log at debug/info; aggregate at query time

### Precondition: is the state per-process, or per-peer?

Ask this **before** the three questions above. "Restrain the metric → use a `system_metrics`
reason instead" only holds for state the **emitting process** owns. `edgion_system_error_state`
is keyed by `(component, reason)` and nothing else, so a condition observed once *per peer* —
one controller against many gateways — cannot live there: every healthy peer's success clears
the broken peer's mark, and a `sustained == 1` alert can never fire. **Reaching for a `peer`
label to fix that is worse**, especially pre-auth (a TLS handshake *is* the authentication),
where any unauthenticated caller could then mint unbounded series at will.

When neither fits, the answer is **not** "add a family anyway":

1. **Let the broken peer flag itself.** A per-peer condition is per-*process* on the other
   end — the peer that is failing can mark it on its own registry, where it has exactly one
   owner and nothing can mask it. This is also strictly better signal: it fires on the pod
   that is actually broken.
2. **Leave the per-peer detail in a rate-limited log.** The log carries the peer identity a
   bounded metric never could, so it is what you grep once (1) alerts.

Worked example (2026-07-16): `TLS_HANDSHAKE_FAILED` was added as a reason on
`CONF_SYNC_SERVER` under exactly the "no new metric families" constraint, and could never
fire — healthy gateways' handshakes cleared it continuously. It was **deleted**, not
converted to a counter: the gateway already flags its own conf_sync link via
`CONFSYNC_DISCONNECTED` (per-pod, unmaskable), and the controller's existing rate-limited
WARN names the peer. Net: one bad signal removed, one good one added, no new family. See the
amendment note atop `docs/superpowers/specs/2026-07-12-conf-sync-cert-rotation-design.md`,
and the "single owner" rule in `edgion-common/src/observe/system_metrics.rs`.

Implementation rules:

1. **Unified entry**: install the global `PrometheusRecorder` via `edgion-common/src/observe/metrics_api.rs::install_global_recorder(RecorderConfig)`. Each in-repo service binary calls it once near `run()` (for example, `service: "edgion-controller"` or `"edgion-gateway"`)
2. **Dedicated listener**: Controller uses `api::create_metrics_router()` on
   `server.metrics_listen`; Gateway uses `observe::metrics::serve_with_shutdown()` on
   `metrics.listen`. Do not mount scrape output onto the Admin listener.
3. **No application auth**: restrict the dedicated listener at the network layer. Metric
   labels may reveal resource identity even though request bodies and Secrets are absent.
4. **Registration style**: follow the style of `edgion-controller/src/conf_mgr/sync_runtime/metrics.rs` — direct `counter!/gauge!` macros without `OnceLock` lazy wrapping (control-plane call frequency is low; eager registration suffices)
5. **Federation metrics**: Controller-side federation metrics go through `edgion-common/src/observe/fed_metrics.rs` (`edgion_fed_*` prefix). Center-side implementation lives in the separate EdgionCenter repository. Protocol labels use `&'static str` constants from `labels::`; resource-kind labels use the canonical static `authz_classifier::bounded_kind_label` projection. Successful watch forwarding captures this projection from the already authorized canonical kind, so accepted spelling variants share one series (ARCH-082). Do not splice request strings into labels.
6. **If a histogram really must be added**: control plane allows it, but: (a) explain why a `_seconds_last` gauge / counter cannot express it; (b) explain why access log aggregation is insufficient; (c) set buckets explicitly via `set_buckets_for_metric` or inject through `RecorderConfig.histogram_buckets` (avoid the default seconds buckets mismatching ms-recorded values, which dumps everything into `+Inf` — see the original `MIRROR_DURATION_MS` incident)

### Arbitrated exception: `edgion_conf_sync_cert_expiry_timestamp_seconds`

The control-plane restraint default above says "reasons-only" — a discrete event should
be a `tracing::warn!` + status condition / `system_metrics` reason, not a new metric
family. This gauge is a deliberate, narrow exception to that default (ruled 2026-07-13),
kept alongside the reason constants added for conf_sync cert rotation (see
`docs/superpowers/specs/2026-07-12-conf-sync-cert-rotation-design.md` §6 Task 6):

- **Type**: `Gauge`.
- **Name**: `edgion_conf_sync_cert_expiry_timestamp_seconds`.
- **Labels**: `side` ∈ `{"server", "client"}` — 2 bounded series total, no cardinality risk.
- **Value/unit**: the in-service certificate's `notAfter`, Unix seconds.
- **Source**: the shared reporter in `edgion-common/src/conf_sync/cert_expiry_watch.rs`.
  The Controller exports hourly from its live hot-swappable identity via
  `CertSource::PemFn` (`side="server"`). Gateway `/metrics` scrapes export the
  preprocessed `LeafValidity` from the installed Channel snapshot (`side="client"`),
  through `GatewayMetrics`. Startup, recovery candidates, and TLS reload carry
  Channel and validity together; only successful publication changes the source.
  Scrapes never read certificate files or parse PEM. `EdgionTls` and ordinary
  Gateway serving identities use the same `LeafValidity` for runtime expiry checks;
  this metric remains scoped to conf_sync, not those serving certificates.
- **Staleness is by design**: on a read/parse failure the gauge is left at its last
  successfully observed value rather than cleared — see the module's
  `check_and_report`. A stuck value still keeps the expiry alert armed instead of going
  blank while the underlying read is failing; the watch's own WARN/ERROR logs (cold-start
  vs. persistent failure) carry the failure signal separately.
  Before Gateway has installed an identity, initialization errors and auto-CA retry
  diagnostics remain visible; no disk candidate is reported as an installed identity.
  Gateway severity-transition logs are evaluated on scrapes, not by a separate timer.
- **Why the exception**: `system_metrics` reasons like `CERT_ROTATION_FAILED` only fire if
  the rotation code path is still alive to raise them. This gauge instead watches the
  certificate actually in service, so it is the one signal that still moves if the whole
  rotation pipeline is wedged (dead reconciler, leader-election gap, a stopped file
  watcher) — a case where every reason-based signal above goes silent right when it
  matters most.
- **Alert rule shape**:
  ```text
  edgion_conf_sync_cert_expiry_timestamp_seconds - time() < 7 * 86400
  ```
  (paired with a 30-day WARN-band transition log from the same watch — see
  `skills/02-features/02-config/00-controller-config.md`'s Alerting subsection for the full
  division of labor across this gauge, `system_error` reasons, and log-only events.)

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
