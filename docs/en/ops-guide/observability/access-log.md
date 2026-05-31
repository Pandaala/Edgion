# Access Log

> **🔌 Edgion extension**
> 
> The access log format and plugin log structure are Edgion Gateway features; they are not part of standard Gateway API.

Edgion Gateway emits one structured JSON access log entry per request, recording the full request lifecycle: client information, route matching, backend responses, per-plugin execution logs, and errors. It is the gateway's primary mechanism for diagnosing individual requests.

## Overview

The access log is the right tool when you need to answer a question about a *specific* request — "what happened to the request with `x-trace-id=abc`?". Aggregate metrics (see [Metrics](./metrics.md)) answer fleet-wide questions; the access log answers per-request ones.

Each entry is one JSON object on a single line, so the file is directly consumable by `jq`, Loki, Elasticsearch, or any line-oriented log pipeline.

```
client request ──► Gateway ──► backend
                     │
                     └──► one JSON line ──► logs/edgion_access.log
```

## Configuration

The access log is configured under the `access_log` section of the gateway configuration.

### Minimal configuration

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
```

This enables the log with default queue and rotation behavior. `path` is resolved relative to `work_dir` unless absolute.

### Production configuration

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # In-memory queue before backpressure applies to the request path
      queue_size: 80000
      rotation:
        strategy: daily
        max_files: 7
```

### Configuration parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | Boolean | `false` | Enable the access log channel |
| `output.localFile.path` | String | none | Log file path (relative to `work_dir` unless absolute). **Required** when `localFile` is used |
| `output.localFile.queue_size` | Integer | none | In-memory queue size before backpressure applies |
| `output.localFile.rotation.strategy` | Enum | none | Rotation strategy. Example value: `daily` |
| `output.localFile.rotation.max_files` | Integer | none | Maximum rotated files to keep |
| `output.elasticsearch` | Object | none | Ship the access log to Elasticsearch (via `link_sys`) instead of, or in addition to, a local file |

