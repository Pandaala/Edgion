# Timeout Configuration

Timeouts bound how long the gateway waits for a request to complete, protecting against stuck backends and slow upstreams. Edgion implements the Gateway API v1.4 `rules[].timeouts` field.

## Configuration

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api
    timeouts:
      # End-to-end timeout, including all retries
      request: "30s"
      # Per-attempt backend timeout
      backendRequest: "10s"
    backendRefs:
      - name: api-service
        port: 8080
```

## Configuration reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `request` | Duration | none (no end-to-end limit) | Total time from receiving the request to returning the response, **including all retries** |
| `backendRequest` | Duration | EdgionGatewayConfig `defaultRequestTimeout` (default `60s`) | Timeout for a single attempt to a backend (one try, excluding retries) |

Both accept Gateway API duration strings such as `"500ms"`, `"10s"`, `"1m"`, `"1h"`.

## Timeout types

### request

The end-to-end budget for the whole transaction. It covers every retry attempt and all backoff waits. When the budget is exhausted the request is terminated even if retry attempts remain.

### backendRequest

The budget for a single attempt to a backend: from when the gateway starts sending the request until the full response is received. When the rule also has a retry policy, `backendRequest` acts as the per-try timeout: each attempt gets its own `backendRequest` budget, an attempt exceeding it is canceled and retried, and the total stays bounded by `request` (see [Retry — per-try timeout](./retry.md#per-try-timeout-via-backendrequest)).

Precision: Edgion enforces both timeouts as per-I/O-operation read/write
timeouts, so an attempt that keeps making progress is not killed mid-stream;
see the [precision note](./retry.md#per-try-timeout-via-backendrequest) in the
retry documentation.

## Special values and behavior

| Value | Meaning |
|-------|---------|
| `"0s"` | Disables the timeout (no limit / infinite) — useful for long-lived streaming |
| Omitted | `backendRequest` falls back to the global default from `EdgionGatewayConfig` (`defaultRequestTimeout`, default `60s`); `request` has **no** global default — omitted means no end-to-end limit |
| Positive duration | Explicit limit |

### backendRequest is clamped to request

`backendRequest` must not exceed `request`. If a configured `backendRequest` is larger than `request`, it is automatically clamped down to `request`, so a single attempt can never outlast the end-to-end budget.

### Interaction with retry

Because `request` bounds the entire transaction including retries, set it large enough to allow your retry `attempts` (plus their backoff waits) to complete. `backendRequest` bounds each individual attempt. See [Retry policy — per-try timeout](./retry.md#per-try-timeout-via-backendrequest).

## Examples

### Example 1: Standard API timeouts

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: timeout-api
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      timeouts:
        request: "60s"
        backendRequest: "15s"
      backendRefs:
        - name: api-service
          port: 8080
```

### Example 2: Long-lived streaming (disable the request timeout)

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /stream
    timeouts:
      # No end-to-end limit for streaming connections
      request: "0s"
      backendRequest: "30s"
    backendRefs:
      - name: streaming-service
        port: 8080
```

### Example 3: Timeouts combined with retry

```yaml
rules:
  - timeouts:
      request: "30s"
      backendRequest: "8s"
    retry:
      attempts: 3
      backoff: "200ms"
      codes:
        - 503
    backendRefs:
      - name: api-service
        port: 8080
```

Each attempt is bounded by `backendRequest` (8s); the sum of attempts plus backoff is bounded by `request` (30s).

## Related documents

- [Retry policy](./retry.md)
- [Session persistence](./session-persistence.md)
