# RateLimiter 插件

## 概述

`RateLimiter` 插件使用 **Pingora 的 Count-Min Sketch (CMS) 算法** 实现高性能请求速率限制。该算法具有固定的内存占用和 O(1) 的时间复杂度，非常适合高并发场景。

## 功能特性

- **CMS 算法**：基于 Count-Min Sketch 的概率计数，内存效率高
- **双槽滑动窗口**：平滑的速率估计，避免窗口边界问题
- **多维度限流**：支持按 IP、Header、Cookie、Query、Path 等维度
- **响应头信息**：返回 `X-RateLimit-*` 头，便于客户端感知限流状态
- **固定内存**：无论 key 基数多大，内存占用固定

## 配置参数

### RateLimiterConfig

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `rate` | `integer` | ✅ | - | 每个时间窗口允许的请求数 |
| `interval` | `string` | ❌ | `"1s"` | 时间窗口（如 `"1s"`, `"10s"`, `"1m"`） |
| `key` | `LimitKey` | ❌ | `ClientIP` | 限流维度 |
| `onMissingKey` | `string` | ❌ | `"Allow"` | key 缺失时的行为：`Allow` 或 `Deny` |
| `defaultKey` | `string` | ❌ | - | key 缺失时使用的默认值 |
| `rejectStatus` | `integer` | ❌ | `429` | 拒绝时的 HTTP 状态码 |
| `rejectMessage` | `string` | ❌ | `"Rate limit exceeded"` | 拒绝时的响应消息 |
| `showLimitHeaders` | `boolean` | ❌ | `true` | 返回限流响应头 |
| `headerNames` | `LimitHeaderNames` | ❌ | - | 自定义响应头名称 |
| `estimatorSlotsK` | `integer` | ❌ | 全局默认值 | CMS 槽位数，单位 K（见下方说明） |

### LimitKey

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `source` | `string` | ❌ | 数据源：`ClientIP`、`Header`、`Cookie`、`Query`、`Path`、`ClientIPAndPath` |
| `name` | `string` | 视情况 | 键名（Header/Cookie/Query 时必填） |

### LimitHeaderNames

| 参数 | 类型 | 说明 |
|------|------|------|
| `limit` | `string` | 限流上限头名称 |
| `remaining` | `string` | 剩余配额头名称 |
| `reset` | `string` | 重置时间头名称 |
| `retryIn` | `string` | 重试时间头名称 |

## 全局配置

RateLimiter 支持通过 Gateway 的 TOML 配置文件 (`edgion-gateway.toml`) 配置全局默认值：

```toml
# config/edgion-gateway.toml

[rate_limiter]
default_estimator_slots_k = 64     # 默认 64K 槽位 (~4MB)
max_estimator_slots_k = 1024       # 最大 1024K 槽位 (~64MB)
```

### RateLimiterGlobalConfig

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `default_estimator_slots_k` | `integer` | `64` | 默认 CMS 槽位数 (K为单位, 64K = ~4MB) |
| `max_estimator_slots_k` | `integer` | `1024` | 最大允许槽位数 (K为单位, 1024K = ~64MB) |

### CMS 估算器槽位说明

`estimatorSlotsK` 控制 Count-Min Sketch 数据结构的精度和内存使用。

**单位说明**：配置值以 **K** 为单位，1K = 1024 个槽位。

- **槽位越多** → 哈希冲突越少 → 计数精度越高 → 内存占用越大
- **槽位越少** → 哈希冲突越多 → 可能高估计数 → 内存占用越小

**内存计算公式**：`内存 ≈ K值 × 64KB`

| 配置值 (K) | 实际槽位 | 内存占用 | 适用场景 |
|-----------|---------|----------|----------|
| 1 | 1K | ~64KB | 最小配置 |
| 8 | 8K | ~512KB | 低基数场景 |
| **64** | **64K** | **~4MB** | **默认，大多数场景** |
| 256 | 256K | ~16MB | 高基数 key 场景 |
| **1024** | **1M** | **~64MB** | **最大值** |

**注意**：槽位数会被限制在 `[1, max_estimator_slots_k]` 范围内。

## 算法说明

