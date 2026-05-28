---
name: metrics-features
description: Prometheus Metrics endpoints and available metrics.
---

# Metrics

## Endpoints

| Endpoint | Port | Path | Auth | Description |
|----------|------|------|------|-------------|
| Gateway Metrics | `:5901` | `/metrics` | None | Independent listening port, started via `serve_with_shutdown` |
| Controller Metrics | Admin API (default `:8080`) | `/metrics` | No auth (unified_auth skip_paths) | Mounted under the existing admin router |
| Center Metrics | Admin API (default `:5810`) | `/metrics` | No auth (unified_auth skip_paths) | Mounted under the existing router |

> Prometheus scrape skips auth following K8s controller-manager convention. `/metrics` content is aggregated numbers and does not contain resource names or request bodies.

## Metric Conventions

Edgion's Metrics design principles (strict for the Gateway data plane; looser for the control plane but governed by an "ask before adding" rule):

1. **No Histograms in the Gateway data plane**: histograms multiplied by many labels on the data plane per-request path can produce a Cartesian-product explosion. Use only Counter and Gauge. **The single existing exception**: `edgion_gateway_request_duration_ms` (a core SLI, disabled by default and requiring explicit opt-in, with explicitly configured buckets). Adding new histograms is forbidden — if you believe one is necessary, first amend the rules in `skills/03-coding/observability/01-metrics.md` and explain the reasoning in code review.
2. **Histograms are allowed but restrained on the Controller / Center control plane**: the control plane has low trigger frequency, label cardinality is bounded by deployment scale, and cardinality risk is low — histograms are **technically allowed**. However, following the "ask before adding a metric" principle, prefer counter/gauge + access log aggregation, and only add a histogram when distribution shape (not "most recent") is the core decision input. There are currently 0 histograms on the control plane (the `edgion_fed_*` / `edgion_controller_*` families are all counters and gauges).
3. **Bounded label cardinality**: each label's value domain is ≤ a few dozen (applies to both data plane and control plane).
4. **Managed via the GatewayMetrics struct**: do not directly use the `metrics::counter!()` macro (on the Gateway side; for Controller/Center see `common/observe/fed_metrics.rs` — the control plane may use macros directly).
5. **Gauge balance**: every +1 path has a matching -1.
6. **Hybrid registration strategy**: hot per-request paths (ctx_created / gateway_request_bytes, etc.) store `Counter`/`Gauge` directly with eager registration in `new()`; cold failure/low-frequency paths use `OnceLock<Counter>/<Gauge>` and register on first record — untouched metrics do not appear in `/metrics`, so zero values do not consume bandwidth.
7. **Fine-grained observation goes through access log aggregation**: Prometheus carries only **low-cardinality alerting + second-level aggregation**. Finer questions (per-request latency, per-target distributions, trace-level replay) have raw data in the access log (and `mirror_log` / `ssl_log` / `tls_log` / `tcp_log` / `udp_log`); use Loki / Elasticsearch / ClickHouse to aggregate at query time and you get rates / percentiles in any dimension. Do not expand Prometheus metrics for "I want more detail" needs.

### Observability Responsibility Cheat Sheet

| Question | Where to look |
|----------|---------------|
| "What is the gateway's current error rate? Should we alert?" | Prometheus counter `edgion_backend_requests_total{status=~"5xx\|failed"}` (no standalone "gateway request total/failed" counters — derive both from this multi-label counter) |
| "What happened on the request with x-trace-id=abc?" | access log |
| "Is mirror keeping up with primary traffic?" | Prometheus counter `edgion_mirror_requests_total{status="channel_full"\|"timeout"}` |
| "What is the p99 latency distribution of mirror?" | aggregate the `et` field in the mirror access log (do not add a histogram) |
| "What is the 503 ratio for some backend in the last 5 minutes?" | Prometheus counter `rate()` |
| "What were the upstream response headers for one specific 5xx?" | access log |

### Common Metrics

| Metric Name | Type | Description |
|-------------|------|-------------|
| `edgion_backend_requests_total` | Counter | Total backend requests; also the **only** per-request counter — gateway-level totals/failures are derived via `sum(...)` and `status` label filtering. Labels are **configurable** (see "backend_requests label configuration" below). By default only `gateway_namespace` + `gateway_name` + `protocol` + `status`; enable `route_*` / `backend_*` drill-down via `metrics.backend_requests.labels` when needed. ⚠️ `test_key` / `test_data` only appear under `--integration-testing-mode` with explicit Gateway annotation opt-in; `test_data` varies per request (containing ip / latency_ms, etc.), so **never enable this flag in production**. |
| `edgion_ctx_active` | Gauge | Currently active request contexts |
| `edgion_gateway_request_duration_ms` | **Histogram (the only exception on the Gateway data plane)** | End-to-end request latency, core SLI. Disabled by default; once enabled, buckets are configured via `MetricsConfig::request_duration`. |
| `edgion_mirror_requests_total{status}` | Counter | Result of each mirror by the request mirror plugin. `status` ∈ `ok` / `channel_full` / `connect_err` / `connect_timeout` / `write_err` / `timeout` (**mirror latency no longer has a histogram — query the `et` field in mirror_log**). |
| `edgion_rr_selector_truncated_total` | Counter | RoundRobin truncation events when backend count > `u16::MAX` (extremely rare, used to alert on configuration anomalies; no labels). |
| `edgion_link_sys_es_bulk_channel_full_total` | Counter | Producer→bulk-loop mpsc channel was full at submit time (no labels). Non-zero values signal sustained backpressure on the access-log path; raise `bulk.channelSize` (default 10000, ceiling 100000) before it surfaces as request latency. |

