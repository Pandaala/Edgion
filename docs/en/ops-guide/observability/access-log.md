# Access Log User Guide

> **🔌 Edgion Extension**
> 
> The access log format and plugin log structure are features specific to Edgion Gateway.

## Overview

Edgion Gateway generates detailed access logs for each request, recording the complete request lifecycle including client information, route matching, backend responses, plugin execution logs, and more.

Logs are output in JSON format by default for easy parsing by log analysis tools.

## Log Location

- **Default path**: `logs/edgion_access.log`
- **Format**: JSON (one log entry per line)

## Log Structure

### Basic Fields

Each access log entry contains the following main fields:

```json
{
  "ts": 1767089146376,
  "request_info": { ... },
  "match_info": { ... },
  "backend_context": { ... },
  "plugin_logs": [ ... ],
  "errors": [ ... ]
}
```

### 1. Timestamp (`ts`)

Request timestamp in millisecond Unix time.

**Example**:
```json
"ts": 1767089146376
```

### 2. Request Info (`request_info`)

Records client and request basic information:

| Field | Description | Example |
|-------|-------------|---------|
| `client-addr` | Client TCP connection address | `"127.0.0.1"` |
| `client-port` | Client port | `54882` |
| `remote-addr` | Actual client IP (after proxy) | `"203.0.113.1"` |
| `x-trace-id` | Request trace ID | `"eff524a7-10a4-483c-..."` |
| `host` | Request Host header | `"test.example.com"` |
| `path` | Request path | `"/health"` |
| `status` | Response status code | `200` |
| `x-forwarded-for` | X-Forwarded-For header (if present) | `"203.0.113.1, 198.51.100.2"` |
| `discover_protocol` | Auto-detected protocol type | `"grpc"`, `"websocket"` |
| `grpc_service` | gRPC service name (for gRPC requests) | `"test.TestService"` |
| `grpc_method` | gRPC method name (for gRPC requests) | `"SayHello"` |

**Example**:
```json
"request_info": {
  "client-addr": "127.0.0.1",
  "client-port": 54882,
  "remote-addr": "127.0.0.1",
  "x-trace-id": "eff524a7-10a4-483c-a534-3c50b1199aa0",
  "host": "test.example.com",
  "path": "/health",
  "status": 200
}
```

### 3. Match Info (`match_info`)

Records the matched route rule:

| Field | Description | Example |
|-------|-------------|---------|
| `rns` | Route namespace | `"edge"` |
| `rn` | Route name | `"test-http"` |
| `rule_id` | Rule ID (starting from 0) | `0` |
| `match_id` | Match item ID (starting from 0) | `0` |

**Example**:
```json
"match_info": {
  "rns": "edge",
  "rn": "test-http",
  "rule_id": 0,
  "match_id": 0
}
```

If no route is matched, the `match_info` field will not appear.

### 4. Backend Context (`backend_context`)

Records backend service and upstream connection information:

| Field | Description |
|-------|-------------|
| `name` | Backend service name |
| `namespace` | Backend service namespace |
| `upstreams` | Upstream connection attempt list (supports retries) |

**Upstream connection info** (`upstreams`):

| Field | Description | Unit |
|-------|-------------|------|
| `ip` | Upstream IP address | - |
| `port` | Upstream port | - |
| `status` | Upstream response status code | - |
| `ct` | Connect Time | milliseconds |
| `ht` | Header Time (time to first byte) | milliseconds |
| `bt` | Body Time (time to first body byte) | milliseconds |
| `err` | Error message (if any) | - |

**Example**:
```json
"backend_context": {
  "name": "test-http",
  "namespace": "edge",
  "upstreams": [
    {
      "ip": "127.0.0.1",
      "port": 30001,
      "status": 200,
      "ct": 0,
      "ht": 0,
      "bt": 0
    }
  ]
}
```

**Multi-retry example**:
```json
"backend_context": {
  "name": "test-service",
  "namespace": "edge",
  "upstreams": [
    {
      "ip": "10.0.1.5",
      "port": 8080,
      "status": 502,
      "ct": 1,
      "ht": 100,
      "err": ["Connection reset"]
    },
    {
      "ip": "10.0.1.6",
      "port": 8080,
      "status": 200,
      "ct": 2,
      "ht": 50,
      "bt": 50
    }
  ]
}
```

