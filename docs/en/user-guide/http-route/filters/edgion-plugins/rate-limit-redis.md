# RateLimitRedis Plugin

> **🔌 Edgion extension**
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
| External dependency | none | Requires Redis |
| On Redis failure | N/A | Configurable: Allow / Deny |
| Use case | High throughput, tolerates approximation | Exact quotas, cluster-wide enforcement |

## Quick Start

### Simplest Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-with-redis-limit
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
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
  namespace: production
spec:
  requestPlugins:
    - type: RateLimitRedis
      config:
        redisRef: "default/shared-redis"
        key:
          - type: remoteIp
        policies:
          - rate: 100
            interval: "1s"
```

### Prerequisites

`redisRef` must point to an existing `LinkSys` Redis connector, in `"namespace/name"` format. Ensure the corresponding LinkSys is created before applying this plugin. For how to define a Redis connector, see [Infrastructure Connectors](../../../../ops-guide/infrastructure/connectors.md).

### Default Behavior

- Rate limits by client IP by default
- Default algorithm is SlidingWindow
- Redis failure defaults to allowing requests (fail-open)
- Returns `X-RateLimit-Limit` and `X-RateLimit-Remaining` response headers by default; `X-RateLimit-Reset` is only emitted on rejection (429) responses (see HeaderNames below)
- Default reject status code is 429

---

## Configuration Parameters

### Basic Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `redisRef` | String | — (required) | Redis LinkSys resource reference in `"namespace/name"` format. Only alphanumeric characters, `-`, and `_` are allowed |
| `policies` | Array\<Object\> | — (required) | Rate limiting policy list. At least 1, maximum 10. See Policy Configuration below |
| `key` | Array\<Object\> | `[{type: remoteIp}]` | Rate limiting dimension list. A single key is used verbatim; multiple keys are length-prefixed and joined with `_`. See Key Configuration below |
| `onKeyMiss` | Enum | `Allow` | Behavior when key cannot be extracted. Values: `Allow` \| `Deny` |
| `defaultKey` | String | none | Default key value when extraction fails, takes precedence over `onKeyMiss` |
| `rejectStatus` | Integer | `429` | HTTP status code on rejection (100–599) |
| `rejectMessage` | String | none | JSON response body on rejection |
| `showLimitHeaders` | Boolean | `true` | Whether to return rate limit response headers |
| `headerNames` | Object | none | Custom response header names. See HeaderNames Configuration below |

### Redis Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `onRedisFailure` | Enum | `Allow` | Policy applied when Redis is unavailable **or** when `requestTimeout` elapses. Covers both (a) Redis detectably down — fast-eject with zero I/O, and (b) Redis healthy but slow/erroring — bounded wait then give up. Both cases route through this single policy. Values: `Allow` (fail-open) \| `Deny` (fail-close) |
| `requestTimeout` | String | unset | Optional wall-clock deadline for the complete multi-policy Redis check (e.g. `"500ms"`). When unset, no aggregate timeout is applied; the per-command timeout from the LinkSys Redis CRD still applies. When set and the deadline elapses, `onRedisFailure` applies |
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
| `interval` | String | — (required) | Time window, e.g. `"1s"`, `"1m"`, `"1h"`, `"24h"`, or combined like `"1h30m"`. Minimum: 100ms |
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
| `remoteIp` | No | Resolved client IP address |
| `directPeerIp` | No | Physical transport peer IP before PROXY, RealIp, or forwarded-header processing |
| `host` | No | Entry-time request authority, normalized by stripping an unambiguous port and trailing dot and converting ASCII letters to lowercase |
| `header` | Yes | Value of a specific request header |
| `cookie` | Yes | Value of a specific cookie |
| `query` | Yes | Value of a specific query parameter |
| `path` | No | Request path |
| `method` | No | HTTP method |
| `ctx` | Yes | Context variable set by another plugin |
| `remoteIpAndPath` | No | Combined resolved client IP and path |
| `requestPath` | No | Entry-time raw request path before plugin rewrites |

Example of combining multiple keys:

```yaml
key:
  - type: directPeerIp
  - type: host
  - type: header
    name: "X-API-Key"
```

Use `directPeerIp` when the policy means the physical peer that connected to Edgion. Use
`remoteIp` only when the deployment intentionally treats trusted PROXY protocol or RealIp/XFF
processing as the client identity boundary. The two values are deliberately distinct.

`host` uses the same frozen, canonical authority snapshot used for route selection. It covers an
absolute-form URI authority, HTTP/1 Host, and HTTP/2 `:authority`; a plugin that later rewrites Host
does not move the request into a different rate-limit bucket.

The resulting Redis key looks like `{keyPrefix}{pluginScope}:{algorithm}:{policyIndex}:{rateLimitKey}`, e.g. `edgion:rl:default/api-limiter/#0:sw:0:11:192.168.1.1_9:apikey123` — with multiple keys configured, each part of the rate limiting key carries its `<byte-length>:` prefix so that two different value tuples can never share a bucket. A single configured key is used verbatim (`edgion:rl:default/api-limiter/#0:sw:0:192.168.1.1`).

`{pluginScope}` identifies the plugin entry that owns the bucket: `<namespace>/<name>/<alias>` for the EdgionPlugins resource this plugin is declared in, or `<namespace>/<name>/#<index>` when the entry has no `alias`. Every Gateway instance derives the same string from the same resource, so cluster-wide counting is unaffected — but two unrelated EdgionPlugins resources can no longer index the same buckets, even when their `keyPrefix` and `key` sources are identical. Without it, a client that controls one plugin's key source could craft a value matching another plugin's composed key, and — more commonly — two default-configured plugins would silently share one set of per-IP buckets. Renaming the resource or its `alias`, or reordering unaliased entries, moves the plugin to a fresh namespace and restarts its counters; set an `alias` to pin the scope across reordering. An alias must be unique within its stage array: a duplicate sets `Accepted=False` on the resource, and the Gateway additionally drops the repeated alias so that entry falls back to its `#<index>` scope — a duplicate can never merge two entries' buckets.

