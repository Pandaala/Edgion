# RateLimitRedis 插件

> **🔌 Edgion 扩展**
>  
> RateLimitRedis 是 `EdgionPlugins` CRD 提供的基于 Redis 的集群级精确限流插件，不属于标准 Gateway API。

## 什么是 RateLimitRedis？

[RateLimitLocal 插件](rate-limit-local.md) 使用进程内 Count-Min Sketch 算法，延迟极低（纳秒级），但每个网关实例独立计数，多副本场景下总体放量是配置值的倍数。RateLimitRedis 通过 Redis 实现跨实例的**精确**集群限流：所有实例共享同一个 Redis 计数器，配置的速率就是整个集群的实际速率。

### RateLimitLocal 与 RateLimitRedis 对比

| 特性 | RateLimitLocal | RateLimitRedis |
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
          - type: remoteIp
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
| `key` | Array\<Object\> | `[{type: remoteIp}]` | 限流维度列表。单个 key 原样使用，多个 key 带长度前缀后用 `_` 拼接，见下方 Key 配置 |
| `onKeyMiss` | Enum | `Allow` | key 缺失时的行为。可选值：`Allow` \| `Deny` |
| `defaultKey` | String | 无 | key 缺失时的默认值，优先于 `onKeyMiss` |
| `rejectStatus` | Integer | `429` | 拒绝时的最终 HTTP 状态码（200–599） |
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
| `cost` | Integer | `1` | 每次请求的消耗单位，范围为 1–1,000,000。`cost: 5` 表示每次请求计 5 次 |

---

### Policy 子字段

`policies` 数组中每个策略包含以下字段：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `rate` | Integer | —（必填） | 时间窗口内允许的最大请求数（或消耗单位数），范围为 1–1,000,000 |
| `interval` | String | —（必填） | 整毫秒时间窗口，范围为 100ms–2,147,483,647ms，例如 `"1s"`、`"1m"`、`"1h"`、`"24h"`，也支持组合格式如 `"1h30m"` |
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

与 [RateLimitLocal 插件](rate-limit-local.md) 相同，支持以下 key 类型：

| type | 需要 name | 说明 |
|------|-----------|------|
| `remoteIp` | 否 | 解析后的客户端 IP |
| `header` | 是 | 指定请求头的值 |
| `cookie` | 是 | 指定 Cookie 的值 |
| `query` | 是 | 指定 Query 参数的值 |
| `path` | 否 | 请求路径 |
| `method` | 否 | HTTP 方法 |
| `ctx` | 是 | 上下文变量（由其他插件设置） |
| `remoteIpAndPath` | 否 | 解析后的客户端 IP + 路径组合 |

多个 key 拼接示例：

```yaml
key:
  - type: remoteIp
  - type: header
    name: "X-API-Key"
```

生成的 Redis key 形如 `{keyPrefix}{pluginScope}:{算法}:{策略序号}:{限流键}`，例如 `edgion:rl:default/api-limiter/#0:sw:0:11:192.168.1.1_9:apikey123` —— 配置多个 key 时，限流键的每一段都带 `<字节长度>:` 前缀，保证不同的取值元组不会落进同一个桶。只配置一个 key 时原样使用（`edgion:rl:default/api-limiter/#0:sw:0:192.168.1.1`）。

`{pluginScope}` 标识拥有这个桶的插件条目：插件所在 EdgionPlugins 资源的 `<namespace>/<name>/<alias>`，条目未配 `alias` 时为 `<namespace>/<name>/#<下标>`。同一份资源在每个 Gateway 实例上推导出相同的字符串，集群级计数不受影响；但两个互不相关的 EdgionPlugins 资源再也不会落进同一批桶，哪怕它们的 `keyPrefix` 和 `key` 来源完全相同。没有它，控制了 A 插件某个 key 来源的客户端可以构造出与 B 插件复合键相同的取值；更常见的是，两个用默认配置的插件会悄悄共用同一套按 IP 的桶。重命名资源或 `alias`、或调换未配 alias 的条目顺序，都会让插件切到新命名空间、计数从零开始；配上 `alias` 即可让作用域不随条目顺序变化。alias 在同一 stage 数组内必须唯一：重复会让资源变为 `Accepted=False`，同时 Gateway 会丢弃重复的那个 alias、让该条目回落到 `#<下标>` 作用域——所以重复 alias 不可能把两个条目并进同一个桶。

引用**同一个** EdgionPlugins 资源的多条路由共享同一个限流器——这正是跨路由共享限流的实现方式，也与内存版 `RateLimitLocal`（计数器同样按资源共享）一致。两个**不同**的资源在构造上已经不可能共享桶：想跨路由共享配额，请让它们指向同一个资源，而不是复制一份配置。