If no route is matched or the request is terminated by a plugin, `backend_context` will be `null`.

### 5. Plugin Logs (`plugin_logs`)

Records detailed plugin execution logs, grouped by execution stage:

```json
"plugin_logs": [
  {
    "stage": "request_filters",
    "logs": [
      {
        "name": "BasicAuth",
        "time_cost": 5,
        "log": ["Auth success; "]
      },
      {
        "name": "Cors",
        "time_cost": 2,
        "log": ["CORS resp set; "]
      }
    ]
  },
  {
    "stage": "upstream_response_filters",
    "logs": [
      {
        "name": "ResponseHeaderModifier",
        "time_cost": 1,
        "log": ["Header modified; "]
      }
    ]
  }
]
```

**Plugin log field descriptions**:

| Field | Description | Example |
|-------|-------------|---------|
| `stage` | Execution stage | `"request_filters"`, `"upstream_response_filters"` |
| `name` | Plugin name | `"BasicAuth"`, `"Cors"` |
| `time_cost` | Execution time (microseconds) | `5` |
| `log` | Log content array | `["Auth success; "]` |
| `log_full` | Whether the log buffer is full | `true` (only appears when full) |

**Common plugin log content**:

| Plugin | Log Content |
|--------|------------|
| **BasicAuth** | `Auth success` - Authentication succeeded<br>`Auth failed` - Authentication failed<br>`Anonymous` - Anonymous access |
| **Cors** | `CORS resp set` - CORS headers set<br>`Preflight handled` - Preflight handled<br>`Origin rejected` - Origin rejected |
| **CSRF** | `Token verified` - Token verification succeeded<br>`Token set` - Token set<br>`Token mismatch` - Token mismatch<br>`No token in header` - Missing token |
| **IPRestriction** | `Allowed` - IP allowed<br>`Denied` - IP denied |
| **Mock** | `Mock returned` - Mock response returned |

**`log_full` explanation**:

Plugin logs use a fixed-size buffer (100 bytes). When a plugin's log output exceeds this limit, it is truncated and `log_full: true` is set:

```json
{
  "name": "CustomPlugin",
  "time_cost": 10,
  "log": ["Entry 1; ", "Entry 2; ", "..."],
  "log_full": true
}
```

This indicates the plugin's logs were truncated, and you may need to check the plugin logic or optimize log output.

### 6. Errors (`errors`)

Records errors during request processing:

```json
"errors": ["RouteNotFound"]
```

**Common error codes**:

| Error Code | Description |
|-----------|-------------|
| `RouteNotFound` | No matching route found |
| `XffHeaderTooLong` | X-Forwarded-For header too long |
| `BackendNotFound` | Backend service not found |
| `UpstreamConnectError` | Upstream connection failed |

## Log Examples

### Basic HTTP Request

```json
{
  "ts": 1767089146376,
  "request_info": {
    "client-addr": "127.0.0.1",
    "client-port": 54882,
    "remote-addr": "127.0.0.1",
    "x-trace-id": "eff524a7-10a4-483c-a534-3c50b1199aa0",
    "host": "test.example.com",
    "path": "/health",
    "status": 200
  },
  "match_info": {
    "rns": "edge",
    "rn": "test-http",
    "rule_id": 0,
    "match_id": 0
  },
  "backend_context": {
    "name": "test-http",
    "namespace": "edge",
    "upstreams": [
      {
        "ip": "127.0.0.1",
        "port": 30001,
        "status": 200,
        "ct": 0,
        "ht": 0,
        "bt": 0
      }
    ]
  }
}
```

### Request with Plugins

