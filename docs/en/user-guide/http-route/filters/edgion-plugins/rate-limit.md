# RateLimit Plugin

## Overview

The `RateLimit` plugin implements high-performance request rate limiting using **Pingora's Count-Min Sketch (CMS) algorithm**. This algorithm has fixed memory usage and O(1) time complexity, making it ideal for high-concurrency scenarios.

## Features

- **CMS algorithm**: Probabilistic counting based on Count-Min Sketch with high memory efficiency
- **Dual-slot sliding window**: Smooth rate estimation, avoiding window boundary issues
- **Multi-dimensional rate limiting**: Supports limiting by IP, Header, Cookie, Query, Path, etc.
- **Response header information**: Returns `X-RateLimit-*` headers for client-side rate limit awareness
- **Fixed memory**: Memory usage remains constant regardless of key cardinality

## Configuration Parameters

### RateLimitConfig

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `rate` | `integer` | ✅ | - | Number of requests allowed per time window |
| `interval` | `string` | ❌ | `"1s"` | Time window (e.g., `"1s"`, `"10s"`, `"1m"`) |
| `key` | `LimitKey` | ❌ | `ClientIP` | Rate limiting dimension |
| `onMissingKey` | `string` | ❌ | `"Allow"` | Behavior when key is missing: `Allow` or `Deny` |
| `defaultKey` | `string` | ❌ | - | Default value when key is missing |
| `rejectStatus` | `integer` | ❌ | `429` | HTTP status code on rejection |
| `rejectMessage` | `string` | ❌ | `"Rate limit exceeded"` | Response message on rejection |
| `showLimitHeaders` | `boolean` | ❌ | `true` | Whether to return rate limit response headers |
| `headerNames` | `LimitHeaderNames` | ❌ | - | Custom response header names |
| `estimatorSlotsK` | `integer` | ❌ | Global default | CMS slot count in K units (see below) |

### LimitKey

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `source` | `string` | ❌ | Data source: `ClientIP`, `Header`, `Cookie`, `Query`, `Path`, `ClientIPAndPath` |
| `name` | `string` | Depends | Key name (required for Header/Cookie/Query) |

### LimitHeaderNames

| Parameter | Type | Description |
|-----------|------|-------------|
| `limit` | `string` | Rate limit cap header name |
| `remaining` | `string` | Remaining quota header name |
| `reset` | `string` | Reset time header name |
| `retryIn` | `string` | Retry time header name |

## Global Configuration

RateLimit supports global defaults via the Gateway's TOML configuration file (`edgion-gateway.toml`):

```toml
# config/edgion-gateway.toml

[rate_limit]
default_estimator_slots_k = 64     # Default 64K slots (~4MB)
max_estimator_slots_k = 1024       # Maximum 1024K slots (~64MB)
```

### RateLimitGlobalConfig

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `default_estimator_slots_k` | `integer` | `64` | Default CMS slot count (K unit, 64K = ~4MB) |
| `max_estimator_slots_k` | `integer` | `1024` | Maximum allowed slot count (K unit, 1024K = ~64MB) |

### CMS Estimator Slots

`estimatorSlotsK` controls the precision and memory usage of the Count-Min Sketch data structure.

**Unit**: Configuration values are in **K** units, where 1K = 1024 slots.

- **More slots** → fewer hash collisions → higher counting accuracy → more memory usage
- **Fewer slots** → more hash collisions → potential count overestimation → less memory usage

**Memory formula**: `Memory ≈ K value × 64KB`

| Config (K) | Actual Slots | Memory | Use Case |
|-----------|-------------|--------|----------|
| 1 | 1K | ~64KB | Minimum configuration |
| 8 | 8K | ~512KB | Low cardinality scenarios |
| **64** | **64K** | **~4MB** | **Default, most scenarios** |
| 256 | 256K | ~16MB | High cardinality key scenarios |
| **1024** | **1M** | **~64MB** | **Maximum** |

**Note**: Slot count is clamped to the `[1, max_estimator_slots_k]` range.

## Algorithm Details

### Count-Min Sketch + Dual-Slot Sliding Window