### `backend_requests` Label Configuration

The labels of `edgion_backend_requests_total` are entirely controlled by `metrics.backend_requests.labels`. Changing the default behavior requires a restart (OnceLock does not support hot reload). The defaults are conservative:

```yaml
metrics:
  backend_requests:
    labels:
      gateway: true     # Enable the gateway_namespace + gateway_name pair
      route: false      # Enable route_namespace + route_name
      backend: false    # Enable backend_namespace + backend_name
      protocol: true    # Bounded enum (http/grpc/websocket), cardinality cost is essentially zero
      status: true      # Bounded enum (2xx/3xx/4xx/5xx/failed), same as above
```

Design principles:

- Each `gateway` / `route` / `backend` switch controls a `*_namespace` + `*_name` pair — K8s resource identity is naturally a pair; configuring them separately is meaningless.
- The default 15 series (1 gateway × 1 gateway × 3 protocols × 5 statuses) is enough to support "error rate / traffic ratio"-style alerts.
- When you need "per-backend error rate" / "per-route traffic", explicitly enable `backend = true` / `route = true`, **knowing the cardinality cost** before doing so.
- For per-request detail replay, query the access log; do not solve it by expanding Prometheus labels.

Cardinality multiplier reference (single Gateway deployment):

| Default | With `backend` | With `backend` + `route` |
|---------|----------------|--------------------------|
| 15 series | 15 × N_backend | 15 × N_backend × N_route |

N_backend / N_route depend on cluster scale; in production, start with the default + `backend = true`, observe the Prometheus `{__name__="edgion_backend_requests_total"}` series count, then decide whether to enable `route`.

### Federation Metrics (Center + Controller)

All Federation baseline metrics are exposed under the `edgion_fed_` prefix. Definitions and helpers are in `src/core/common/observe/fed_metrics.rs`. All label value domains are compile-time constants (see the `labels` submodule in the same file).

| Metric | Type | Labels | Source Process |
|--------|------|--------|----------------|
| `edgion_fed_connections_active` | Gauge | `role` ∈ center/controller | Both |
| `edgion_fed_connection_events_total` | Counter | `role`, `event` ∈ connected/disconnected/reload | Both |
| `edgion_fed_connection_duration_seconds_last` | Gauge | `role` | Both |
| `edgion_fed_watch_events_total` | Counter | `kind`, `direction` ∈ sent/recv | Both |
| `edgion_fed_watch_list_total` | Counter | `kind`, `result` ∈ ok/version_too_old/parse_error/ready_wait | Center |
| `edgion_fed_watch_errors_total` | Counter | `kind`, `reason` ∈ parse_error/timeout/recv_error | Center |
| `edgion_fed_mark_offline_total` | Counter | `reason` ∈ heartbeat/disconnect/reload | Center |
| `edgion_fed_peer_identity_check_total` | Counter | `result` ∈ ok/mismatch/no_spiffe_san/multi_san/parse_error | Center |
| `edgion_fed_session_takeover_total` | Counter | (none) | Center |
| `edgion_fed_evict_stale_total` | Counter | `source` ∈ registry/aggregator | Center |
| `edgion_fed_session_reentry_total` | Counter | — | Center |
| `edgion_fed_aggregator_controllers` | Gauge | `cluster` | Center |
| `edgion_fed_aggregator_keys_total` | Gauge | `kind` | Center |
| `edgion_fed_consistency_mismatch_total` | Counter | — | Center |
| `edgion_fed_fanout_total` | Counter | `op` ∈ patch_enable/patch_profile, `result` ∈ ok/partial/fail | Center |
| `edgion_fed_ready_gate_wait_seconds_last` | Gauge | — | Controller |

> Note: the `edgion_fed_stream_tx_drops_total` from the original ticket is not yet implemented — fed_sync currently has no `try_send` path; adding this metric requires introducing backpressure first; it will be handled in a future ticket.

See [../../03-coding/observability/01-metrics.md](../../03-coding/observability/01-metrics.md) for the detailed metric conventions.
