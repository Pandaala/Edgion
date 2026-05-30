# Retry Policy

Retries re-send a failed request to a backend to ride out transient failures and improve availability. Edgion implements the Gateway API v1.4 `rules[].retry` field.

## Configuration

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api
    retry:
      # Maximum retry attempts (0 disables retry)
      attempts: 3
      # Minimum wait between retries; doubles each attempt, capped at 30s
      backoff: "1s"
      # HTTP status codes that trigger a retry
      codes:
        - 502
        - 503
        - 504
    backendRefs:
      - name: api-service
        port: 8080
```

## Configuration reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `attempts` | Integer | `EdgionGatewayConfig.spec.maxRetries` (default `3`) | Maximum retry attempts. `0` disables retry. When omitted, the global default applies |
| `backoff` | Duration | none | Minimum wait between retries; used as the exponential backoff base. Example: `"1s"`, `"100ms"`. When omitted, retries are issued with no delay |
| `codes` | Array\<Integer\> | none | HTTP status codes (100–599) that trigger a retry. Out-of-range codes are ignored. When omitted, only connection errors are retried |

## Retry behavior

### What triggers a retry

- **Connection errors** are retried while a retry policy is in effect (connect failure, reset before any response).
- **Status codes** trigger a retry only when listed in `codes`. If `codes` is omitted, status-code retries are off and only connection errors are retried.
- Connection-failure retries and status-code retries share the same `attempts` counter.

### Attempts budget

The effective budget is `rule.retry.attempts` when set, otherwise the global `EdgionGatewayConfig.spec.maxRetries` (default `3`). Setting `attempts: 0` disables retry for the rule.

### Backoff

When `backoff` is set, the wait before retry attempt `n` (counting from the first retry) is `backoff × 2^(n-1)`, capped at 30 seconds:

```yaml
retry:
  attempts: 5
  backoff: "100ms"
  codes:
    - 503
```

Resulting waits: 100ms → 200ms → 400ms → 800ms → 1600ms (each capped at 30s). When `backoff` is omitted, retries are issued back-to-back with no sleep.

### Interaction with timeouts

The `timeouts.request` value bounds the **entire** request including all retries and their backoff waits. If the cumulative time would exceed `request`, the request is terminated regardless of remaining attempts. Set `request` high enough to allow your configured attempts to complete. See [Timeout configuration](./timeouts.md).

## Examples

### Example 1: Retry on connection failure only

Omit `codes` to retry only connection errors (safe for any backend):

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: retry-conn
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - retry:
        attempts: 2
      backendRefs:
        - name: api-service
          port: 8080
```

### Example 2: Retry 5xx with exponential backoff

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: retry-5xx
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - timeouts:
        request: "30s"
      retry:
        attempts: 3
        backoff: "200ms"
        codes:
          - 500
          - 502
          - 503
          - 504
      backendRefs:
        - name: api-service
          port: 8080
```

### Example 3: Disable retry for a rule

```yaml
rules:
  - retry:
      attempts: 0
    backendRefs:
      - name: non-idempotent-service
        port: 8080
```

## Notes

- Enable status-code retries only for idempotent operations; retrying a non-idempotent `POST` can duplicate side effects.
- Always pair retries with a `timeouts.request` so cumulative retry time is bounded.
- A persistently high retry rate usually points at an unhealthy backend rather than transient errors.

## Related documents

- [Timeout configuration](./timeouts.md)
- [Session persistence](./session-persistence.md)
- [Load balancing algorithms](../lb-algorithms.md)
