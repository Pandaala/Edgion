# RateLimit Plugin

## Overview

The RateLimit plugin provides high-performance request rate limiting based on Pingora's Count-Min Sketch (CMS) algorithm.
It supports multi-dimensional rate limiting keys, custom response headers, and cluster-level distributed rate limiting.

This plugin is suitable for API rate limiting, brute-force attack prevention, and protecting backend services from traffic spikes.

## Features

- **Count-Min Sketch algorithm** - Fixed memory usage regardless of key cardinality
- **Sliding window** - Dual-slot (red/blue) design for smooth sliding window rate limiting
- **Multi-dimensional rate limiting keys** - Supports Client IP, Header, Path, and composite keys
- **Cluster rate limiting** - Cluster mode automatically distributes global quota across Gateway instances
- **Custom response headers** - Supports standard `X-RateLimit-*` and custom header names
- **Fail-open** - Defaults to allowing requests on configuration errors or missing keys

## Configuration

### Basic Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rate-limit-basic
  namespace: default
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 100
        interval: "1s"
        key:
          - type: clientIp
```

### Configuration Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `rate` | integer | No | `100` | Rate limit threshold (max requests per interval) |
| `interval` | string | No | `"1s"` | Time window, supports `ms`, `s`, `m`, `h`, `d` |
| `key` | KeyGet[] | No | `[{type: clientIp}]` | Rate limiting key configuration (multiple keys joined with `_`) |
| `scope` | enum | No | `Instance` | Rate limiting scope: `Instance` or `Cluster` |
| `skewTolerance` | float | No | `1.2` | Traffic skew tolerance (Cluster mode only, range 1.0~2.0) |
| `onMissingKey` | enum | No | `Allow` | Behavior when key cannot be extracted: `Allow` or `Deny` |
| `defaultKey` | string | No | - | Default key when extraction fails |
| `rejectStatus` | integer | No | `429` | HTTP status code on rejection (100~599) |
| `rejectMessage` | string | No | `"Rate limit exceeded"` | Response message on rejection |
| `showLimitHeaders` | boolean | No | `true` | Whether to add rate limiting headers to the response |
| `headerNames` | object | No | - | Custom rate limiting response header names |
| `estimatorSlotsK` | integer | No | - | CMS precision (K units), uses global default if not set |

#### rate

Maximum number of requests allowed per `interval` time window.

- `scope: Instance`: represents the quota for a **single Gateway instance**
- `scope: Cluster`: represents the **total cluster-wide** quota (automatically distributed across instances)

#### interval

Time window length, determines the CMS sliding window rotation period. Supported formats:

| Format | Example | Meaning |
|--------|---------|---------|
| `ms` | `500ms` | Milliseconds |
| `s` | `1s`, `10s` | Seconds |
| `m` | `1m`, `5m` | Minutes |
| `h` | `1h` | Hours |
| `d` | `1d` | Days |

#### key

The rate limiting key determines which dimension to rate limit on. Supported key types:

| Type | Description | Requires name |
|------|-------------|--------------|
| `clientIp` | Rate limit by client IP | No |
| `header` | Rate limit by request header value | Yes |
| `path` | Rate limit by request path | No |
| `clientIpAndPath` | Rate limit by IP + Path combination | No |

**Multi-key composition**: When multiple keys are configured, their values are joined with `_` to form a composite rate limiting key.

```yaml
# Rate limit by IP + API Key combination
key:
  - type: clientIp
  - type: header
    name: "X-API-Key"
# Rate limiting key example: 192.168.1.1_my-api-key-123
```

#### headerNames

Custom rate limiting response header names. When not configured, the default `X-RateLimit-*` style is used.
When configured, **only explicitly specified headers** are shown; unspecified ones will not appear in the response.

| Sub-parameter | Description | Default Header |
|---------------|-------------|----------------|
| `limit` | Quota | `X-RateLimit-Limit` |
| `remaining` | Remaining quota | `X-RateLimit-Remaining` |
| `reset` | Window reset timestamp | `X-RateLimit-Reset` |
| `retryIn` | Retry wait time (human-readable) | None |

### Response Headers

When a request is allowed, the response includes these headers (`showLimitHeaders: true`):

```http
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 73
X-RateLimit-Reset: 1707465600
```

When a request is rejected (HTTP 429):

```http
HTTP/1.1 429 Too Many Requests
Content-Type: application/json
Retry-After: 1
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1707465600

