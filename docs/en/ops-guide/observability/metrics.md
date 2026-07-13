# Metrics

> **🔌 Edgion extension**
> 
> The `edgion_*` metric names and the configurable `backend_requests` label set are Edgion features; they are not part of standard Gateway API.

Edgion exposes Prometheus metrics from the Gateway, Controller, and Center processes. The metric surface is deliberately small: the Gateway data plane uses only counters and gauges (with one optional histogram), and high-cardinality, per-request questions are answered by the [access log](./access-log.md) rather than by adding labels.

## Overview

Each process exposes a `/metrics` endpoint in Prometheus text format on its own independent listener. For the Controller and Center this is a dedicated metrics port (`12190` / `12290`), separate from the admin/API port — `/metrics` is no longer served on the admin port.

| Process | Default endpoint | Path | Auth |
|---------|------------------|------|------|
| Gateway | `:12090` | `/metrics` | None (independent listener) |
| Controller | `:12190` | `/metrics` | None (independent listener; separate from admin port `12101`) |
| Center | `:12290` | `/metrics` | None (independent listener; separate from admin port `12201`) |

`/metrics` deliberately skips authentication, following the Kubernetes controller-manager convention. The output is aggregated numbers only — it never contains request bodies, and by default it does not contain resource names (see the cardinality notes below).

## Scrape configuration

### Prometheus Operator (ServiceMonitor)

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: edgion-gateway
  namespace: monitoring
spec:
  selector:
    matchLabels:
      app: edgion-gateway
  namespaceSelector:
    matchNames:
      - gateway-system
  endpoints:
    - port: metrics
      interval: 15s
      path: /metrics
```

### Plain Prometheus scrape config

```yaml
scrape_configs:
  - job_name: edgion-gateway
    metrics_path: /metrics
    static_configs:
      - targets: ["10.0.0.10:12090"]
  - job_name: edgion-controller
    metrics_path: /metrics
    static_configs:
      - targets: ["10.0.0.11:12190"]   # dedicated metrics listener, not the admin port
  - job_name: edgion-center
    metrics_path: /metrics
    static_configs:
      - targets: ["10.0.0.12:12290"]   # dedicated metrics listener, not the admin port
```

## Gateway metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `edgion_backend_requests_total` | Counter | configurable (see below) | The only per-request counter. Gateway-level totals and failures are derived from this via `sum(...)` and the `status` label |
| `edgion_ctx_active` | Gauge | none | Active in-flight request contexts |
| `edgion_gateway_request_duration_ms` | Histogram | none | End-to-end request latency (the only histogram on the data plane). **Disabled by default** |
| `edgion_mirror_requests_total` | Counter | `status` | Request-mirror outcomes. `status` ∈ `ok` / `channel_full` / `connect_err` / `connect_timeout` / `write_err` / `timeout` |
| `edgion_rr_selector_truncated_total` | Counter | none | RoundRobin selector truncation events (backend count exceeded `u16::MAX`); used to alert on configuration anomalies |
| `edgion_lb_panic_mode` | Gauge | `service` | `1` when load-balancer panic mode is engaged for the named service (healthy fraction below the configured threshold), `0` when normal. `service` format: `namespace/name`. |
| `edgion_link_sys_es_bulk_channel_full_total` | Counter | none | Elasticsearch producer→bulk-loop channel was full at submit time; a sustained-backpressure signal on the log-shipping path |
| `edgion_outbound_template_fallback_total` | Counter | `field` (`timeout_ms` \| `allow_degradation`) | A webhook / ForwardAuth `timeoutMsTemplate` or `allowDegradationTemplate` render failed to parse and fell back to the static field. See [Connectors → Templates](../infrastructure/connectors.md#templates) |
| `edgion_outbound_success_predicate_fail_total` | Counter | none | A webhook / ForwardAuth response's status was accepted but a `success.body` predicate classified it as a failure (the "200 OK but error in the body" case). See [Connectors → Success](../infrastructure/connectors.md#success) |

### `edgion_backend_requests_total` labels

The labels on this counter are controlled entirely by the `metrics.backend_requests.labels` configuration, so you can trade detail against cardinality. The defaults are conservative:

```yaml
metrics:
  backend_requests:
    labels:
      # Each switch below controls a namespace+name pair (or a single enum)
      gateway: true     # gateway_namespace + gateway_name
      route: false      # route_namespace + route_name
      backend: false    # backend_namespace + backend_name
      protocol: true    # bounded enum: http / grpc / websocket
      status: true      # bounded enum: 2xx / 3xx / 4xx / 5xx / failed