The raw client-entry URI (`request_uri`) is controlled by a separate top-level `access_log_options` section, not by the `access_log` block — see [Special fields](#special-fields).

> The `output` block also accepts an `elasticsearch` sink. When the Elasticsearch producer→bulk-loop channel fills up, the `edgion_link_sys_es_bulk_channel_full_total` counter increments — watch it as a backpressure signal. See [Metrics](./metrics.md).

## Log format

The log is always JSON, one entry per line. Optional fields are omitted entirely when empty (rather than emitted as `null`), which keeps each line compact. The top-level shape is:

```json
{
  "ts": 1767089146376,
  "client-addr": "192.168.1.20",
  "host": "api.example.com",
  "path": "/health",
  "status": 200,
  "match_info": { },
  "backend_context": { },
  "plugin_logs": [ ],
  "info": [ ],
  "errors": [ ]
}
```

Request-scoped fields (`client-addr`, `host`, `path`, `status`, etc.) are flattened directly into the top-level object; routing, backend, and plugin information are nested.

## Field reference

### Top-level fields

| Field | Type | Description |
|-------|------|-------------|
| `ts` | Integer | Entry timestamp, epoch milliseconds |
| `match_info` | Object | Matched route identity (omitted when no route matched) |
| `backend_context` | Object | Backend identity and upstream timing (omitted/`null` when no backend was used) |
| `plugin_logs` | Array\<Object\> | Per-plugin execution logs grouped by stage (omitted when empty) |
| `info` | Array\<String\> | Gateway-internal informational events (omitted when empty) |
| `errors` | Array\<String\> | Error reasons captured during processing (omitted when empty) |

### Request fields

Flattened into the top-level object. Every field is omitted when not present.

| Field | Type | Description |
|-------|------|-------------|
| `client-addr` | String | Client TCP connection address |
| `client-port` | Integer | Client TCP port |
| `remote-addr` | String | Real client IP after proxy resolution (e.g. from `X-Forwarded-For`) |
| `x-trace-id` | String | Per-request trace ID; propagate the same value from clients to correlate |
| `host` | String | Request `Host` header |
| `path` | String | Normalized request path |
| `status` | Integer | Response status code |
| `x-forwarded-for` | String | `X-Forwarded-For` header value (when present) |
| `discover_protocol` | String | Auto-detected protocol. Example values: `grpc`, `websocket` |
| `grpc_service` | String | gRPC service name (gRPC requests only) |
| `grpc_method` | String | gRPC method name (gRPC requests only) |
| `sni` | String | TLS Server Name Indication (TLS connections only) |
| `tls_id` | String | TLS connection/session identifier (TLS connections only) |
| `request_uri` | String | Raw client-entry URI; emitted **only** when `access_log_options.include_raw_uri: true` |

### `match_info` fields

| Field | Type | Description |
|-------|------|-------------|
| `rns` | String | Route namespace |
| `rn` | String | Route name |
| `rule_id` | Integer | Matched rule index (starts at 0) |
| `match_id` | Integer | Matched match-item index (starts at 0) |
| `sv` | Integer | Controller→Gateway sync version; omitted when 0 |

When no route matches, the entire `match_info` object is omitted.

### `backend_context` fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | String | Backend service name |
| `namespace` | String | Backend service namespace |
| `upstreams` | Array\<Object\> | One entry per upstream attempt (retries produce multiple entries) |

#### `upstreams[]` sub-fields

| Field | Type | Unit | Description |
|-------|------|------|-------------|
| `ip` | String | — | Upstream IP address |
| `port` | Integer | — | Upstream port |
| `status` | Integer | — | Upstream response status code |
| `ct` | Integer | ms | Connect time |
| `ht` | Integer | ms | Header time (time to first response byte) |
| `bt` | Integer | ms | Body first-byte time |
| `et` | Integer | ms | Total elapsed time for this attempt |
| `err` | Array\<String\> | — | Error messages for this attempt (omitted when empty) |

When no backend was used (route not matched, or the request was short-circuited by a plugin), `backend_context` is `null` or omitted.

### `plugin_logs` fields

`plugin_logs` is an array of stage groups; each group holds the per-plugin logs for one execution stage.

```json
"plugin_logs": [
  {
    "stage": "request_filters",
    "logs": [
      { "name": "BasicAuth", "time_cost": 5, "log": ["Auth success"] },
      { "name": "Cors", "time_cost": 2, "log": ["CORS resp set"] }
    ]
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `stage` | String | Execution stage. Examples: `request_filters`, `upstream_response_filters` |
| `logs[].name` | String | Plugin name |
| `logs[].time_cost` | Integer | Plugin execution time in **microseconds** |
| `logs[].log` | Array\<String\> | Log lines emitted by the plugin |
| `logs[].log_full` | Boolean | Present and `true` only when the plugin's log buffer overflowed and entries were dropped |

Common plugin log lines:

| Plugin | Log line | Meaning |
|--------|----------|---------|
| BasicAuth | `Auth success` / `Auth failed` / `Anonymous` | Authentication outcome |
| Cors | `CORS resp set` / `Preflight handled` / `Origin rejected` | CORS handling outcome |
| CSRF | `Token verified` / `Token set` / `Token mismatch` / `No token in header` | CSRF handling outcome |
| IPRestriction | `Allowed` / `Denied` | IP filtering outcome |
| Mock | `Mock returned` | Mock response was returned |

`log_full: true` means the plugin emitted more log data than the per-plugin buffer holds, so later lines were dropped. Treat it as a hint to trim the plugin's log output rather than as an error.

### `errors` values

| Value | Description |
|-------|-------------|
| `RouteNotFound` | No matching route for the request |
| `XffHeaderTooLong` | `X-Forwarded-For` header exceeded the allowed length |
| `BackendNotFound` | Backend service could not be resolved |
| `UpstreamConnectError` | Connection to the upstream failed |

## Special fields

### Raw request URI

By default the log records the normalized `path`. To also capture the exact URI as the client sent it (before normalization / rewrites), enable `include_raw_uri` under the top-level `access_log_options` section:

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log

access_log_options:
  include_raw_uri: true
```

The raw URI then appears as the `request_uri` field. This is useful when debugging path rewrites or URL-encoding issues, but it increases line size, so enable it only while investigating.

## Common configuration scenarios

### 1. Quick local debugging

Enable the log to a local file with defaults — the simplest way to see what the gateway is doing during development:

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
```

### 2. Production with daily rotation

Keep one file per day and retain a week of history:

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      queue_size: 80000
      rotation:
        strategy: daily
        max_files: 7
```

### 3. Investigating path rewrites

Capture both the normalized path and the raw client URI while debugging a rewrite problem:

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log

access_log_options:
  include_raw_uri: true
```

## Log examples

### Basic HTTP request

```json
{
  "ts": 1767089146376,
  "client-addr": "192.168.1.20",
  "client-port": 54882,
  "remote-addr": "192.168.1.20",
  "x-trace-id": "eff524a7-10a4-483c-a534-3c50b1199aa0",
  "host": "test.example.com",
  "path": "/health",
  "status": 200,
  "match_info": { "rns": "production", "rn": "api-route", "rule_id": 0, "match_id": 0 },
  "backend_context": {
    "name": "api-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.5", "port": 8080, "status": 200, "ct": 0, "ht": 0, "bt": 0, "et": 1 }
    ]
  }
}
```

### Request with a retry

```json
{
  "ts": 1767089146377,
  "host": "api.example.com",
  "path": "/api/data",
  "status": 200,
  "backend_context": {
    "name": "api-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.5", "port": 8080, "status": 502, "ct": 1, "ht": 100, "et": 100, "err": ["Connection reset"] },
      { "ip": "10.0.0.6", "port": 8080, "status": 200, "ct": 2, "ht": 50, "bt": 50, "et": 52 }
    ]
  }
}
```

### Route not matched

```json
{
  "ts": 1767089149015,
  "host": "wrong-hostname.example.com",
  "path": "/test",
  "status": 404,
  "errors": ["RouteNotFound"],
  "backend_context": null
}
```

### gRPC request

```json
{
  "ts": 1767089148470,
  "host": "grpc.example.com",
  "path": "/test.TestService/SayHello",
  "status": 200,
  "discover_protocol": "grpc",
  "grpc_service": "test.TestService",
  "grpc_method": "SayHello",
  "backend_context": {
    "name": "grpc-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.21", "port": 8080, "status": 200, "ct": 0, "ht": 1, "bt": 1, "et": 1 }
    ]
  }
}
```

## Analyzing logs with jq

```bash
# All 4xx responses
cat logs/edgion_access.log | jq 'select(.status >= 400 and .status < 500)'