{"message":"Rate limit exceeded"}
```

## Cluster Rate Limiting Mode (Cluster Scope)

### What is Cluster Mode?

By default (`scope: Instance`), each Gateway instance rate-limits independently.
If you set `rate: 1000` with 4 Gateway instances, the cluster allows a total of
`4 x 1000 = 4000` requests — usually not what you want.

Cluster mode solves this: the configured `rate` represents the **total cluster-wide quota**,
and the system automatically distributes it across each Gateway instance.

### Core Formula

```
Per-instance effective rate = ceil(rate x skewTolerance / instance count)
```

### How It Works

```text
scope: Cluster, rate: 1000, skewTolerance: 1.2, gateway_count: 4
---------------------------------------------------------------

Gateway Instance 1:  effective_rate = ceil(1000 x 1.2 / 4) = 300 req/s
Gateway Instance 2:  effective_rate = ceil(1000 x 1.2 / 4) = 300 req/s
Gateway Instance 3:  effective_rate = ceil(1000 x 1.2 / 4) = 300 req/s
Gateway Instance 4:  effective_rate = ceil(1000 x 1.2 / 4) = 300 req/s
                                                              ----------
                                       Max Total (even) = 1200 req/s (upper bound)
                                       Actual Total (70/30 skew) ≈ 1000 req/s
```

After startup, the Gateway obtains the current cluster instance count from the Controller via the `WatchServerMeta` gRPC stream in real-time.
When instance counts change (scaling), the Controller pushes the new count event-driven, and the Gateway updates immediately via a global atomic variable.
The data plane only needs one `AtomicU32::load` + floating-point calculation (~2ns), **zero additional overhead**.

### skewTolerance Details

#### Why is this parameter needed?

Cluster mode distributes quota evenly across instances, but in practice, load balancers cannot distribute traffic perfectly evenly.

Example: 2 instances, quota `1000 req/s`, each gets `500`. If actual traffic is 70/30:
- Instance 1 receives 700 requests, allows 500, **rejects 200**
- Instance 2 receives 300 requests, allows 300, **wastes 200 quota**
- Total allowed 800 / configured 1000 = **80% utilization** (200 fewer requests allowed)

`skewTolerance` gives each instance extra quota headroom to compensate for this waste.

#### Effect Comparison (rate=1000, 2 instances)

| Traffic Distribution | skewTolerance=1.0 | skewTolerance=1.2 (default) | skewTolerance=1.5 |
|---------------------|-------------------|--------------------------|-------------------|
| Per-instance quota | 500 | 600 | 750 |
| **50/50 even** | Allowed 1000 | Allowed 1000 | Allowed 1000 |
| **60/40 slight skew** | Allowed 900 | Allowed **1000** | Allowed 1000 |
| **70/30 moderate skew** | Allowed 800 | Allowed **900** | Allowed 1000 |
| **80/20 significant skew** | Allowed 700 | Allowed **800** | Allowed 950 |
| **Even high traffic cap** | Total <=1000 | Total <=**1200** | Total <=1500 |

#### How to Choose the Right Value?

| Value | Meaning | Use Case |
|-------|---------|----------|
| `1.0` | No headroom, strict even split | Strict non-exceeding needed, very even traffic (e.g., round-robin) |
| `1.2` | **Default**, 20% headroom | Most scenarios, standard K8s Service / NLB load balancing |
| `1.5` | 50% headroom | Sticky sessions, long connections causing significant skew |
| `2.0` | 100% headroom (maximum) | Extreme skew scenarios (consider using Instance mode instead) |

#### Key Tradeoffs

- **Higher skewTolerance** -> better quota utilization, but may exceed configured rate under even high traffic
- **Lower skewTolerance** -> won't exceed limits, but wastes more quota under skew
- **Default 1.2** -> achieves ~100% utilization with typical load balancing (<=60/40 skew), max 20% over

### Fallback Strategy

Cluster mode automatically degrades when the Controller is unavailable, ensuring uninterrupted service:

| Scenario | gateway_count | Behavior |
|----------|---------------|----------|
| Controller pushing normally | Dynamic value (e.g., 4) | Normal cluster rate limiting |
| Controller disconnected, TOML has `gateway_instance_count` | Static value (e.g., 4) | Uses static fallback |
| Controller disconnected, no TOML config | 1 | Equivalent to Instance scope (skewTolerance still applies) |
| Gateway just started, not yet connected to Controller | 1 or TOML value | Graceful startup |

### Response Header Behavior

In Cluster mode, response headers show the **effective rate** (the current instance's actual quota including skewTolerance),
not the global configured value. This lets clients accurately know their available quota.

```http
# Cluster scope, rate=1000, skewTolerance=1.2, 4 instances
X-RateLimit-Limit: 300
X-RateLimit-Remaining: 173
X-RateLimit-Reset: 1707465600
```

## Usage Examples

### Example 1: Basic IP Rate Limiting

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rate-limit-by-ip
  namespace: default
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 100
        interval: "1s"
        key:
          - type: clientIp
```

