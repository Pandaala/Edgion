# RateLimitRedis 插件

> **🔌 Edgion 扩展**
>  
> RateLimitRedis 是 `EdgionPlugins` CRD 提供的基于 Redis 的集群级精确限流插件，不属于标准 Gateway API。

## 什么是 RateLimitRedis？

[RateLimit 插件](rate-limit.md) 使用进程内 Count-Min Sketch 算法，延迟极低（纳秒级），但每个网关实例独立计数，多副本场景下总体放量是配置值的倍数。RateLimitRedis 通过 Redis 实现跨实例的**精确**集群限流：所有实例共享同一个 Redis 计数器，配置的速率就是整个集群的实际速率。

### RateLimit 与 RateLimitRedis 对比

| 特性 | RateLimit | RateLimitRedis |
|------|-----------|----------------|
| 实现方式 | 进程内 Count-Min Sketch | Redis Lua 脚本（原子操作） |
| 延迟 | 纳秒级 | 0.1–1 ms（网络 RTT） |
| 精确度 | 近似（CMS 误差） | 精确 |
| 计数范围 | 单实例 | 集群级 |
| 外部依赖 | 无 | 需要 Redis |
| Redis 故障 | 不适用 | 可配置 Allow / Deny |
| 适用场景 | 高吞吐、可容忍误差 | 需要精确配额、集群级限流 |

## 快速开始

### 最简单的配置

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

### 前置条件

`redisRef` 必须指向一个已存在的 `LinkSys` 资源（类型为 Redis）。确保在同一命名空间或目标命名空间中已创建对应的 LinkSys。

### 默认值说明

- 默认按客户端 IP 限流
- 默认算法为 SlidingWindow（滑动窗口）
- 默认 Redis 故障时放行请求（fail-open）
- 默认返回 `X-RateLimit-Limit` 和 `X-RateLimit-Remaining` 响应头；`X-RateLimit-Reset` 仅在拒绝（429）响应中返回（见下方 HeaderNames）
- 默认拒绝状态码为 429

---

## 配置参数

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `redisRef` | String | —（必填） | Redis LinkSys 资源引用，格式 `"namespace/name"`。仅支持字母数字、`-`、`_` |
| `policies` | Array\<Object\> | —（必填） | 限流策略列表，至少一条，最多 10 条，见下方 Policy 配置 |
| `key` | Array\<Object\> | `[{type: clientIp}]` | 限流维度列表，多个 key 用 `_` 拼接，见下方 Key 配置 |
| `onMissingKey` | Enum | `Allow` | key 缺失时的行为。可选值：`Allow` \| `Deny` |
| `defaultKey` | String | 无 | key 缺失时的默认值，优先于 `onMissingKey` |
| `rejectStatus` | Integer | `429` | 拒绝时的 HTTP 状态码（100–599） |
| `rejectMessage` | String | 无 | 拒绝时的 JSON 响应体 |
| `showLimitHeaders` | Boolean | `true` | 是否返回限流响应头 |
| `headerNames` | Object | 无 | 自定义响应头名称，见下方 HeaderNames 配置 |

### Redis 参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `onRedisFailure` | Enum | `Allow` | Redis 不可用**或** `requestTimeout` 超时时执行的策略。涵盖两种情况：(a) Redis 可检测到已宕机——零 I/O 快速弹出；(b) Redis 健康但响应慢或报错——有界等待后放弃。两种情况均通过此单一策略处理。可选值：`Allow`（放行）\| `Deny`（拒绝） |
| `requestTimeout` | String | 未设置 | 可选的多策略 Redis 检查总体耗时上限（例如 `"500ms"`）。未设置时不设置聚合超时，仍适用 LinkSys Redis CRD 中的单命令超时。设置后若超时，则执行 `onRedisFailure` 策略 |
| `keyPrefix` | String | `"edgion:rl:"` | Redis key 前缀，必须以 `:` 结尾，仅 ASCII 可打印字符 |
| `maxKeyLen` | Integer | `4096` | key 部分的最大字节数（64–65536），防止恶意超长 key 导致 Redis OOM |

### 高级参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `cost` | Integer | `1` | 每次请求的消耗单位（加权限流）。`cost: 5` 表示每次请求计 5 次 |

---

### Policy 子字段

`policies` 数组中每个策略包含以下字段：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `rate` | Integer | —（必填） | 时间窗口内允许的最大请求数（或消耗单位数），必须 > 0 |
| `interval` | String | —（必填） | 时间窗口，如 `"1s"`、`"1m"`、`"1h"`、`"24h"`，也支持组合格式如 `"1h30m"`。最小 100ms |
| `algorithm` | Enum | `SlidingWindow` | 限流算法。可选值见下表 |