# Slowest requests by upstream header time
cat logs/edgion_access.log | jq -r '[.path, .backend_context.upstreams[0].ht] | @tsv' | sort -k2 -n

# Plugin executions over 100 microseconds
cat logs/edgion_access.log | jq '.plugin_logs[]?.logs[]? | select(.time_cost > 100)'

# Trace one request end to end
cat logs/edgion_access.log | jq 'select(.["x-trace-id"] == "eff524a7-10a4-483c-a534-3c50b1199aa0")'
```

Key fields to watch:

- `ht` (header time): backend time-to-first-byte; high values point to backend latency.
- `ct` (connect time): high values point to network or connection-pool issues.
- `et` (elapsed): total per-attempt time, including retries.
- `plugin_logs[].time_cost`: plugin overhead in microseconds.
- `log_full: true`: a plugin's log buffer overflowed.

## FAQ

### Q1: I enabled the access log but the file is empty. Why?

**A**: Confirm `enabled: true` is set and `output.localFile.path` resolves to a writable location. Relative paths are resolved against `work_dir`, so check the actual working directory of the gateway process. Also confirm requests are actually reaching the gateway.

### Q2: Why are some fields missing from a log line?

**A**: Optional fields are omitted when empty rather than written as `null`. For example, `match_info` is absent when no route matched, `grpc_service` only appears for gRPC, and `sni` / `tls_id` only appear for TLS connections. This is expected and keeps lines compact.

### Q3: How do I correlate logs across a multi-hop request?

**A**: Send an `X-Trace-ID` header from the client. The gateway records it as `x-trace-id`, so you can filter every entry for one logical request with a single `jq` selector.

### Q4: A request was rejected by a plugin — where do I look?

**A**: Check `plugin_logs`. The rejecting plugin's `log` array typically carries the reason (for example `Auth failed` for BasicAuth or `Origin rejected` for Cors). When a plugin short-circuits the request, `backend_context` is usually `null`.

### Q5: How do I get more detailed plugin diagnostics for one route?

**A**: Use the debug access-log plugin to raise per-plugin verbosity. See [debug-access-log](../../user-guide/http-route/filters/edgion-plugins/debug-access-log.md).

## Get help

If you run into problems:

1. Check the FAQ section of this document
2. Check the gateway logs
3. Open an issue on GitHub

## Related documents

- [Metrics](./metrics.md)
- [debug-access-log Plugin](../../user-guide/http-route/filters/edgion-plugins/debug-access-log.md)
- [edgion-cli](../edgion-cli.md)
