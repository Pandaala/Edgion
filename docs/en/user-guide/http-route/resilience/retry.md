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
- **Per-try timeout**: when a retry policy is in effect, an attempt that exceeds
  `timeouts.backendRequest` is canceled and retried (GEP-1731 per-try timeout
  semantics). Without a retry policy the timeout is final and the client gets 504
  (see [below](#per-try-timeout-via-backendrequest)).
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

The `timeouts.request` value bounds the **entire** request including all retries and their backoff waits. If the cumulative time would exceed `request`, the request is terminated regardless of remaining attempts. Set `request` high enough to allow your configured attempts to complete. See [Timeout configuration](./timeouts.md) and [Per-try timeout via `backendRequest`](#per-try-timeout-via-backendrequest) below for how `backendRequest` interacts with retries.

### Per-try timeout via `backendRequest`

Per Gateway API [GEP-1731](https://gateway-api.sigs.k8s.io/geps/gep-1731/),
`timeouts.backendRequest` acts as the **per-try timeout**: each attempt
(including the first) gets a fresh `backendRequest` window, while
`timeouts.request` caps the sum of all attempts. An attempt that reaches
`backendRequest` without a response is canceled and retried, subject to the
`attempts` budget and the remaining `request` budget.

> **Behavior change** (as of this release): routes that configure a retry policy retry on backend-request timeouts — including routes that rely on the global default backend timeout. Previously a backendRequest timeout always surfaced as 504 with no retry.

```yaml
rules:
  - retry:
      attempts: 2
    timeouts:
      request: "10s"        # outer budget for everything
      backendRequest: "1s"  # each attempt gets at most 1s
    backendRefs:
      - name: api-service
        port: 8080
```

Notes:

- Timeouts caused by the `request` deadline or a client `grpc-timeout` are
  **never** retried — the caller is out of budget.
- When `backendRequest` is not set on the rule, the global default backend
  timeout (`EdgionGatewayConfig.defaultRequestTimeout`, default `60s`) applies
  and is also treated as the per-try bound — so **any** rule with a retry
  policy gets timeout-retry behavior. Always pair a retry policy with
  `timeouts.request`: without it there is no end-to-end cap and the worst case
  is `(1 + attempts) × backendRequest` (4 × 60s with all defaults).
- There is no "retry on status codes but not on timeouts" mode: configuring
  any retry policy (even `codes` only) opts the rule into timeout-retry, per
  GEP-1731, which classes per-try timeouts with connection errors.
- A per-try timeout is only retried while nothing has been sent to the client
  yet. Once response headers have been forwarded downstream, a mid-body
  timeout terminates the request instead of retrying.
- The retry re-runs normal load balancing with no memory of the timed-out
  backend. RoundRobin usually rotates to another endpoint, but ConsistentHash
  deterministically re-picks the **same** backend (same hash key); pair tight
  per-try timeouts on ConsistentHash routes with the circuit-breaker
  annotation so a persistently slow endpoint gets ejected.
- When the remaining `request`/`grpc-timeout` budget is smaller than the
  computed `backoff` delay, the wait is skipped and the retry proceeds
  immediately (deadline checks still apply) — under a tight deadline the
  configured minimum backoff may not be honored.
- Each canceled-and-retried attempt records `BackendRequestTimeoutRetry`
  (504_03) in the access-log error chain, and its per-upstream entry is
  stamped with status 504 and the timeout error type.
- Every retry decision made by Edgion is labeled in the attempt's
  `upstreams[].err` access-log field: connect failures carry the error type
  (`ConnectTimedout` etc.), per-try timeouts carry the timeout type,
  status-code retries carry `RetryOnStatus`, and stale-keepalive retries
  carry `ConnectionReuseRetry` (retries decided inside Pingora itself, e.g. h2
  GOAWAY re-attempts, may appear as an extra unlabeled entry).
  Retry rate by reason is therefore computable from access-log aggregation
  alone; there is deliberately no per-retry Prometheus counter (see the
  metrics design rules).
- If the client disconnects before the response can be delivered, the
  request-level `errors` array records `ClientClosed` (status 499 in the
  common case; delivery-failure records keep their original error status —
  filter on `errors`, not status). In-flight
  attempts are aborted within milliseconds on HTTP/1.1; a `ClientClosed`
  record with multiple `upstreams` entries quantifies retry work spent on a
  caller that had already left.
- Precision: Edgion enforces `backendRequest` as a per-I/O-operation
  read/write timeout, so an attempt that keeps making progress is not killed
  mid-stream (closest to Envoy's `per_try_timeout` for the response-header
  phase, and to `per_try_idle_timeout` during body transfer). In particular,
  on gRPC routes with long-lived streams, a tight `backendRequest` combined
  with a retry policy will cut idle streams — size it for the longest expected
  quiet period.

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