### Count-Min Sketch + 双槽滑动窗口

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
│        │  α = 当前时间窗口进度        │               │
│        └───────────────────────────────┘               │
│                                                         │
│   特点：                                                │
│   - 固定内存占用（与 key 数量无关）                    │
│   - O(1) 时间复杂度                                    │
│   - 无锁原子操作，高并发友好                           │
│   - 平滑的滑动窗口估计                                 │
└─────────────────────────────────────────────────────────┘
```

## 响应头

当 `showLimitHeaders: true` 时，响应会包含以下头：

| Header | 说明 |
|--------|------|
| `X-RateLimit-Limit` | 速率限制（每窗口请求数） |
| `X-RateLimit-Remaining` | 剩余配额 |
| `X-RateLimit-Reset` | 窗口重置的 Unix 时间戳 |
| `Retry-After` | (仅超限时) 建议重试秒数 |

## 使用场景

### 1. 基本速率限制（按 IP）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-rate-limiter
spec:
  requestPlugins:
    - type: RateLimiter
      config:
        rate: 100              # 每秒 100 请求
        interval: "1s"
        key:
          source: ClientIP
        rejectStatus: 429
        rejectMessage: "Too many requests"
```

### 2. 按 API Key 限流（每分钟配额）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-key-rate-limiter
spec:
  requestPlugins:
    - type: RateLimiter
      config:
        rate: 1000             # 每分钟 1000 请求
        interval: "1m"
        key:
          source: Header
          name: "X-API-Key"
        onMissingKey: Deny     # 无 API Key 时拒绝
        rejectMessage: "API rate limit exceeded"
```

### 3. 按路径限流

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: path-rate-limiter
spec:
  requestPlugins:
    - type: RateLimiter
      config:
        rate: 20
        interval: "1s"
        key:
          source: Path         # 每个路径独立限流
```

### 4. 按 IP + 路径组合限流

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ip-path-rate-limiter
spec:
  requestPlugins:
    - type: RateLimiter
      config:
        rate: 10
        interval: "1s"
        key:
          source: ClientIPAndPath  # IP:Path 组合
```

### 5. 自定义响应头名称

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: custom-headers-limiter
spec:
  requestPlugins:
    - type: RateLimiter
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

### 6. 使用默认 Key（兜底限流）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: fallback-limiter
spec:
  requestPlugins:
    - type: RateLimiter
      config:
        rate: 50
        interval: "1s"
        key:
          source: Header
          name: "X-User-ID"
        defaultKey: "anonymous"  # 无 User-ID 时使用此 key
```

### 7. 自定义 CMS 槽位数（高精度场景）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: high-precision-limiter
spec:
  requestPlugins:
    - type: RateLimiter
      config:
        rate: 1000
        interval: "1m"
        key:
          source: Header
          name: "X-API-Key"
        estimatorSlotsK: 256    # 256K 槽位 = 16MB 内存
```

## 与其他网关对比

| 功能 | APISIX | Kong | Traefik | Edgion |
|------|--------|------|---------|--------|
| **算法** | 漏桶/固定窗口 | 固定窗口 | 令牌桶 | CMS 滑动窗口 |
| **内存效率** | 中 | 低 | 中 | 高（固定内存） |
| **响应头** | ❌ | ✅ | ✅ | ✅ |
| **限流维度** | var组合 | 6种 | 3种 | 6种 |
| **窗口平滑** | ❌ | ❌ | ❌ | ✅（双槽设计） |

## 注意事项

1. **rate 与 interval**：`rate` 是每个 `interval` 内允许的请求数。例如 `rate: 100, interval: "1s"` 表示每秒 100 请求。

2. **CMS 精度**：Count-Min Sketch 是概率数据结构，可能会略微高估计数（但不会低估）。对于大多数限流场景，这个误差是可接受的。

3. **内存使用**：无论有多少不同的 key，CMS 的内存占用是固定的。这使得它非常适合高基数场景（如大量不同 IP）。
   - 每个 RateLimiter 插件实例拥有独立的 Rate 实例
   - 内存占用 ≈ `estimatorSlotsK × 64KB`
   - 默认 64K 槽位 ≈ 4MB

4. **estimatorSlotsK 配置**：
   - 配置值以 K 为单位（1K = 1024 槽位）
   - 如果未配置，使用全局 `default_estimator_slots_k`（默认 64）
   - 如果配置值超过 `max_estimator_slots_k`，会自动截断
   - 建议根据 key 基数和精度需求调整

5. **分布式限流**：当前版本仅支持单机限流，分布式场景需后续支持。

## 相关插件

- [IpRestriction](./ip-restriction.md) - IP 访问控制
- [RequestRestriction](./request-restriction.md) - 请求限制
