# AllEndpointStatus Plugin

> **🔌 Edgion extension**
>  
> AllEndpointStatus is a backend endpoint fan-out probe plugin provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## What is AllEndpointStatus?

AllEndpointStatus concurrently queries all backend endpoints for the current route and aggregates each endpoint's status code, latency, and response body into a single JSON response. Rather than forwarding the request to one backend, it **fans out** to all backends simultaneously. This makes it ideal for validating all Pod health states at once during rolling deployments, or quickly comparing responses across replicas without logging into servers.

## Quick Start

### Simplest Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: health-fan-out
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "ops.example.com"
  rules:
    - matches:
        - path:
            type: Exact
            value: /check-all
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: all-ep-status
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: all-ep-status
  namespace: default
spec:
  requestPlugins:
    - type: AllEndpointStatus
      config: {}
```

### Default Behavior

- Default per-endpoint timeout is 2 seconds; overall wall-clock timeout is 10 seconds
- Queries up to 20 endpoints with a concurrency of 10 by default
- Reads up to 16 KB of response body per endpoint by default
- All values exceeding hard limits are **silently clamped** — no error is returned

---

## Configuration Parameters

### Basic Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `timeoutMs` | Integer | `2000` | Per-endpoint request timeout in milliseconds. Hard limit: 5000 ms |
| `wallTimeoutMs` | Integer | `10000` | Wall-clock timeout for the entire fan-out operation in milliseconds. Hard limit: 10000 ms. Endpoints cancelled by this deadline are reported as `"wall timeout exceeded"` |
| `maxEndpoints` | Integer | `20` | Maximum number of endpoints to query. Hard limit: 50 |
| `maxBodySize` | Integer | `16384` | Maximum response body bytes to read per endpoint. Hard limit: 16384 (16 KB). Excess bytes are discarded via streaming |
| `concurrencyLimit` | Integer | `10` | Maximum number of concurrent requests |

### Override Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `methodOverride` | String | None | Override the HTTP method sent to endpoints, e.g. `"GET"` |
| `pathOverride` | String | None | Override the request path sent to endpoints, e.g. `"/health"` |

### Debug Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `includeResponseHeaders` | Boolean | `false` | Whether to include each endpoint's response headers in the result |

### Security Note

All parameters have hard limits. The effective value is the three-layer minimum: `min(user config, global TOML limit, hard limit)`. Values exceeding the limit are silently clamped without returning an error.

---

## Response Format

The plugin returns `200 OK` with `Content-Type: application/json`. The body is a JSON object with a `summary` block and a `backends` array; each backend carries an `endpoints` array (one element per endpoint). Field names use snake_case.

```json
{
  "summary": {
    "total_backends": 1,
    "total_endpoints": 2,
    "success_count": 1,
    "failure_count": 1,
    "truncated": false,
    "timeout_ms": 2000,
    "wall_timeout_ms": 10000,
    "total_latency_ms": 42,
    "total_response_bytes": 17,
    "wall_timeout_hit": false
  },
  "backends": [
    {
      "name": "api-service",
      "namespace": "production",
      "port": 8080,
      "endpoint_count": 2,
      "endpoints": [
        {
          "address": "10.0.0.1:8080",
          "status": 200,
          "latency_ms": 12,
          "body": "{\"status\":\"ok\"}",
          "body_truncated": false,
          "error": null
        },
        {
          "address": "10.0.0.2:8080",
          "status": null,
          "latency_ms": 0,
          "body": null,
          "body_truncated": false,
          "error": "wall timeout exceeded"
        }
      ]
    }
  ]
}
```

### `summary` fields

| Field | Type | Description |
|-------|------|-------------|
| `total_backends` | Integer | Number of backends probed |
| `total_endpoints` | Integer | Number of endpoints probed across all backends |
| `success_count` | Integer | Endpoints that returned a 2xx/3xx status |
| `failure_count` | Integer | Endpoints that returned a non-2xx status or failed |
| `truncated` | Boolean | `true` if the endpoint count exceeded `maxEndpoints` and was capped |
| `timeout_ms` | Integer | Effective per-endpoint timeout used |
| `wall_timeout_ms` | Integer | Effective wall-clock timeout used |
| `total_latency_ms` | Integer | Wall-clock time for the whole fan-out, in milliseconds |
| `total_response_bytes` | Integer | Total response body bytes fetched across all endpoints |
| `wall_timeout_hit` | Boolean | `true` if the wall timeout cancelled some endpoints |

### `backends[]` fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | String | Backend Service name |
| `namespace` | String | Backend namespace |
| `port` | Integer | Backend port |
| `endpoint_count` | Integer | Number of endpoints for this backend |
| `endpoints` | Array | Per-endpoint results (see below) |

### `endpoints[]` fields

| Field | Type | Description |
|-------|------|-------------|
| `address` | String | Endpoint address (`host:port`) |
| `status` | Integer | HTTP status code; `null` when the probe failed before a response |
| `latency_ms` | Integer | Round-trip latency in milliseconds |
| `body` | String | Response body (truncated to `maxBodySize`); `null` when the probe failed or no body was read |
| `body_truncated` | Boolean | `true` if the body was cut off at the `maxBodySize` limit |
| `headers` | Object | Response headers. Present only when `includeResponseHeaders: true` |
| `error` | String | Error description; `null` on success |

When an endpoint fails — connection failure or the `wallTimeoutMs` deadline — `status` is `null` and `error` carries the reason (a connection error, or `"wall timeout exceeded"`).

---

## Common Configuration Scenarios

### 1. Minimal Health Probe (status codes only)

Useful for rolling deployment validation when response body details are not needed.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: liveness-check
  namespace: default
spec:
  requestPlugins:
    - type: AllEndpointStatus
      config:
        timeoutMs: 1000
        methodOverride: "GET"
        pathOverride: "/healthz"
```