Multiple routes referencing the **same** EdgionPlugins resource share one limiter — that is how a rate limit is shared across routes, and it matches the in-memory `RateLimit`, whose counter is likewise shared per resource. Two *different* resources can no longer share a bucket by construction: to share a quota across routes, point them at one resource rather than duplicating the configuration.

Omitting `key` applies the default `[{type: remoteIp}]`. An explicitly empty `key: []` means "one global bucket": every request composes to the same empty key and is still rate limited — it is not treated as a key extraction failure, so `onKeyMiss` / `defaultKey` do not apply to it.

For a composite key, a missing component is encoded as a `-` slot as long as at least one other
component resolves. `onKeyMiss` applies only when every configured source is missing. If a header
or cookie must be present, enforce that requirement with a deny-capable plugin such as
[RequestRestriction](request-restriction.md) or
[RequestAccessPolicy](request-access-policy.md) before RateLimitRedis. A RateLimitRedis run
condition only selects traffic to count; when it does not match, the limiter is skipped and the
request continues. `onKeyMiss: Deny` alone does not require every component.

Do not use secrets such as complete Authorization credentials as descriptors: generated Redis keys
are operationally inspectable. `maxKeyLen` bounds bytes in each Redis key, not the number of
distinct keys. Prefer stable, bounded-cardinality identifiers and size Redis for the number of
active buckets a client-controlled descriptor can create.

---

### HeaderNames Sub-fields

Customize rate limit response header names. Only explicitly configured fields appear in responses.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | String | none | Rate limit header name (default: `X-RateLimit-Limit`) |
| `remaining` | String | none | Remaining quota header name (default: `X-RateLimit-Remaining`) |
| `reset` | String | none | Window reset time header name (default: `X-RateLimit-Reset`). Only emitted on rejection (429) responses — the Redis Lua scripts do not return a window-reset timestamp for allowed requests |
| `retryIn` | String | none | Human-readable retry time (e.g., `"1.5s"`) |

---

## Common Configuration Scenarios

### 1. Simple Cluster Rate Limiting (Single Policy by IP)

Limit each IP to 100 requests per second cluster-wide, with all gateway instances sharing the same counter.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-cluster-limit
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
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
  namespace: production
spec:
  requestPlugins:
    - type: RateLimitRedis
      config:
        redisRef: "default/shared-redis"
        key:
          - type: remoteIp
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
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
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
  namespace: production
spec:
  requestPlugins:
    - type: RateLimitRedis
      config:
        redisRef: "default/shared-redis"
        key:
          - type: header
            name: "X-API-Key"
        onKeyMiss: Deny
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
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
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
  namespace: production
spec:
  requestPlugins:
    - type: RateLimitRedis
      config:
        redisRef: "default/ha-redis"
        key:
          - type: header
            name: "Authorization"
        onKeyMiss: Deny
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

### 4. Conditional Global Policy with a Composite Bucket

The plugin entry can select requests with shared conditions and then count them with a separate
composite descriptor. This example limits each physical peer + normalized host + tenant header for
only `/v1/expensive` requests. A missing tenant header is counted in the composite `-` bucket; if
the header is mandatory, reject it separately before this limiter.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: expensive-api-limit
  namespace: production
spec:
  requestPlugins:
    - type: RateLimitRedis
      alias: expensive-api
      conditions:
        run:
          allOf:
            - type: keyMatch
              key:
                type: host
              values: ["api.example.com"]
            - type: keyMatch
              key:
                type: path
              regex: ["^/v1/expensive(?:/|$)"]
      config:
        redisRef: "production/shared-redis"
        key:
          - type: directPeerIp
          - type: host
          - type: header
            name: X-Tenant-ID
        onKeyMiss: Deny
        onRedisFailure: Deny
        policies:
          - rate: 30
            interval: 1m
            algorithm: SlidingWindow
---
apiVersion: edgion.io/v1
kind: EdgionGatewayConfig
metadata:
  name: production-gateway
spec:
  globalPluginsRef:
    - name: expensive-api-limit
      namespace: production
```

`globalPluginsRef` runs before route-level plugins for every request handled under that Gateway
configuration; the entry's `conditions.run` narrows which requests consume quota.

Do not use a client-controlled `keyExist` run condition to enforce a required field: a false run
condition skips the limiter and allows processing to continue. Required-field rejection belongs in
a preceding deny-capable policy.

For one quota shared by every selected request, replace the composite list with an explicitly empty
list:

```yaml
key: []
```

The empty descriptor is still namespaced by the owning plugin resource and entry alias, so unrelated
plugin entries do not accidentally share the bucket.

---

## FAQ

### Q1: What is the format of `redisRef`?

**A**: The format is `"namespace/name"`, referring to a `LinkSys` Redis connector. For example: `"default/shared-redis"`. Each segment (namespace and name) may contain only alphanumeric characters, `-`, and `_` — no special characters or extra separators. The reference must resolve to an existing LinkSys; see [Infrastructure Connectors](../../../../ops-guide/infrastructure/connectors.md).

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
- [Infrastructure Connectors](../../../../ops-guide/infrastructure/connectors.md) — Defining the Redis `LinkSys` referenced by `redisRef`
- [IpRestriction Plugin](ip-restriction.md) — IP-based access control
- [KeyAuth Plugin](key-auth.md) — API Key authentication
- [JwtAuth Plugin](jwt-auth.md) — JWT authentication
