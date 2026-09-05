---
name: metrics-features
description: Prometheus endpoints, exposure model, configuration, and authoritative metric sources.
---

# Metrics

## Endpoints and Exposure

| Process | Default listener | Paths | Authentication |
|---------|------------------|-------|----------------|
| Gateway | `0.0.0.0:12090` | `/metrics`, `/health` | None |
| Controller | `0.0.0.0:12190` | `/metrics` | None |

Both are dedicated listeners, separate from Admin and probe ports. The federation
Center is built and documented in the separate EdgionCenter repository; there is no
Center binary or `:12290` endpoint in this workspace.

The scrape endpoints contain no request bodies or Secret values, but they are not
anonymous-safe metadata. Labels may expose Gateway, Route, Backend, Service, LinkSys,
Wasm module, namespace, resource name, kind, and status/error identity. Restrict the
metrics listeners with a Kubernetes NetworkPolicy, firewall, or private scrape network.

## Authoritative Sources

- Gateway names and recorders: `edgion-gateway/src/observe/metrics/registry.rs`
- Gateway HTTP exposure: `edgion-gateway/src/observe/metrics/api.rs`
- Controller recorder installation/exposure: `edgion-common/src/observe/metrics_api.rs`
- Controller resource/status append block: `edgion-controller/src/observe/status_metrics.rs`
- Controller-side federation helpers: `edgion-common/src/observe/fed_metrics.rs`

Metric inventories change more often than this guide. Search the sources above before
adding alerts or claiming that a family exists. Important groups include request/byte
counters, active contexts/connections, circuit breaking and degrade mode, mirror results,
log drops, LinkSys health/backpressure, FIPS/system-error state, Wasm runtime metrics,
and Controller sync/status/certificate-expiry signals.

## Histograms and Cardinality

The Gateway currently has three deliberately reviewed histograms:

| Metric | Purpose | Cardinality note |
|--------|---------|------------------|
| `edgion_gateway_request_duration_ms` | End-to-end request SLI; disabled by default | Resource labels are individually configurable |
| `edgion_wasm_module_compile_duration_ms` | Wasm compilation latency | `module` must remain bounded by deployed modules |
| `edgion_wasm_hook_duration_ms` | Wasm hook execution latency | Hot-path `module` label; monitor series count |

Do not interpret this as permission to add another histogram casually. New hot-path
histograms require an explicit cardinality/bucket review and an explanation of why
access-log aggregation or a counter/gauge is insufficient. The coding rules live in
[../../03-coding/observability/01-metrics.md](../../03-coding/observability/01-metrics.md).

## Backend Request Counter

`edgion_backend_requests_total` is the primary request/error counter. Derive overall
traffic and failures by summing/filtering its `status` label; there is no separate
gateway request-total/failure family. Its resource labels are startup configuration:

```yaml
metrics:
  backend_requests:
    labels:
      gateway: true     # gateway_namespace + gateway_name
      route: false      # route_namespace + route_name
      backend: false    # backend_namespace + backend_name
      protocol: true    # bounded: http/grpc/websocket
      status: true      # bounded: 2xx/3xx/4xx/5xx/failed
```

Changing these fields requires a restart. Enabling route/backend identity multiplies
series count by the number of those resources; use access logs for per-request replay.
`test_key`/`test_data` labels exist only under the doubly gated integration-testing mode
and must never be enabled in production.

## Request Duration Configuration

```yaml
metrics:
  request_duration:
    enabled: false
    buckets: [1, 5, 10, 25, 50, 100, 200, 500, 1000, 2500, 5000, 10000]
    idle_timeout_secs: 600
    labels:
      route_name: true
      backend_name: true
      protocol: true
      status: true
```

The histogram is off by default. Review every enabled resource label and the resulting
bucket multiplier before enabling it broadly.

## Controller-Side Federation Metrics

The current workspace defines these Controller/common families under `edgion_fed_`:

- `connections_active`
- `connection_events_total`
- `connection_duration_seconds_last`
- `watch_events_total`
- `ready_gate_wait_seconds_last`
- `rbac_denied_total`
- `kill_switch_state` — the operator's `center.enabled` intent only
- `client_running` — whether the federation client is actually running here; only
  the leader replica runs it, so a follower reports `kill_switch_state=1` with
  `client_running=0`

Use the exact full names and bounded label domains from
`edgion-common/src/observe/fed_metrics.rs`. Center-side watch aggregation, takeover,
fanout, consistency, and peer metrics belong to EdgionCenter and must be verified there.

## Responsibility Guide

| Question | Signal |
|----------|--------|
| Current error rate / alerting | `rate(edgion_backend_requests_total{status=~"5xx|failed"}[...])` |
| One request by trace ID | Access log |
| Mirror delivery failures | `edgion_mirror_requests_total{status=...}` |
| Mirror latency distribution | Aggregate mirror-log timing fields |
| Resource reconciliation problem | Controller status condition plus structured log; resource-abnormal scrape block for current state |
| Certificate expiry | `edgion_conf_sync_cert_expiry_timestamp_seconds - time()` |

Prometheus is for bounded operational signals. Access/protocol logs and status conditions
carry high-cardinality diagnostic detail.