```

| Switch | Labels added | Default | Notes |
|--------|--------------|---------|-------|
| `gateway` | `gateway_namespace`, `gateway_name` | `true` | Gateway resource identity |
| `route` | `route_namespace`, `route_name` | `false` | Enable for per-route traffic; raises cardinality |
| `backend` | `backend_namespace`, `backend_name` | `false` | Enable for per-backend error rate; raises cardinality |
| `protocol` | `protocol` | `true` | Bounded enum; cardinality cost is essentially zero |
| `status` | `status` | `true` | Bounded enum; cardinality cost is essentially zero |

> Changing these toggles requires a Gateway **restart** — the label set is fixed when the metric is first registered.

⚠️ The `test_key` / `test_data` labels appear **only** under `--integration-testing-mode` with an explicit Gateway annotation opt-in. `test_data` varies per request, so it is unbounded in cardinality. **Never enable integration-testing mode in production.**

### Enabling the latency histogram

`edgion_gateway_request_duration_ms` is off by default. Enable it under the `metrics.request_duration` section:

```yaml
metrics:
  request_duration:
    enabled: true
```

Like `backend_requests`, the histogram's labels are individually toggleable (gateway / route / backend namespace+name, protocol, status) under `metrics.request_duration.labels`, and a disabled label is omitted entirely rather than written as an empty string. Keep the label set minimal — every active label multiplies the number of histogram series.

## Controller metrics

The Controller exposes control-plane metrics under the `edgion_controller_*` prefix on its dedicated metrics listener (`:12190`), separate from the admin port (`:12101`).

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `edgion_controller_active` | Gauge | none | Number of active resource controllers |
| `edgion_controller_leader` | Gauge | none | Leader election status (1 = leader, 0 = standby) |
| `edgion_controller_resources_watched` | Gauge | `kind` | Resources currently watched, per kind |
| `edgion_controller_init_sync_duration_seconds_last` | Gauge | `kind` | Duration of the last initial sync, per kind |
| `edgion_controller_reload_total` | Counter | none | Total reload operations triggered |
| `edgion_controller_reload_duration_seconds_last` | Gauge | none | Duration of the last reload |
| `edgion_controller_reload_client_notifications_total` | Counter | none | Client notifications sent for reloads |
| `edgion_controller_status_writes_total` | Counter | `kind`, `backend`, `result` | Status writes (`backend` ∈ kubernetes/filesystem, `result` ∈ success/error) |
| `edgion_controller_status_writes_skipped_total` | Counter | `kind` | Status writes skipped because a semantic diff found no change |
| `edgion_controller_unexpected_apply_during_init_total` | Counter | `kind` | Apply events received during init (should stay 0; non-zero indicates watcher-stream ordering anomalies) |
| `edgion_controller_resource_abnormal` | Gauge | `kind`, `namespace`, `name`, `err`, `warn` | One series per resource that has at least one failing status condition. `err` / `warn` are JSON arrays of the triggering conditions; empty string means none at that severity |

The `edgion_controller_resource_abnormal` snapshot is rebuilt on a timer and capped (default 1000 entries) to bound cardinality. Use it to alert on resources stuck in a bad state, e.g. `edgion_controller_resource_abnormal{err!=""}`.

## Federation metrics (Center and Controller)

Federation baseline metrics are exposed under the `edgion_fed_*` prefix. All label value domains are fixed enums.

| Metric | Type | Labels | Process |
|--------|------|--------|---------|
| `edgion_fed_connections_active` | Gauge | `role` (center/controller) | Both |
| `edgion_fed_connection_events_total` | Counter | `role`, `event` (connected/disconnected/reload) | Both |
| `edgion_fed_connection_duration_seconds_last` | Gauge | `role` | Both |
| `edgion_fed_watch_events_total` | Counter | `kind`, `direction` (sent/recv) | Both |
| `edgion_fed_watch_list_total` | Counter | `kind`, `result` | Center |
| `edgion_fed_watch_errors_total` | Counter | `kind`, `reason` | Center |
| `edgion_fed_mark_offline_total` | Counter | `reason` | Center |
| `edgion_fed_peer_identity_check_total` | Counter | `result` | Center |
| `edgion_fed_aggregator_controllers` | Gauge | `cluster` | Center |
| `edgion_fed_aggregator_keys_total` | Gauge | `kind` | Center |
| `edgion_fed_fanout_total` | Counter | `op`, `result` | Center |
| `edgion_fed_ready_gate_wait_seconds_last` | Gauge | none | Controller |

## Cardinality

⚠️ The single most important cardinality lever is the `edgion_backend_requests_total` label set.

With the defaults (a single Gateway), the counter produces about **15 series** (1 gateway × 3 protocols × 5 statuses). Enabling more drill-down multiplies this:

| Configuration | Approximate series |
|---------------|--------------------|
| Default | 15 |
| `+ backend` | 15 × N_backend |
| `+ backend + route` | 15 × N_backend × N_route |

Recommendation: start with the default plus `backend: true`, watch the live series count for `{__name__="edgion_backend_requests_total"}`, and only enable `route: true` once you know the cost. For per-request detail, use the [access log](./access-log.md) — do not solve it by expanding labels.

## Common scenarios

### 1. Fleet error rate alert

`edgion_backend_requests_total` carries `status` by default, so an error-rate alert needs no extra labels:

```text
sum(rate(edgion_backend_requests_total{status=~"5xx|failed"}[5m]))
  / sum(rate(edgion_backend_requests_total[5m]))
