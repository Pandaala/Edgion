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

> The full per-metric catalog (name, type, labels, self/business category,
> description, alert hint) is maintained in the authoritative
> [Metrics Inventory](./metrics-inventory.md), kept in sync with the code by the
> `check_metrics_inventory.py` CI guard. This page keeps the scrape configuration,
> cardinality guidance, and common scenarios.

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

### Request-body storage saturation

`edgion_request_body_storage_io_inflight` reports how many fixed storage-I/O
permits are held. A sustained value of `32` together with growth in
`edgion_request_body_storage_io_failures_total{reason="admission_timeout"}`
means disk-backed request-body work is saturated before submission to the
blocking pool. `reason="execution_timeout"` instead means an admitted file
operation exceeded `requestBody.storage.operationTimeout`.

### Optional GenAI token counter

`edgion_gateway_genai_tokens_total` is disabled by default. Enable it in the Gateway
process configuration when Prometheus token-rate or trend aggregation is useful:

```yaml
metrics:
  genai_tokens:
    enabled: true
```

This is a startup-time setting; changing it requires a Gateway restart. When disabled,
the metric family is not registered. The counter has only three fixed-enum labels:

- `direction`: `input` or `output`
- `provider`: `openai`, `openai_compatible`, or `anthropic`
- `usage_source`: `reported` or `estimated`

Their Cartesian product is bounded to at most twelve series per Gateway process. There
is deliberately no `model` label, so model names cannot expand Prometheus cardinality.
Use `rate(edgion_gateway_genai_tokens_total[5m])` for rates rather than treating the raw
counter as durable state: every Gateway process restart resets its counters.

When enabled, the counter aggregates the reported or bounded estimated usage of every
dispatched attempt, including attempts followed by a retry. A pre-dispatch failure contributes
nothing. It does not export resource, model, credential, or request identity labels.

This metric is **not** a persistent usage or billing ledger. Independently of the metric
switch, the access log records the final request summary and a bounded `genai.attempts`
ledger with per-attempt usage source, tokens, cost, status/failure, Retry-After, and TTFT.
An ambiguous result may retain the full input-plus-maximum-output Redis reservation for
rate-limit safety while its durable log records only observed or bounded estimated usage;
the retained reservation is not asserted consumption. Ship and retain access logs when
durable accounting is required; do not use Prometheus counter values as authoritative billing.

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

> The full per-metric catalog (name, type, labels, self/business category,
> description, alert hint) is maintained in the authoritative
> [Metrics Inventory](./metrics-inventory.md), kept in sync with the code by the
> `check_metrics_inventory.py` CI guard. This page keeps the scrape configuration,
> cardinality guidance, and common scenarios.

The `edgion_controller_resource_abnormal` snapshot is rebuilt on a timer and capped (default 1000 entries) to bound cardinality. Use it to alert on resources stuck in a bad state, e.g. `edgion_controller_resource_abnormal{err!=""}`.

### Unknown config field alert

`edgion_controller_unknown_config_fields` is a live (not timer-based) scan appended at the
same `/metrics` seam, rendered fresh on every scrape from `EdgionPlugins`, `EdgionStreamPlugins`,
`EdgionConfigData`, and `LinkSys` `spec.unknown_fields`. It emits one count series per affected resource
kind; a healthy fleet emits nothing. Unlike `resource_abnormal`, this gauge is populated in FileSystem
and etcd conf_center mode too, not Kubernetes-mode only — see
[Unknown Config Field Detection](../plugin-config-unknown-fields.md) for why.

The only label is the bounded `kind` enum. Resource names and finding paths stay in the
diagnostics endpoint, status condition, and Controller warning log; they are intentionally not
Prometheus labels because their historical value domain is unbounded. This metric is purely
advisory and never blocks a sync.

```promql
edgion_controller_unknown_config_fields > 0
```

Alert on a sustained non-empty result if you want CI/dashboards to catch config typos that the
Controller itself deliberately does not reject.

### Leader-job death alert

`edgion_controller_leader_job_alive` has one series per leader-job worker that has ever started **on this
replica** — the series is **absent**, not `0`, for a job that never started there (e.g.
`conf_sync_rotation` is never spawned on a replica with `conf_sync_security.auto_ca` off, so it mints no
`{leader_job="conf_sync_rotation"}` series at all; an absent series correctly suppresses the alert instead
of paging forever on a job that was never supposed to run).

```promql
edgion_controller_leader_job_alive == 0
  and on(instance) edgion_controller_leader == 1
```

Alert with `for: 2m` — the gauge legitimately dips to `0` for a moment during every normal backoff-restart
iteration of the worker, so a shorter window pages on routine restarts instead of an actually-dead worker.
`on(instance) edgion_controller_leader
== 1` scopes the alert to the replica currently holding leadership — a `0` on a non-leader replica (job
never started there) is expected and must not page. The `and` keeps the `leader_job` label from the left
side in the alert output, so the page names which job died.

