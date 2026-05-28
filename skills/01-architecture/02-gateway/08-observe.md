---
name: gateway-observe
description: Gateway observability — zero-copy AccessLog JSON, protocol logs, Prometheus metrics, AccessLogStore for integration testing.
---

# Gateway observability

> The observability module provides three dimensions of runtime insight: access logs, protocol logs, and Prometheus metrics.

## Iron rule for data plane observability

**Data plane hot paths (`pg_*` callbacks, plugin runtime, stream routes, session_persistence, TLS callbacks, etc.) MUST NOT use any `tracing::` macro (including `tracing::debug!`).**

The only allowed observability channels on the data plane are:

1. AccessLog (per-request JSON log for HTTP/gRPC, zero-copy).
2. Protocol logs (`ssl_log` / `tcp_log` / `tls_log` / `udp_log`).
3. Prometheus metrics (`metrics::counter!` / `histogram!` / `gauge!`).
4. Error code accumulation (`EdgionStatus`, written into `EdgionHttpContext.errors` and ultimately landing in the AccessLog `errors` field).

Control-plane and startup logs use `tracing` and are not subject to this rule. See [`../../03-coding/01-log-safety.md`](../../03-coding/01-log-safety.md) iron rule 3 for details.

## Access Log

### AccessLogEntry

`AccessLogEntry<'a>` is a zero-copy access-log entry; all fields reference data inside `EdgionHttpContext`:

```rust
pub struct AccessLogEntry<'a> {
    pub timestamp: i64,
    pub request_info: &'a RequestInfo,
    pub match_info: Option<RouteMatchInfo<'a>>,  // HTTP or gRPC route match info
    pub errors: &'a [EdgionStatus],
    pub backend_context: Option<&'a BackendContext>,
    pub stage_logs: &'a [StageLogs],             // plugin execution logs
    pub conn_est: Option<bool>,                   // whether the upstream connection is established
    pub ctx: &'a HashMap<String, String>,         // custom context variables
}
```

- `RouteMatchInfo` is an `untagged` enum supporting both HTTP (`MatchInfo`) and gRPC (`GrpcMatchInfo`) match info.
- Built from EdgionHttpContext via `AccessLogEntry::from_context(ctx)`.
- JSON serialization uses serde's `skip_serializing_if` to reduce output size.

### AccessLogger

AccessLogger is a log dispatcher that sends the log to the first healthy DataSender:

```rust
pub struct AccessLogger {
    senders: Vec<Box<dyn DataSender<String>>>,
}
```

- `register()` — register a DataSender (e.g., LocalFileWriter, Elasticsearch DataSender).
- `init()` — initialize all senders.
- `send()` — iterate senders and send to the first one whose `healthy()` returns true.

Configurable output targets:
- **LocalFile** — local files (with log rotation).
- **LinkSys** — external systems (Elasticsearch, Redis, etc.).

## Protocol logs

Each protocol has its own logger, initialized via `init_*_logger()`:

| Protocol | Init function | Log entry | Description |
|----------|---------------|-----------|-------------|
| SSL | `init_ssl_logger()` | `log_ssl()` | SSL/TLS handshake logs |
| TCP | `init_tcp_logger()` | `TcpLogEntry` / `log_tcp()` | TCP connection logs |
| TLS | `init_tls_logger()` | `log_tls()` | TLS route logs |
| UDP | `init_udp_logger()` | `UdpLogEntry` / `log_udp()` | UDP session logs |

Logging infrastructure:
- `create_async_logger()` — create an asynchronous logger.
- `create_sync_logger()` — create a synchronous logger.
- `init_default()` / `init_logging()` — initialize the default logging system (`SysLogConfig`).
- `logs/buffer.rs` — log buffering.
- `logs/logger_factory.rs` — logger factory.

## Prometheus metrics

### Exposed port

Metrics are exposed on port `:5901` via Axum routes:
- `GET /metrics` — Prometheus-format metrics.
- `GET /health` — health-check endpoint.

### Metric registry

`GatewayMetrics` is a global singleton that registers the following metrics using the `metrics` crate:

