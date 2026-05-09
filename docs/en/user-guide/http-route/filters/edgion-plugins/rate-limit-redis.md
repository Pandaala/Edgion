# RateLimitRedis Plugin

> **🔌 Edgion Extension**
>  
> RateLimitRedis is a Redis-backed cluster-wide precise rate limiting plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

## What is RateLimitRedis?

The [RateLimit plugin](rate-limit.md) uses an in-process Count-Min Sketch algorithm with nanosecond latency, but each gateway instance counts independently — in multi-replica deployments, the effective total throughput is the configured rate multiplied by the number of instances. RateLimitRedis uses Redis to enforce **precise** cluster-wide rate limits: all instances share the same Redis counter, so the configured rate is the actual rate for the entire cluster.

### RateLimit vs. RateLimitRedis

| Feature | RateLimit | RateLimitRedis |
|---------|-----------|----------------|
| Implementation | In-process Count-Min Sketch | Redis Lua scripts (atomic) |
| Latency | Nanoseconds | 0.1–1 ms (network RTT) |
| Accuracy | Approximate (CMS error) | Exact |
| Scope | Per-instance | Cluster-wide |
| External dependency | None | Requires Redis |
| On Redis failure | N/A | Configurable: Allow / Deny |
| Use case | High throughput, tolerates approximation | Exact quotas, cluster-wide enforcement |

## Quick Start

### Simplest Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-with-redis-limit
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: redis-rate-limit
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: redis-rate-limit
  namespace: default
spec:
  requestPlugins:
    - type: RateLimitRedis
      config:
        redisRef: "default/shared-redis"
        key:
          - type: clientIp
        policies:
          - rate: 100
            interval: "1s"
```

### Prerequisites

`redisRef` must point to an existing `LinkSys` resource of type Redis. Ensure the corresponding LinkSys is created in the same or target namespace before applying this plugin.

### Default Behavior

- Rate limits by client IP by default
- Default algorithm is SlidingWindow
- Redis failure defaults to allowing requests (fail-open)
- Returns `X-RateLimit-*` response headers by default
- Default reject status code is 429

---

## Configuration Parameters

### Basic Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `redisRef` | String | — (required) | Redis LinkSys resource reference in `"namespace/name"` format. Only alphanumeric characters, `-`, and `_` are allowed |
| `policies` | Array\<Object\> | — (required) | Rate limiting policy list. At least 1, maximum 10. See Policy Configuration below |
| `key` | Array\<Object\> | `[{type: clientIp}]` | Rate limiting dimension list. Multiple keys are joined with `_`. See Key Configuration below |
| `onMissingKey` | Enum | `Allow` | Behavior when key cannot be extracted. Values: `Allow` \| `Deny` |
| `defaultKey` | String | None | Default key value when extraction fails, takes precedence over `onMissingKey` |
| `rejectStatus` | Integer | `429` | HTTP status code on rejection (100–599) |
| `rejectMessage` | String | None | JSON response body on rejection |
| `showLimitHeaders` | Boolean | `true` | Whether to return rate limit response headers |
| `headerNames` | Object | None | Custom response header names. See HeaderNames Configuration below |

### Redis Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `onRedisFailure` | Enum | `Allow` | Behavior when Redis is unavailable. Values: `Allow` (fail-open) \| `Deny` (fail-close) |
| `keyPrefix` | String | `"edgion:rl:"` | Redis key prefix. Must end with `:` and contain only printable ASCII characters |
| `maxKeyLen` | Integer | `4096` | Maximum byte length of the key portion in Redis keys (64–65536). Prevents Redis OOM from maliciously long key values |

### Advanced Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `cost` | Integer | `1` | Request cost units for weighted rate limiting. `cost: 5` means each request consumes 5 units of the quota |

---

### Policy Sub-fields

Each entry in the `policies` array contains:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `rate` | Integer | — (required) | Maximum requests (or cost units) allowed per interval. Must be > 0 |
| `interval` | String | — (required) | Time window, e.g. `"1s"`, `"1m"`, `"1h"`, `"1d"`, or combined like `"1h30m"`. Minimum: 100ms |
| `algorithm` | Enum | `SlidingWindow` | Rate limiting algorithm. See algorithm table below |

#### Algorithm Reference

| Value | Description |
|-------|-------------|
| `SlidingWindow` | **Sliding window** (default). Weighted average of current and previous fixed windows. Prevents boundary bursts. Best accuracy/performance balance |
| `FixedWindow` | **Fixed window**. Simplest approach. Counter resets at each window boundary. Susceptible to boundary bursts |
| `TokenBucket` | **Token bucket**. Allows controlled bursts up to bucket capacity (= rate). Suitable for scenarios where short-term bursts are acceptable |

Multiple policies are evaluated **independently** — a request is allowed only when ALL policies pass. If any single policy triggers a rate limit, the request is rejected (the most restrictive policy wins).

---

### Key Configuration

Identical to the [RateLimit plugin](rate-limit.md). Supported key types:

| type | Requires `name` | Description |
|------|-----------------|-------------|
| `clientIp` | No | Client IP address (after real IP extraction) |
| `header` | Yes | Value of a specific request header |
| `cookie` | Yes | Value of a specific cookie |
| `query` | Yes | Value of a specific query parameter |
| `path` | No | Request path |
| `method` | No | HTTP method |
| `ctx` | Yes | Context variable set by another plugin |
| `clientIpAndPath` | No | Combined client IP and path |

Example of combining multiple keys:

```yaml
key:
  - type: clientIp
  - type: header
    name: "X-API-Key"