不写 `key` 时套用默认值 `[{type: remoteIp}]`。显式写 `key: []` 表示「单一全局桶」：所有请求组合出同一个空键，仍然会被限流——它不算 key 提取失败，因此 `onKeyMiss` / `defaultKey` 对它不生效。

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
          - type: remoteIp
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

---

## 常见问题

### Q1：`redisRef` 格式是什么？

**A**：格式为 `"namespace/name"`，对应一个 `LinkSys` 类型为 Redis 的资源。例如 `"default/shared-redis"`。字符串中只能包含字母数字、`-`、`_`，不能有特殊字符或斜杠以外的分隔符。

### Q2：`interval` 最小可以设多小？

**A**：最小 100 ms。小于 100 ms 的 interval 会在校验阶段报错，原因是过小的窗口会频繁写入 Redis，可能压垮 Redis 集群。对于极高频率限流，建议使用 [RateLimitLocal 插件](rate-limit-local.md)（进程内，纳秒级）。

### Q3：多条策略是"OR"还是"AND"关系？

**A**：是 **AND** 关系——所有策略必须同时通过，请求才被放行。任意一条策略触发限流，请求即被拒绝。

### Q4：`keyPrefix` 为什么必须以 `:` 结尾？

**A**：这是 Redis key 命名规范，用于在 Redis 中清晰分隔命名空间。默认值 `"edgion:rl:"` 已符合规范。如果自定义前缀忘记加 `:`，校验时会报错。

### Q5：RateLimitRedis 和 RateLimitLocal 可以同时挂载吗？

**A**：可以。两个插件独立工作，请求必须同时通过两者才能到达后端。常见组合：RateLimitLocal 在前做粗粒度防护（本地快速拦截流量洪峰），RateLimitRedis 在后做精确集群配额管理。

---

## 相关文档

- [RateLimitLocal 插件](rate-limit-local.md) — 进程内近似限流（高性能、低延迟）
- [IpRestriction 插件](ip-restriction.md) — 基于 IP 的访问控制
- [KeyAuth 插件](key-auth.md) — API Key 认证
- [JwtAuth 插件](jwt-auth.md) — JWT 认证
## 本地额度与 Redis 补充准入

在同一 `EdgionPlugins.requestPlugins` 数组内，把 `RateLimitLocal` 放在 Redis
条目前面。本地额度内的请求直接继续，不调用 Redis；本地超额后，再由 Redis
补充额度决定放行或拒绝。以下示例需要已有 `default/redis` Redis LinkSys：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: local-with-overflow
  namespace: default
spec:
  requestPlugins:
    - alias: local-budget
      type: RateLimitLocal
      config:
        rate: 50
        interval: "1s"
        scope: Instance
        key: []
        onLimitExceeded: Continue
        showLimitHeaders: false
    - alias: overflow-budget
      type: RateLimitRedis
      conditions:
        run:
          allOf:
            - type: rateLimitExceeded
              pluginAlias: local-budget
      config:
        redisRef: default/redis
        key: []
        policies:
          - algorithm: TokenBucket
            rate: 100
            interval: "1s"
        onRedisFailure: Deny
        requestTimeout: "100ms"
```

`key: []` 表示聚合额度；按用户或 IP 限流时，在两个条目中显式配置 key。
如果中间插件改变 key 的输入，Redis 会使用改变后的值。

`scope: Instance` 下每个副本都有独立本地额度，使用相同插件条目及策略身份的
实例共享 Redis 补充额度。增加副本会增加本地额度，因此这里不是固定的全局
150 请求上限。现有 `scope: Cluster` 会根据实例数量和偏斜容忍度计算各进程阈值，
但仍是本地计数，不会改为 Redis 记账。本地 CMS 也会计入超额尝试；示例 Redis
令牌桶容量为 100、每秒补充 100，并非任意一秒内严格最多 100 次。

示例显式选择 `onRedisFailure: Deny`。现有默认值 `Allow` 不变，Redis 故障时会
允许超额请求。超时可能发生在 Redis 已扣减额度之后，不会自动重试或退款。

示例关闭本地限流响应头：本地直接放行时不报告 Redis 额度，执行 Redis 时才报告
其自身额度，不生成合并剩余额度。额外的有效条件或禁用 Redis 条目可以主动跳过
补充检查；`Continue` 不会强制后续限流器执行。参见[条件引用规则](./conditions.md#ratelimitexceeded)。