### 2. Custom Health Check Path

Route health probe requests to `/health` and use HEAD to minimize response body transfer.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: health-probe
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "ops.example.com"
  rules:
    - matches:
        - path:
            type: Exact
            value: /pod-health
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: pod-health-check
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: pod-health-check
  namespace: default
spec:
  requestPlugins:
    - type: AllEndpointStatus
      config:
        methodOverride: "HEAD"
        pathOverride: "/health"
        timeoutMs: 500
        wallTimeoutMs: 3000
        maxEndpoints: 30
```

### 3. Detailed Debug with Response Headers

When you need to compare response headers across replicas (e.g., `X-Pod-Name`, `X-Version`), enable `includeResponseHeaders`.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: debug-pods
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "ops.example.com"
  rules:
    - matches:
        - path:
            type: Exact
            value: /debug/all-pods
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: debug-all-ep
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-all-ep
  namespace: default
spec:
  requestPlugins:
    - type: AllEndpointStatus
      config:
        methodOverride: "GET"
        pathOverride: "/info"
        includeResponseHeaders: true
        maxBodySize: 4096
        concurrencyLimit: 5
```

---

## FAQ

### Q1: Why is the response body truncated?

**A**: `maxBodySize` defaults to 16384 bytes (16 KB) and cannot exceed this hard limit. Excess bytes are discarded during streaming. If you need the full response body, consider reducing the payload in your application, or query the endpoint directly.

### Q2: I configured `timeoutMs: 10000`, but it seems to be capped at 5000. Why?

**A**: The hard limit for `timeoutMs` is 5000 ms. Values above this are silently clamped. This is a security design decision to prevent slow backends from blocking the fan-out operation for extended periods.

### Q3: Some endpoints have `status: null`. What does this mean?

**A**: The probe never received a response. There are two common causes:
1. Connection failure (network unreachable, port not listening)
2. The `wallTimeoutMs` deadline was reached before the endpoint responded (`summary.wall_timeout_hit` is then `true`)

Read the `error` field to distinguish the two — it carries the failure reason (a connection error, or `"wall timeout exceeded"`).

### Q4: Does this plugin affect normal traffic?

**A**: AllEndpointStatus is a **terminating plugin** — all requests matching the route are handled by the plugin and returned directly, without being forwarded to backends. We recommend configuring a dedicated path (e.g., `/check-all`) rather than attaching it to a business path.

---

## Related Documentation

- [DebugAccessLogToHeader Plugin](debug-access-log.md) — Inject debug info into response headers
- [Mock Plugin](mock.md) — Return static responses
- [DirectEndpoint Plugin](direct-endpoint.md) — Forward to a specific endpoint
- [RateLimit Plugin](rate-limit.md) — Rate limiting