| Metric name | Type | Description |
|-------------|------|-------------|
| `edgion_ctx_created_total` | Counter | Total contexts created |
| `edgion_ctx_active` | Gauge | Currently active contexts |
| `edgion_requests_total` | Counter | Total requests |
| `edgion_requests_failed_total` | Counter | Total failed requests |
| `edgion_access_log_dropped_total` | Counter | Number of dropped access logs |
| `edgion_ssl_log_dropped_total` | Counter | Number of dropped SSL logs |
| `edgion_tcp_log_dropped_total` | Counter | Number of dropped TCP logs |
| `edgion_tls_log_dropped_total` | Counter | Number of dropped TLS logs |
| `edgion_udp_log_dropped_total` | Counter | Number of dropped UDP logs |
| `edgion_status_update_total` | Counter | Total K8s status updates |
| `edgion_status_update_failed_total` | Counter | Failed K8s status updates |
| `edgion_status_update_skipped_total` | Counter | Skipped K8s status updates |
| `edgion_config_reload_signals_total` | Counter | Configuration reload signals |
| `edgion_config_relist_total` | Counter | Configuration relist count |
| `edgion_config_event_parse_errors_total` | Counter | Watch-event JSON parse failures (label: `kind`); fires when `ClientCache::start_watch` cannot deserialize a `WatchResponse.data` batch or a per-event `T` payload — primary symptom of controller↔gateway schema drift |
| `edgion_backend_requests_total` | Counter | Total backend requests (grouped by backend) |
| `edgion_gateway_request_bytes_total` | Counter | Gateway request bytes |
| `edgion_gateway_response_bytes_total` | Counter | Gateway response bytes |
| `edgion_mirror_requests_total` | Counter | Mirror requests (the `status` label distinguishes `ok` / `channel_full` / `connect_err` / `connect_timeout` / `write_err` / `timeout`; mirror is fire-and-forget; for per-request latency look up the `et` field in the mirror access log — no longer exposed as a histogram) |
| `edgion_controller_connected_gateways` | Gauge | Number of connected gateway instances |
| `edgion_controller_schema_validation_errors_total` | Counter | Schema validation error count |

Global label: `service=edgion-gateway`.

Helper functions:
- `record_backend_request()` — record backend request metrics.
- `record_mirror_metric()` — record mirror metrics.
- `status_group()` — status-code grouping.

### Test metrics

`test_metrics.rs` provides metric data injection for integration testing:
- `set_latency_test_data()` — set latency test data.
- `set_lb_test_data()` — set LB test data.
- `set_retry_test_data()` — set retry test data.

## AccessLogStore (integration testing mode)

An in-memory log store activated only under `--integration-testing-mode`:

```rust
pub struct AccessLogStore {
    entries: DashMap<String, StoredEntry>,  // trace_id -> JSON
    ttl: Duration,                          // default 5 minutes
    max_capacity: usize,                    // default 10,000 entries
    total_stored: AtomicU64,
}
```

- Indexed by `trace_id` for exact lookup.
- DashMap provides lock-free concurrent access.
- TTL-expired entries are cleaned automatically (triggered every 100 stores).
- When at capacity, expired entries are cleaned first.
- API: `store()`, `get()`, `delete()`, `list()`, `clear()`, `status()`.
- Global singleton accessed via `get_access_log_store()`.

## Directory layout

```
src/core/gateway/observe/
├── mod.rs                    # module exports
├── access_log/               # access logs
│   ├── entry.rs              # AccessLogEntry definition (zero-copy)
│   ├── logger.rs             # AccessLogger (DataSender dispatch)
│   └── mod.rs
├── access_log_store.rs       # AccessLogStore (in-memory store for integration testing)
├── logs/                     # protocol logs
│   ├── ssl_log.rs            # SSL log
│   ├── tcp_log.rs            # TCP log
│   ├── tls_log.rs            # TLS log
│   ├── udp_log.rs            # UDP log
│   ├── sys_log.rs            # system log configuration
│   ├── buffer.rs             # log buffering
│   ├── logger_factory.rs     # logger factory
│   └── mod.rs
└── metrics/                  # Prometheus metrics
    ├── api.rs                # Axum HTTP endpoint (:5901)
    ├── registry.rs           # GatewayMetrics definition
    ├── test_metrics.rs       # test-only metric injection
    └── mod.rs
```