```json
{
  "ts": 1767089146377,
  "request_info": {
    "client-addr": "127.0.0.1",
    "client-port": 54883,
    "remote-addr": "127.0.0.1",
    "x-trace-id": "d82dcc21-2412-4582-8726-3e7a7becc58b",
    "host": "api.example.com",
    "path": "/api/data",
    "status": 200
  },
  "match_info": {
    "rns": "edge",
    "rn": "api-route",
    "rule_id": 0,
    "match_id": 0
  },
  "backend_context": {
    "name": "api-backend",
    "namespace": "edge",
    "upstreams": [
      {
        "ip": "10.0.1.5",
        "port": 8080,
        "status": 200,
        "ct": 2,
        "ht": 15,
        "bt": 15
      }
    ]
  },
  "plugin_logs": [
    {
      "stage": "request_filters",
      "logs": [
        {
          "name": "BasicAuth",
          "time_cost": 8,
          "log": ["Auth success; "]
        },
        {
          "name": "Cors",
          "time_cost": 2,
          "log": ["CORS resp set; "]
        }
      ]
    }
  ]
}
```

### Route Not Matched

```json
{
  "ts": 1767089149015,
  "request_info": {
    "client-addr": "127.0.0.1",
    "client-port": 54892,
    "remote-addr": "127.0.0.1",
    "x-trace-id": "c8f3e4ec-e1b6-47f0-8941-be21bb86ee0f",
    "host": "wrong-hostname.example.com",
    "path": "/test",
    "status": 404
  },
  "errors": ["RouteNotFound"],
  "backend_context": null
}
```

### gRPC Request

```json
{
  "ts": 1767089148470,
  "request_info": {
    "client-addr": "127.0.0.1",
    "client-port": 54885,
    "remote-addr": "127.0.0.1",
    "x-trace-id": "358c2727-250a-405a-92b2-7b5017cf95d0",
    "host": "grpc.example.com",
    "path": "/test.TestService/SayHello",
    "status": 200,
    "discover_protocol": "grpc",
    "grpc_service": "test.TestService",
    "grpc_method": "SayHello"
  },
  "backend_context": {
    "name": "test-grpc",
    "namespace": "edge",
    "upstreams": [
      {
        "ip": "127.0.0.1",
        "port": 30021,
        "status": 200,
        "ct": 0,
        "ht": 1,
        "bt": 1
      }
    ]
  }
}
```

## Log Analysis Tips

### 1. Analyze Logs with jq

```bash
# View all 4xx errors
cat logs/edgion_access.log | jq 'select(.request_info.status >= 400 and .request_info.status < 500)'

# Find slowest requests (by ht)
cat logs/edgion_access.log | jq -r '[.request_info.path, .backend_context.upstreams[0].ht] | @tsv' | sort -k2 -n

# View plugin execution times
cat logs/edgion_access.log | jq '.plugin_logs[] | .logs[] | select(.time_cost > 100)'

# View all authentication failures
cat logs/edgion_access.log | jq 'select(.plugin_logs[]?.logs[]? | select(.name == "BasicAuth" and (.log[] | contains("Auth failed"))))'
```

### 2. Key Metrics to Watch

- **`ht` (Header Time)**: Backend time-to-first-byte; high values may indicate backend performance issues
- **`ct` (Connect Time)**: Connection establishment time; high values may indicate network issues
- **`plugin_logs.time_cost`**: Plugin execution time; high values may indicate plugin performance issues
- **`log_full: true`**: Plugin logs truncated, may need plugin optimization

### 3. Error Investigation

1. **Route not matched** (`RouteNotFound`): Check if `host` and `path` match the route configuration
2. **Authentication failure**: Check `plugin_logs` for `BasicAuth` logs
3. **CORS error**: Check `plugin_logs` for `Cors` logs, look for `Origin rejected`
4. **Backend error**: Check `backend_context.upstreams[].status` and `err` fields

## Best Practices

1. **Set X-Trace-ID**: Include an `X-Trace-ID` header in client requests for request tracing
2. **Rotate logs regularly**: Use `logrotate` or similar tools to regularly rotate `edgion_access.log`
3. **Integrate with log systems**: Integrate `edgion_access.log` with ELK, Loki, or other log analysis platforms
4. **Monitor key metrics**: Set up alerts based on `edgion_access.log` for high latency, high error rates, etc.

## Related Features

- [Gateway Overview](../gateway/overview.md)
- [Preflight Policy](../gateway/preflight-policy.md)