By design there is **no automatic restart** of a dead leader-job worker (a panic usually means corrupted
state; a blind restart risks a panic loop) — recovery is by alert + operator action
(`POST /api/v1/reload` or a pod restart). Separately, `/health` now returns `503` if the control-plane
lifecycle itself terminates abnormally (a deadman, distinct from this per-worker gauge) — see
[Supervision & recovery](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md#supervision--recovery)
for the full lifecycle-recovery design.

## Federation metrics (Center and Controller)

Federation baseline metrics are exposed under the `edgion_fed_*` prefix. All label value domains are fixed enums.

> The full per-metric catalog (name, type, labels, self/business category,
> description, alert hint) is maintained in the authoritative
> [Metrics Inventory](./metrics-inventory.md), kept in sync with the code by the
> `check_metrics_inventory.py` CI guard. This page keeps the scrape configuration,
> cardinality guidance, and common scenarios. Note: the inventory covers only the
> `edgion_fed_*` metrics emitted from this repository (Controller-side); the
> federation Center process now lives in the separate EdgionCenter repository and
> its Center-only metrics (`edgion_fed_watch_list_total`, `edgion_fed_aggregator_*`,
> etc.) are out of scope here.

## System error state

`edgion_system_error_state` is a single gauge registering **"the process is running, but in a known
degraded state"**. It is emitted by both the Gateway and the Controller (hence its own section here),
and is `1` while a condition is active, `0` or absent otherwise.

| Metric | Type | Labels | Process |
|--------|------|--------|---------|
| `edgion_system_error_state` | Gauge | `component`, `reason` | Both |

Both labels come from closed sets, so the whole family stays at a handful of series per pod. The
authoritative list of `component` / `reason` values is
[`edgion-common/src/observe/system_metrics.rs`](https://github.com/Pandaala/Edgion/blob/main/edgion-common/src/observe/system_metrics.rs);
each reason's doc comment states what raises it and whether it clears.

Two kinds of reason live here, and the difference decides your alert:

- **Latching** — a startup configuration decision (e.g. `skip_tls`, `cert_watch_failed`). Set once
  and never zeroed; only a restart clears it. Alert on `== 1` directly.
- **Transient** — a runtime condition the same process raises and clears (e.g.
  `confsync_disconnected`, `cert_rotation_failed`). Alert on a **sustained** `== 1` with a `for:`
  guard — a single blip is normal and pages on it will be noise.

The full PAGE / TICKET / log-only division of labor, with a recommended `for:` duration and the
reasoning behind each, is maintained in the
[conf_sync operations reference](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/02-config/00-controller-config.md)
(see its **Alerting** subsection). That is the canonical source; this page deliberately does not
restate it.

### Is a Gateway still in sync with the Controller?

Certificate expiry is a separate signal:
`edgion_conf_sync_cert_expiry_timestamp_seconds - time()` gives the installed
conf_sync certificate's remaining lifetime in seconds. Each Gateway exports its
loaded client certificate's expiry on `/metrics` scrapes; replacing the mounted
Secret does not advance the value until that Gateway successfully installs the
replacement. The Controller samples its live server identity hourly. A rejected
reload is also exposed by `edgion_system_error_state{reason="cert_reload_invalid"}`.
The expiry metric excludes EdgionTls and ordinary Gateway serving certificates.

The one query most worth wiring up from this family:

```promql
edgion_system_error_state{component="conf_sync_client",reason="confsync_disconnected"} == 1
```

Scrape it from the **Gateway** pods (`:12090`), not the Controller — each Gateway raises this on its
own registry, so the pod that is broken is the pod that reports it.

A `1` sustained for more than ~10 minutes means that Gateway's config-sync link is down and has not
recovered. This is **not** a traffic outage: the Gateway keeps serving from its last-known config on
purpose, so that a control-plane blip never costs data-plane availability. What it means is that the
Gateway's configuration is **frozen** — every route, backend, and certificate change from that point
on silently fails to reach it, so a rollout can report success while that pod keeps serving the old
world. Nothing else surfaces this: the pod stays `Ready` (readiness latches once at boot, by design)
and keeps returning 200s.

Use ~10 minutes rather than a tighter window: a Controller rolling restart legitimately flips the
gauge to 1 for a few seconds, and the Gateway's own reconnect backoff caps at 30s.

Scope: this tracks the Gateway's `WatchServerMeta` stream, which is what a transport-level failure
(dead Controller, expired cert, bad CA) breaks. A failure confined to a single resource kind's
watch — for example a list response this Gateway cannot parse — does not raise it; those surface in
the Gateway's logs instead.

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

### Q4b: How do I detect config typos in plugins, typed `EdgionConfigData`, or LinkSys that were silently ignored?

**A**: Alert on `edgion_controller_unknown_config_fields > 0` — one series per affected kind,
with the value equal to the affected-resource count. Fetch exact resource names and paths from
`GET /api/v1/diagnostics/unknown-fields`. See
[Unknown Config Field Detection](../plugin-config-unknown-fields.md); this stays advisory (the
Controller never rejects the resource because of it), so use the metric to drive your own alert
or dashboard, not a blocking gate.

### Q5: A Gateway is serving traffic fine, but my config changes are not taking effect on it. Why?

**A**: Its config-sync link to the Controller is probably down. Check
`edgion_system_error_state{component="conf_sync_client",reason="confsync_disconnected"}` on that
Gateway's own `:12090` — a sustained `1` means it never reconnected and is serving frozen config.
Note that `/ready` and traffic both stay green in this state by design, so neither will tell you.
See [System error state](#system-error-state).

### Q6: Is `/metrics` safe to expose without authentication?

**A**: The endpoint returns aggregated numbers only — no request bodies, and (at default settings) no resource names. Restrict network access at the cluster level if needed, but the data itself is not sensitive.

## Get help

If you run into problems:

1. Check the FAQ section of this document
2. Check the gateway and controller logs
3. Open an issue on GitHub

## Related documents

- [Metrics Inventory](./metrics-inventory.md) — the authoritative per-metric catalog
- [Access Log](./access-log.md)
- [Gateway Configuration](../gateway/README.md)
- [edgion-cli](../edgion-cli.md)