Maximum 100 requests per second per client IP.

### Example 2: Rate Limiting by API Key

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rate-limit-by-api-key
  namespace: default
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 1000
        interval: "1m"
        key:
          - type: header
            name: "X-API-Key"
        onMissingKey: Deny
        rejectMessage: "API key required and rate limit exceeded"
```

Maximum 1000 requests per minute per API Key. Requests without an API Key are rejected directly.

### Example 3: Composite Key Rate Limiting

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rate-limit-composite
  namespace: default
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 50
        interval: "1s"
        key:
          - type: clientIp
          - type: path
        defaultKey: "anonymous"
```

Rate limiting by IP + Path combination, max 50 requests per second per IP per path.
When the key cannot be extracted, `anonymous` is used as the default key.

### Example 4: Cluster Rate Limiting (Cluster Scope)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rate-limit-cluster
  namespace: default
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 1000
        interval: "1s"
        scope: Cluster
        key:
          - type: clientIp
```

Cluster-wide quota of 1000 req/s, automatically distributed across instances, with 20% default skew tolerance.

### Example 5: Cluster Rate Limiting + Strict Mode

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rate-limit-cluster-strict
  namespace: default
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 1000
        interval: "1s"
        scope: Cluster
        skewTolerance: 1.0
        key:
          - type: clientIp
```

Cluster-wide quota of 1000 req/s, strict even distribution, no exceeding (suitable for round-robin load balancing).

### Example 6: Cluster Rate Limiting + Sticky Session

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rate-limit-cluster-sticky
  namespace: default
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 10000
        interval: "1m"
        scope: Cluster
        skewTolerance: 1.5
        key:
          - type: header
            name: "X-API-Key"
        showLimitHeaders: true
        headerNames:
          limit: "RateLimit-Limit"
          remaining: "RateLimit-Remaining"
          reset: "RateLimit-Reset"
```

Cluster-level rate limiting by API Key, IETF-style custom response headers, increased skew tolerance for sticky sessions.

### Example 7: Custom Reject Response

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rate-limit-custom-reject
  namespace: default
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 10
        interval: "1s"
        key:
          - type: clientIp
        rejectStatus: 503
        rejectMessage: "Service temporarily unavailable, please try again later"
        showLimitHeaders: false
```

Uses 503 status code with a custom message, rate limiting headers hidden.

## Global Configuration (TOML)

RateLimit global parameters can be set in the Gateway's TOML configuration file:

```toml
[rate_limit]
# CMS default precision (K units, 1K = 1024 slots, default 64K ≈ 4MB)
default_estimator_slots_k = 64

# CMS maximum precision (K units, default 1024K ≈ 64MB)
max_estimator_slots_k = 1024

# Cluster scope static fallback (used when Controller is unavailable, default 1)
gateway_instance_count = 4
```

### CMS Precision and Memory