```

The resulting Redis key looks like: `edgion:rl:sw:192.168.1.1_apikey123`

---

### HeaderNames Sub-fields

Customize rate limit response header names. Only explicitly configured fields appear in responses.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | String | None | Rate limit header name (default: `X-RateLimit-Limit`) |
| `remaining` | String | None | Remaining quota header name (default: `X-RateLimit-Remaining`) |
| `reset` | String | None | Window reset time header name (default: `X-RateLimit-Reset`) |
| `retryIn` | String | None | Human-readable retry time (e.g., `"1.5s"`) |

---

## Common Configuration Scenarios

### 1. Simple Cluster Rate Limiting (Single Policy by IP)

Limit each IP to 100 requests per second cluster-wide, with all gateway instances sharing the same counter.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-cluster-limit
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: cluster-rate-limit
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cluster-rate-limit
  namespace: default
spec:
  requestPlugins:
    - type: RateLimitRedis
      config:
        redisRef: "default/shared-redis"
        key:
          - type: clientIp
        policies:
          - rate: 100
            interval: "1s"
            algorithm: SlidingWindow
```

### 2. Multi-Policy Rate Limiting (Burst + Hourly Quota)

Limit the same API Key to 50 requests per second AND 5000 per hour simultaneously. Both policies are evaluated independently — either one triggering causes rejection.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-multi-policy
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /v1
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: multi-policy-limit
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: multi-policy-limit
  namespace: default
spec:
  requestPlugins:
    - type: RateLimitRedis
      config:
        redisRef: "default/shared-redis"
        key:
          - type: header
            name: "X-API-Key"
        onMissingKey: Deny
        policies:
          - rate: 50
            interval: "1s"
            algorithm: SlidingWindow
          - rate: 5000
            interval: "1h"
            algorithm: FixedWindow
        rejectMessage: '{"error":"rate limit exceeded","code":"RATE_LIMIT"}'
```

### 3. High-Security — Deny on Redis Failure

For paid APIs or security-sensitive endpoints, configure `onRedisFailure: Deny` to ensure no excess requests are allowed when Redis is unavailable.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: secure-api
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "secure-api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /premium
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: secure-redis-limit
      backendRefs:
        - name: premium-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: secure-redis-limit
  namespace: default
spec:
  requestPlugins:
    - type: RateLimitRedis
      config:
        redisRef: "default/ha-redis"
        key:
          - type: header
            name: "Authorization"
        onMissingKey: Deny
        onRedisFailure: Deny
        policies:
          - rate: 1000
            interval: "1m"
            algorithm: TokenBucket
        keyPrefix: "premium:rl:"
        showLimitHeaders: true
        headerNames:
          limit: "X-Premium-Limit"
          remaining: "X-Premium-Remaining"
          retryIn: "X-Retry-After"
```

---

## FAQ

### Q1: What is the format of `redisRef`?

**A**: The format is `"namespace/name"`, referring to a `LinkSys` resource of type Redis. For example: `"default/shared-redis"`. Only alphanumeric characters, `-`, and `_` are allowed — no special characters or extra separators.

### Q2: What is the minimum `interval`?

**A**: The minimum interval is 100 ms. Intervals below 100 ms are rejected during validation, because very small windows cause frequent Redis writes that can overwhelm the Redis cluster. For extremely high-frequency rate limiting, use the [RateLimit plugin](rate-limit.md) (in-process, nanosecond latency).

### Q3: Are multiple policies "OR" or "AND" logic?

**A**: **AND** logic — a request is allowed only when ALL policies pass. Any single policy triggering a rate limit causes the request to be rejected.

### Q4: Why must `keyPrefix` end with `:`?

**A**: This follows Redis key naming conventions and clearly separates namespace segments in Redis key strings. The default `"edgion:rl:"` already conforms. If you omit the trailing `:` in a custom prefix, validation will return an error.

### Q5: Can RateLimitRedis and RateLimit be used on the same route simultaneously?

**A**: Yes. Both plugins work independently; a request must pass both to reach the backend. A common pattern: RateLimit first for coarse-grained local burst protection (fast, nanosecond-level), followed by RateLimitRedis for precise cluster-wide quota management.

---

## Related Documentation

- [RateLimit Plugin](rate-limit.md) — In-process approximate rate limiting (high performance, low latency)
- [IpRestriction Plugin](ip-restriction.md) — IP-based access control
- [KeyAuth Plugin](key-auth.md) — API Key authentication
- [JwtAuth Plugin](jwt-auth.md) — JWT authentication