```

### 2. Per-backend error rate

Enable the `backend` label, restart the Gateway, then query per backend:

```yaml
metrics:
  backend_requests:
    labels:
      backend: true
```

```text
sum by (backend_name) (rate(edgion_backend_requests_total{status="5xx"}[5m]))
```

### 3. Latency SLO

Enable the histogram (scenario above), then compute a percentile:

```text
histogram_quantile(0.99, sum(rate(edgion_gateway_request_duration_ms_bucket[5m])) by (le))
```

## FAQ

### Q1: Why is `edgion_gateway_request_duration_ms` missing from `/metrics`?

**A**: It is disabled by default. Set `metrics.request_duration.enabled: true` and restart the Gateway. Until then, derive latency by aggregating the access log's timing fields (`ht`, `et`).

### Q2: I added a label under `backend_requests.labels` but it did not appear. Why?

**A**: The label set is fixed when the counter is first registered, so you must restart the Gateway after changing the toggles. Hot reload does not change the metric's labels.

### Q3: There is no "total requests" or "failed requests" counter. How do I get those?

**A**: Both are derived from `edgion_backend_requests_total`. Total requests is `sum(edgion_backend_requests_total)`; failures are `sum(edgion_backend_requests_total{status=~"5xx|failed"})`. Edgion intentionally avoids redundant counters.

### Q4: How do I detect resources stuck in a bad state?

**A**: Scrape the Controller `/metrics` and alert on `edgion_controller_resource_abnormal{err!=""}`. Each series identifies one resource by `kind` / `namespace` / `name`, with the failing conditions in the `err` / `warn` labels.

### Q5: Is `/metrics` safe to expose without authentication?

**A**: The endpoint returns aggregated numbers only — no request bodies, and (at default settings) no resource names. Restrict network access at the cluster level if needed, but the data itself is not sensitive.

## Get help

If you run into problems:

1. Check the FAQ section of this document
2. Check the gateway and controller logs
3. Open an issue on GitHub

## Related documents

- [Access Log](./access-log.md)
- [Gateway Configuration](../gateway/README.md)
- [edgion-cli](../edgion-cli.md)