```
┌─────────────────────────────────────────────────────────┐
│           Pingora Rate Estimator                        │
├─────────────────────────────────────────────────────────┤
│                                                         │
│   ┌─────────────────┐    ┌─────────────────┐           │
│   │   Red Slot      │    │   Blue Slot     │           │
│   │  (current)      │    │  (previous)     │           │
│   │                 │    │                 │           │
│   │  Count-Min      │    │  Count-Min      │           │
│   │  Sketch         │    │  Sketch         │           │
│   └─────────────────┘    └─────────────────┘           │
│            ↓                     ↓                      │
│        ┌───────────────────────────────┐               │
│        │  rate = red + blue × (1-α)   │               │
│        │  α = current window progress │               │
│        └───────────────────────────────┘               │
│                                                         │
│   Properties:                                           │
│   - Fixed memory usage (independent of key count)      │
│   - O(1) time complexity                               │
│   - Lock-free atomic operations, high concurrency      │
│   - Smooth sliding window estimation                   │
└─────────────────────────────────────────────────────────┘
```

## Response Headers

When `showLimitHeaders: true`, the response includes these headers:

| Header | Description |
|--------|-------------|
| `X-RateLimit-Limit` | Rate limit (requests per window) |
| `X-RateLimit-Remaining` | Remaining quota |
| `X-RateLimit-Reset` | Unix timestamp when the window resets |
| `Retry-After` | (only when exceeded) Suggested retry seconds |

## Use Cases

### 1. Basic Rate Limiting (by IP)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-rate-limit
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 100              # 100 requests per second
        interval: "1s"
        key:
          source: ClientIP
        rejectStatus: 429
        rejectMessage: "Too many requests"
```

### 2. Rate Limiting by API Key (Per-Minute Quota)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-key-rate-limit
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 1000             # 1000 requests per minute
        interval: "1m"
        key:
          source: Header
          name: "X-API-Key"
        onMissingKey: Deny     # Deny when API Key is missing
        rejectMessage: "API rate limit exceeded"
```

### 3. Rate Limiting by Path

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: path-rate-limit
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 20
        interval: "1s"
        key:
          source: Path         # Independent rate limiting per path
```

### 4. Rate Limiting by IP + Path Combination

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ip-path-rate-limit
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 10
        interval: "1s"
        key:
          source: ClientIPAndPath  # IP:Path combination
```

### 5. Custom Response Header Names

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: custom-headers-limiter
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 100
        interval: "1s"
        key:
          source: ClientIP
        headerNames:
          limit: "RateLimit-Limit"
          remaining: "RateLimit-Remaining"
          reset: "RateLimit-Reset"
          retryIn: "Retry-After"
```

### 6. Using Default Key (Fallback Rate Limiting)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: fallback-limiter
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 50
        interval: "1s"
        key:
          source: Header
          name: "X-User-ID"
        defaultKey: "anonymous"  # Use this key when User-ID is missing
```

### 7. Custom CMS Slots (High Precision)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: high-precision-limiter
spec:
  requestPlugins:
    - type: RateLimit
      config:
        rate: 1000
        interval: "1m"
        key:
          source: Header
          name: "X-API-Key"
        estimatorSlotsK: 256    # 256K slots = 16MB memory
```

## Comparison with Other Gateways

| Feature | APISIX | Kong | Traefik | Edgion |
|---------|--------|------|---------|--------|
| **Algorithm** | Leaky bucket/Fixed window | Fixed window | Token bucket | CMS sliding window |
| **Memory efficiency** | Medium | Low | Medium | High (fixed memory) |
| **Response headers** | ❌ | ✅ | ✅ | ✅ |
| **Rate limiting dimensions** | var combinations | 6 types | 3 types | 6 types |
| **Window smoothing** | ❌ | ❌ | ❌ | ✅ (dual-slot design) |

## Notes

1. **rate and interval**: `rate` is the number of requests allowed per `interval`. For example, `rate: 100, interval: "1s"` means 100 requests per second.

2. **CMS precision**: Count-Min Sketch is a probabilistic data structure that may slightly overestimate counts (but never underestimates). For most rate limiting scenarios, this margin of error is acceptable.

3. **Memory usage**: Regardless of how many different keys exist, CMS memory usage is fixed. This makes it ideal for high-cardinality scenarios (e.g., many different IPs).
   - Each RateLimit plugin instance has its own independent Rate instance
   - Memory usage ≈ `estimatorSlotsK × 64KB`
   - Default 64K slots ≈ 4MB

4. **estimatorSlotsK configuration**:
   - Values are in K units (1K = 1024 slots)
   - If not configured, uses the global `default_estimator_slots_k` (default 64)
   - If the configured value exceeds `max_estimator_slots_k`, it is automatically clamped
   - Adjust based on key cardinality and precision requirements

5. **Distributed rate limiting**: The current version only supports single-instance rate limiting; distributed support will come in future releases.

## Related Plugins

- [IpRestriction](./ip-restriction.md) - IP access control
- [RequestRestriction](./request-restriction.md) - Request restriction