| estimatorSlotsK | Actual Slots | Memory/Instance | Use Case |
|-----------------|-------------|----------------|----------|
| 1 | 1K | ~64KB | Minimum precision |
| 8 | 8K | ~512KB | Low cardinality keys |
| 64 (default) | 64K | ~4MB | General use |
| 256 | 256K | ~16MB | High cardinality keys |
| 1024 | 1M | ~64MB | Maximum precision |

Each RateLimit plugin instance creates an independent CMS Rate instance.
Higher precision means fewer hash collisions and more accurate rate limiting, but higher memory overhead.

## Algorithm Details

### Count-Min Sketch

CMS is a probabilistic data structure that uses multiple hash functions (default 4) and a fixed-size counter array for frequency estimation:

```text
Request key = "192.168.1.1"
       │
       ├─ hash1("192.168.1.1") → slot[23]  ++
       ├─ hash2("192.168.1.1") → slot[87]  ++
       ├─ hash3("192.168.1.1") → slot[142] ++
       └─ hash4("192.168.1.1") → slot[256] ++
       
estimate = min(slot[23], slot[87], slot[142], slot[256])
```

- **Space efficient**: Fixed memory, doesn't grow with key count
- **Bounded error**: Estimated value >= actual value (only overestimates, never underestimates), suitable for rate limiting
- **High concurrency**: Uses lock-free atomic operations

### Sliding Window

Pingora's Rate uses a dual-slot (red/blue) design for smooth sliding windows:

```text
Timeline: ──────────────────────────────────>
          [  Window A  ][  Window B  ][  Window C  ]
                        ↑ Old slot cleared on switch
```

On window switch, the old counter slot is cleared and reused, new requests write to the new slot.
Queries use the current active slot's count as the estimate.

## Notes

1. **Cluster mode depends on Controller**: If the Controller is unavailable, gateway_count falls back to 1 or the TOML static value
2. **CMS bounded overestimation**: Near the rate limit threshold, there may be slight margin of error (always conservative); precision adjustable via `estimatorSlotsK`
3. **scope and skewTolerance validation**: `skewTolerance` outside the 1.0~2.0 range is automatically clamped; Cluster scope requires `rate >= 2`
4. **Independent plugin instances**: Different RateLimit plugin instances have separate CMS states and do not affect each other
5. **Real-time updates**: After updating the EdgionPlugins resource, configuration is automatically hot-reloaded
6. **Fail-open**: On configuration validation failure, requests are allowed through (fail-open), with error information logged

## FAQ

### Q: Should I use Instance or Cluster mode?

A:
- If you only care about per-instance rate limiting (e.g., protecting backends from a single instance overload), use **Instance**
- If you need to control the global API quota (e.g., SLA commitment of 1000 req/s), use **Cluster**

### Q: Will Cluster mode precisely limit to the configured rate?

A: Not exactly. Due to traffic skew and CMS probabilistic characteristics, actual total throughput
fluctuates between `rate` and `rate x skewTolerance`. Setting `skewTolerance: 1.0` guarantees no exceeding,
but may waste quota under skew.

### Q: Will scaling cause rate limiting fluctuations?

A: When instance counts change, each Gateway's effective rate is immediately recalculated, but historical CMS counts are not cleared.
Typically smooths out naturally after one `interval` period.

### Q: How can I verify that Cluster mode is active?

A: Check the `X-RateLimit-Limit` response header. In Cluster mode, it shows the effective rate (post-distribution value),
not the configured global rate. For example, with `rate: 1000`, 4 instances, and `skewTolerance: 1.2`, the header shows `300`.

### Q: What size should I set for estimatorSlotsK?

A: For most scenarios, the default 64K (~4MB) is sufficient. If your rate limiting key cardinality is very large
(e.g., rate limiting by IP with millions of different IPs), consider increasing to 256K.
Memory overhead = `estimatorSlotsK x 64KB`.

### Q: How do I configure multiple rate limiting dimensions?

A: Create multiple RateLimit plugin instances, each with a different key configuration:

```yaml
requestPlugins:
  # Global IP rate limiting
  - type: RateLimit
    config:
      rate: 1000
      key:
        - type: clientIp
  # Fine-grained per-path rate limiting
  - type: RateLimit
    config:
      rate: 50
      key:
        - type: clientIp
        - type: path
```