#### 算法说明

| 值 | 说明 |
|----|------|
| `SlidingWindow` | **滑动窗口**（默认）。加权平均当前窗口与上一窗口，避免边界突刺。精度与性能最佳平衡 |
| `FixedWindow` | **固定窗口**。最简单，每个窗口边界重置计数，存在边界突刺问题 |
| `TokenBucket` | **令牌桶**。允许受控突发，桶容量 = rate。适合允许短期突发的场景 |

多条策略**独立评估**，所有策略均通过请求才被放行；任意一条拒绝则请求被拒绝（最严格的策略生效）。

---

### Key 配置

与 [RateLimit 插件](rate-limit.md) 相同，支持以下 key 类型：

| type | 需要 name | 说明 |
|------|-----------|------|
| `clientIp` | 否 | 客户端 IP（经过真实 IP 提取后） |
| `header` | 是 | 指定请求头的值 |
| `cookie` | 是 | 指定 Cookie 的值 |
| `query` | 是 | 指定 Query 参数的值 |
| `path` | 否 | 请求路径 |
| `method` | 否 | HTTP 方法 |
| `ctx` | 是 | 上下文变量（由其他插件设置） |
| `clientIpAndPath` | 否 | 客户端 IP + 路径组合 |

多个 key 拼接示例：

```yaml
key:
  - type: clientIp
  - type: header
    name: "X-API-Key"
```

生成的 Redis key 形如：`edgion:rl:sw:192.168.1.1_apikey123`

---

### HeaderNames 子字段

自定义限流响应头名称。只有显式配置的字段才会在响应中出现。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `limit` | String | 无 | 速率上限头名称（默认 `X-RateLimit-Limit`） |
| `remaining` | String | 无 | 剩余配额头名称（默认 `X-RateLimit-Remaining`） |
| `reset` | String | 无 | 窗口重置时间头名称（默认 `X-RateLimit-Reset`）。仅在拒绝（429）响应中返回——Redis Lua 脚本不会为放行的请求返回窗口重置时间戳 |
| `retryIn` | String | 无 | 人类可读的重试时间（如 `"1.5s"`） |

---

## 常见配置场景

### 1. 简单集群限流（单策略按 IP）

整个集群每个 IP 每秒限 100 次，所有网关实例共享同一计数器。

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

### 2. 多策略限流（同时限 QPS 和日配额）

同一个 API Key 每秒限 50 次，同时每小时限 5000 次。两条策略独立评估，任意一条触发则拒绝。

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

### 3. 高安全场景——Redis 故障时拒绝

对于付费 API 或安全敏感的接口，配置 `onRedisFailure: Deny`，确保 Redis 不可用时不会放行超额请求。

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

## 常见问题

### Q1：`redisRef` 格式是什么？

**A**：格式为 `"namespace/name"`，对应一个 `LinkSys` 类型为 Redis 的资源。例如 `"default/shared-redis"`。字符串中只能包含字母数字、`-`、`_`，不能有特殊字符或斜杠以外的分隔符。

### Q2：`interval` 最小可以设多小？

**A**：最小 100 ms。小于 100 ms 的 interval 会在校验阶段报错，原因是过小的窗口会频繁写入 Redis，可能压垮 Redis 集群。对于极高频率限流，建议使用 [RateLimit 插件](rate-limit.md)（进程内，纳秒级）。

### Q3：多条策略是"OR"还是"AND"关系？

**A**：是 **AND** 关系——所有策略必须同时通过，请求才被放行。任意一条策略触发限流，请求即被拒绝。

### Q4：`keyPrefix` 为什么必须以 `:` 结尾？

**A**：这是 Redis key 命名规范，用于在 Redis 中清晰分隔命名空间。默认值 `"edgion:rl:"` 已符合规范。如果自定义前缀忘记加 `:`，校验时会报错。

### Q5：RateLimitRedis 和 RateLimit 可以同时挂载吗？

**A**：可以。两个插件独立工作，请求必须同时通过两者才能到达后端。常见组合：RateLimit 在前做粗粒度防护（本地快速拦截流量洪峰），RateLimitRedis 在后做精确集群配额管理。

---

## 相关文档

- [RateLimit 插件](rate-limit.md) — 进程内近似限流（高性能、低延迟）
- [IpRestriction 插件](ip-restriction.md) — 基于 IP 的访问控制
- [KeyAuth 插件](key-auth.md) — API Key 认证
- [JwtAuth 插件](jwt-auth.md) — JWT 认证
