# Stream Plugins

> **🔌 Edgion 扩展**
>
> Stream 插件是 `EdgionStreamPlugins` CRD 提供的连接级（L4）插件；它们在 HTTP 解析之前运行，不属于标准 Gateway API。

Stream 插件在转发任何 payload 之前，对原始 TCP/TLS 连接或 UDP 会话应用访问控制。Gateway 级附件在基于 Pingora `Service` 的监听器上运行于 `ConnectionFilter` 阶段；UDPRoute 附件则由 UDP 代理在接纳新的来源地址会话时执行。

## GatewayClass 执行策略

`EdgionGatewayConfig.spec.pluginPolicy` 决定绑定 GatewayClass 下的 `stream/`
和 `tlsRoute/` 插件能否执行。`block` 拒绝适用的新连接或会话，`bypass` 跳过该条目。
策略修改无需重启监听器，对新的准入生效；已建立的 TCP/TLS 连接和 UDP 会话保留原有行为。

共享监听器在路由之前要求所有服务中的 Gateway 具有等效的连接策略视图；视图不兼容时拒绝新连接。
路由准入使用实际匹配的 Gateway 所属 class。配置示例及策略缺失、无效时的处理见
[插件策略指南](../../ops-guide/plugin-policy.md)。

## 附加方式

Stream 插件**不**内联在路由中。你先定义一个 `EdgionStreamPlugins` 资源，然后通过路由上的 `edgion.io/edgion-stream-plugins` 注解引用它。注解的值为 `namespace/name`（若插件与路由位于同一命名空间，则只写 `name`）。

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/tcp-ip-restriction-deny"
```

同一注解适用于 `TCPRoute`、`TLSRoute`、`UDPRoute` 以及 Gateway 监听器级别，但不同协议的附件生命周期不同。在 `UDPRoute` 上，由于 UDP 没有 TLS 握手，只会在接纳新会话时执行连接上下文运行时（`spec.plugins`）。Gateway 级附件不覆盖 UDP 监听器，因为 UDP 监听器不使用 Pingora `Service` 或监听器 `ConnectionFilter`；要保护 UDP 会话接入，应把资源直接附加到 `UDPRoute`。TLSRoute 只执行 `spec.tlsRoutePlugins`；引用非空的 Stage-1-only 资源会拒绝连接。详见 [TLSRoute 阶段契约](../../dev-guide/stream-plugin-development.md#tlsroute-阶段契约)。

## 支持的插件

| 类型 | 配置对象 | 说明 |
|------|----------|------|
| `IpRestriction` | IpList 引用 | 通过有序的 allowRefs/denyRefs 引用中心 IpList，分组和 CIDR 仅写在 IpList 中。 |
| `GlobalConnectionIpRestriction` | 内联配置 + 可选 `activeProfileRef` | 命名 `profiles` 在插件中仅保存 IpList 引用和动作；可选的 `Selector` EdgionConfigData 用于热切换**当前激活哪个 profile**。由 Center 管理并同步到各 Controller。 |
| `ConnectionRateLimit` | `config` 对象 | 本地监听器保护与可选的 Redis 共享限额；Gateway 挂载在 TLS 前执行，UDPRoute 挂载作用于新会话准入。详见下文 [ConnectionRateLimit 小节](#connectionratelimit-连接与-udp-会话接入限流)。 |
| `ConnectionLimit` | `config` 对象 | 限制挂载作用域内的**并发**连接数（UDP 为 session 数）。挂载到 Gateway 时，超限 socket 在 PROXY 解析和 TLS 握手之前关闭。详见下文 [ConnectionLimit 小节](#connectionlimit-并发连接上限)。 |

`spec.plugins` 下的每个条目都有可选的 `enable` 标志（默认 `true`）、一个 `type` 和一个 `config` 对象。

### `IpRestriction` 配置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `allowRefs` | Array\<Object\> | 无 | 引用中心化的 `IpList` EdgionConfigData，其 CIDR 合并进白名单。每项为 `{name, namespace?}`；`namespace` 缺省取该资源所在 namespace。被引用 IpList 缺失/禁用时 fail-closed（全拒），并通过 `ResolvedRefs=False` 暴露。 |
| `denyRefs` | Array\<Object\> | 无 | 同 `allowRefs`，但合并进黑名单。被引用 IpList 缺失/禁用时少拦（同样以 `ResolvedRefs=False` 暴露）。 |
| `defaultAction` | Enum | `allow` | 没有规则匹配时的动作。允许值：`allow` \| `deny`。 |

`allowRefs`/`denyRefs` 至少一侧非空。旧 allow/deny 键出现即无效，包括 null 和空数组。

#### 分组子字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 分组名称，在插件内唯一（仅用于管理标记）。 |
| `cidrs` | Array\<String\> | **必填** | CIDR 字符串或裸 IP（裸 IP 会转为 `/32` 或 `/128`）。 |
| `description` | String | 无 | 可选的可读说明。 |

求值顺序：先有序 `denyRefs`，再有序 `allowRefs`，最后 `defaultAction`。对于原始 TCP，被拒绝的连接直接关闭（L4 没有协议级响应）。

### `GlobalConnectionIpRestriction` 配置

该插件保存命名的引用型策略（由 git 管理）。IpList 内容可独立热更新；另一个输入是
*当前激活哪个 profile*，由通过 `activeProfileRef` 引用的可选 `Selector`
EdgionConfigData 提供。

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable` | Boolean | **必填** | 总开关。`false` 时禁用插件（放行所有连接）。 |
| `activeProfile` | String | **必填** | 默认激活的 profile 名；必须存在于 `profiles` 中。 |
| `profiles` | Map\<String, Object\> | **必填** | 命名规则集。每个 profile 为 `{ defaultAction, allowRefs?, denyRefs? }`（分组结构同 `IpRestriction`）。每个 profile 也可通过 per-profile 的 `allowRefs`/`denyRefs` 引用中心 IpList（与 `activeProfileRef` 不同——后者是选择哪个 profile 生效，而非引用 IpList）。 |
| `description` | String | 无 | 可选的可读说明。 |
| `activeProfileRef.name` | String | 无 | 可选。一个 `Selector` EdgionConfigData 的名称，用于覆盖当前激活的 profile。 |
| `activeProfileRef.namespace` | String | 父 ESP 命名空间 | Selector 的命名空间；省略时继承父 `EdgionStreamPlugins` 的命名空间。 |

被引用的 `Selector` 只携带 `active: <profileName>`（外加可选的 `description`）。
连接时，若 `active` 命名了插件 `profiles` 中的某个 profile，则生效该 profile；
否则——selector 缺失、被禁用、类型不匹配、`active: null`、或名称不存在——插件回退到
内联的 `activeProfile`（**fail-to-base**）。selector 只能在 git 管理的 profile 之间
切换，永远无法注入规则或禁用插件。

## 快速开始（引用型 IpRestriction）

### 第一步：定义插件

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-tcp-ip-restriction-allow-allow-1
  namespace: edgion-test
spec:
  data:
    type: IpList
    config:
      items:
      - name: allowed-clients
        cidrs:
        - 10.0.0.0/8
---
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tcp-ip-restriction-allow
  namespace: edgion-test
spec:
  plugins:
  - type: IpRestriction
    config:
      defaultAction: deny
      allowRefs:
      - name: example-tcp-ip-restriction-allow-allow-1
        namespace: edgion-test
```

### 第二步：在路由中引用

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TCPRoute
metadata:
  name: redis-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/tcp-ip-restriction-allow"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

### 第三步：应用

```bash
kubectl apply -f stream-plugins.yaml
kubectl apply -f tcp-route.yaml
```

现在只有 `10.0.0.0/8` 内的客户端可以访问 Redis 服务。

## Center 管理的激活 profile（GlobalConnectionIpRestriction）

将命名的引用型策略写在插件中，将 CIDR 分组写入 IpList（由 git 管理、经过评审）。然后让 Center
同步一个很小的 `Selector` EdgionConfigData，用来在多个 Controller 之间选择
**当前激活哪个 profile**——而完全不需要下发规则本身。

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-gcir-stream-allow-2
  namespace: edgion-test
spec:
  data:
    type: IpList
    config:
      items:
      - name: office
        cidrs:
        - 10.0.0.0/8
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-gcir-stream-allow-3
  namespace: edgion-test
spec:
  data:
    type: IpList
    config:
      items:
      - name: any
        cidrs:
        - 0.0.0.0/0
---
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: gcir-stream
  namespace: edgion-test
spec:
  plugins:
  - type: GlobalConnectionIpRestriction
    config:
      enable: true
      activeProfile: locked
      profiles:
        locked:
          defaultAction: deny
          allowRefs:
          - name: example-gcir-stream-allow-2
            namespace: edgion-test
        open:
          defaultAction: deny
          allowRefs:
          - name: example-gcir-stream-allow-3
            namespace: edgion-test
      activeProfileRef:
        name: gcir-active
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: gcir-active
  namespace: edgion-test
spec:
  enable: true
  data:
    type: Selector
    config:
      active: open
      description: 临时放宽访问
---
apiVersion: gateway.networking.k8s.io/v1
kind: TCPRoute
metadata:
  name: gcir-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: edgion-test/gcir-stream
spec:
  parentRefs:
  - name: stream-gateway
    sectionName: tcp-redis
  rules:
  - backendRefs:
    - name: redis-primary
      port: 6379
```

要放宽或收紧访问，把 `active` 切换到另一个**预先授权**的 profile；selector 无法注入
新规则、也无法一键全开——那必须是 git 中经过评审的 profile。若 Selector 被删除或
指向不存在的 profile，插件回退到内联的 `activeProfile`（`locked`）。

## 跨命名空间引用

使用 `namespace/name` 格式指向其他命名空间的插件：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TCPRoute
metadata:
  name: app-route
  namespace: app
  annotations:
    edgion.io/edgion-stream-plugins: "security/common-policy"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

## 边缘情况

### UDPRoute 上的同一注解

`UDPRoute` 接受相同的注解。只有 Stage-1（`spec.plugins`）会执行；IP 检查在每个会话的第一个数据报上求值（fail-close），因此被拒绝的客户端永远无法建立会话。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: UDPRoute
metadata:
  name: udp-route-with-plugins
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/udp-ip-restriction-deny"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: udp-plugins
  rules:
    - backendRefs:
        - name: udp-echo-backend
          port: 9300
```

### 在允许范围内拒绝单个主机

`deny` 先于 `allow` 求值，因此你可以在允许宽泛 CIDR 的同时排除某个地址。分组名称是任意标签，仅用于组织管理。

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-carve-out-allow-4
  namespace: edgion-test
spec:
  data:
    type: IpList
    config:
      items:
      - name: internal
        cidrs:
        - 10.0.0.0/8
---
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-carve-out-deny-5
  namespace: edgion-test
spec:
  data:
    type: IpList
    config:
      items:
      - name: quarantine
        cidrs:
        - 10.6.6.6/32
---
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: carve-out
  namespace: edgion-test
spec:
  plugins:
  - type: IpRestriction
    config:
      defaultAction: deny
      allowRefs:
      - name: example-carve-out-allow-4
        namespace: edgion-test
      denyRefs:
      - name: example-carve-out-deny-5
        namespace: edgion-test
```

## ConnectionRateLimit: 连接与 UDP 会话接入限流

`ConnectionRateLimit` 支持组合使用本地监听器保护与 Redis 共享限额。挂载到 Gateway 监听器时，检查在 TLS 握手**之前**执行；挂载到 TCPRoute 时，在路由匹配后执行；挂载到 UDPRoute 时，在首个数据报尝试创建新的来源地址会话时执行。它不会统计已建立会话的数据报，Gateway 级附件也不覆盖 UDP 监听器。

### 选择本地与共享限额

常规连接突发保护建议配置 `perListener`，保持 `redisDistributed: false`，使用不依赖 Redis 的本地令牌桶。需要共享限额时，同一个插件可以组合本地与 Redis 检查：先执行本地 `perListener` 门控，再执行可选的分布式 `perListener` 检查，最后执行可选的 `perSourceIp` 检查。任一步拒绝即停止；设置 `redisDistributed: true` 仍会保留本地门控。

下方组合示例使用本地 `perListener` 保护与 Redis `perSourceIp` 限额。Redis 连接限流按需启用，应根据预期新连接或 UDP 会话准入速率与依赖延迟评估容量。HTTP 业务配额优先使用请求级限流：长连接或多路复用连接的数量不能代表请求数量。

### 为什么 `redisTimeout` 被硬上限封顶为 1s

挂载到 Gateway 监听器时，每个 Pingora accept 任务在等待过滤器期间持有一个已接受的 socket。Pingora 默认每个监听 FD 只有一个 accept 任务，等待 Redis 时后续连接会在内核 backlog 中排队。这限制了每个 accept 任务中正在过滤的 socket 数量，但可能降低新连接接入吞吐。增加 Redis 连接池容量或进程 FD 上限不能消除这种串行等待；它不会直接阻塞已经建立的连接。

`redisTimeout` 默认 `100ms`，硬上限 `1s`，作用于**每个 Redis 维度**；多个维度或插件条目的等待时间可以叠加。Redis 客户端缺失或已标记不健康时立即执行失败策略；其他情况下，即使 fail-open 也可能先等待调用超时。本地过滤能降低 Redis 压力，但不能消除已通过本地检查的连接所遇到的等待。Pingora 的 `listener_tasks_per_fd` 控制 accept 并行度，与进程 FD 上限不同，也不是 `ConnectionRateLimit` 配置字段。提高并行度还会增加并发 Redis 检查，需要结合实际负载验证。握手超时在过滤结束后开始，不覆盖这段等待。

> **`perSourceIp` 没有本地兜底。** 与 `perListener` 维度（即使 Redis 不可达，本地令牌桶仍然有效）不同，`perSourceIp` 没有进程内回退。进入该维度的连接或 UDP 会话准入尝试需要 Redis 决策或执行配置的失败策略。如果需要 fail-close 行为，请显式设置 `onRedisFailure`。

### 配置字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `redisRef` | String | 无 | 配置 `perSourceIp` 或分布式 `perListener` 时必填；Redis LinkSys 引用，格式 `"namespace/name"`。 |
| `algorithm` | Enum | `SlidingWindow` | 限流算法：`SlidingWindow` \| `FixedWindow` \| `TokenBucket`。 |
| `redisTimeout` | String | `"100ms"` | 每个 Redis 维度的调用耗时上限，硬上限 `1s`。超时后执行 `onRedisFailure`。 |
| `perListener` | Object | 无 | 按监听器限流，带本地令牌桶兜底（即使 Redis 不可达也可用）。 |
| `perSourceIp` | Object | 无 | 按源 IP 限流。需要 Redis——无本地兜底。见上文提示。 |

**桶的作用域**：计数器（Redis key 与本地令牌桶都是）按拥有它的插件条目命名空间隔离，形式为 EdgionStreamPlugins 资源的 `<namespace>/<name>/#<条目下标>`。同一个监听端口上可能同时有两个 `ConnectionRateLimit` 生效（监听器级与 TCPRoute 级各挂一份、或同一资源里写了两条）；没有这层作用域，它们会共用一个计数器却各按自己的 `rate` 判断，`interval` 不同还会互相破坏窗口轮转。各 Gateway 实例从同一份资源推导出相同作用域，集群级计数不受影响，热重载也不会丢桶。调换条目顺序会改变身份：移到曾经使用过的位置，可能继承该位置保留的本地预算。需要预算连续性时，请保持名称和条目位置稳定。

**热重载时的本地预算**：每个监听端口在连接或 UDP 会话准入尝试首次使用新配置执行到该条目时采用新限额。切换前先按旧速率结算经过的时间；提高 `rate` 只增加容量差额，降低时将余额截断到新容量。只修改 Redis 设置（包括 `keyPrefix`）不会重置本地预算。在同一 Gateway 进程内，禁用或删除后再启用或重建，同一条目和端口仍保留本地状态；空闲时间按最后生效的本地速率累计。

热重载期间，已使用旧配置的准入操作可以继续完成。端口采用新本地限额后，这些操作消耗更新后的本地预算，但仍使用原来的 Redis 设置。两者是顺序检查，不是本地与 Redis 的原子事务；后续 Redis 拒绝不会退还本地令牌。在监听器和 TCPRoute 两层重复引用同一资源，仍会在两层分别执行并消耗该条目的预算。

#### `perSourceIp` / `perListener` 子字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `rate` | Integer | **必填** | 每个时间窗口内最大的 TCP/TLS 新连接数或 UDP 会话准入尝试数，范围为 1–1,000,000；不统计已建立 UDP 会话的数据报。 |
| `interval` | String | **必填** | 整毫秒时间窗口，范围为 100ms–2,147,483,647ms，例如 `"1s"`、`"1m"`。 |
| `redisDistributed` | Boolean | `false` | （仅 `perListener`）为 `true` 时，在本地门控之外再通过 Redis 进行全集群范围的限额。 |
| `onRedisFailure` | Enum | `Allow`（`perSourceIp`）/ `Deny`（`perListener`） | Redis 不可用或 `redisTimeout` 超时时的行为。`Allow` = 放行；`Deny` = 拒绝。 |

### 可观测性

指标 `edgion_connection_rate_limit_redis_failure_total`（标签 `dimension`：`listener` 或 `source_ip`；`cause`：`unavailable` 或 `runtime`）在每次 L4 Redis 失败时递增，无论 `onRedisFailure` 设置为何。这样 fail-open 行为在 Prometheus 中始终可见。

PromQL 告警示例：

```promql
rate(edgion_connection_rate_limit_redis_failure_total{dimension="source_ip"}[5m]) > 0
# L4 限流器正在静默 fail-open
```

### 示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: conn-rate-limit
  namespace: edgion-test
spec:
  plugins:
    - type: ConnectionRateLimit
      config:
        redisRef: "default/shared-redis"
        algorithm: SlidingWindow
        redisTimeout: "200ms"     # 默认 100ms；硬上限 1s
        perListener:
          rate: 10000
          interval: "1s"
          redisDistributed: false
          # onRedisFailure: Deny  # perListener 的默认值
        perSourceIp:
          rate: 50
          interval: "1s"
          # onRedisFailure: Allow # perSourceIp 的默认值
```

像其它 stream 插件一样附加到路由上：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TCPRoute
metadata:
  name: rate-limited-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/conn-rate-limit"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-app
  rules:
    - backendRefs:
        - name: app-service
          port: 8080
```

## ConnectionLimit: 并发连接上限

`ConnectionRateLimit` 限制新连接到达的速度；`ConnectionLimit` 限制**同时**打开的连接数。真正耗尽文件描述符和内存的是长连接（空闲 keep-alive、HTTP/2、WebSocket、SSE、慢客户端），只有并发上限能约束它们。

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: conn-limit
  namespace: edgion-test
spec:
  plugins:
    - type: ConnectionLimit
      config:
        maxConnections: 10000   # 1..=10000000
```

不挂载就不生效。计数对象取决于挂载位置：

| 挂载位置 | 计数对象 | 超限连接何时被关闭 |
|---|---|---|
| Gateway 注解 | 该 Gateway **每个**物理 listener 上的存活 socket（上限按 listener 计，不是按 Gateway）。UDP listener 不执行 Gateway 挂载。 | accept 时，在 PROXY 协议解析和 TLS 握手之前。 |
| TCPRoute 注解 | 该 TCP listener 进入路由阶段的连接，整个转发期间都占用名额。 | 路由匹配之后、连接后端之前。 |
| UDPRoute 注解 | 该 UDP listener 的 session，直到 session 被回收。 | 新来源地址将要创建 session 时。 |

同一个资源可以同时挂在 Gateway 和 TCPRoute 上，两层各自计数。计数归 listener 所有，listener 只在启动时创建一次，因此配置变更不会清零或重复计数；调低 `maxConnections` 不会断开已有连接，只会在计数降到上限以下之前拒绝新连接。

各挂载点之间的计数互不共享：

- 路由阶段的计数属于 **listener**，而不是某一条路由。它包含该 listener 路由阶段中的所有连接，包括配置变更前由旧路由建立、仍未结束的转发。新路由如果上限更低，会在这些转发结束前拒绝新连接。
- 挂在多个 listener 上的 TCPRoute 或 UDPRoute，在每个 listener 上分别计算上限。
- 同一个 `EdgionStreamPlugins` 资源被多个 Gateway 或路由引用时，每个挂载点各自计数；资源本身不保存共享状态。

注意：

- 上限统计的是所有客户端的总数，不是按客户端限制。单个来源就可能占满上限并拒绝其他所有客户端。目前还没有按客户端的并发限制；按来源的速率预算请使用 `ConnectionRateLimit`。
- 把 `ConnectionLimit` 放在 `plugins` 的**第一位**，超限连接就不会再执行后面更昂贵的条目（例如依赖 Redis 的 `ConnectionRateLimit`）。如果放在 `ConnectionRateLimit` 之后，超限客户端的重连会先消耗速率令牌（包括按来源 IP 的预算），并等待 Redis 往返后才被拒绝；挂在 Gateway 上时，这个等待发生在 listener 的串行 accept 循环中，会推迟后续连接。在前面条目中等待的连接已经被计数。
- 上限统计的是下游 socket。一个 HTTP/2 连接可以承载多个 stream 和上游连接；基于内存的保护是另一项能力。
- 指向已达上限 listener 的 L4 负载均衡或 kubelet TCP 探活会失败。Admin、探活和 metrics 端口不计入。
- 回收 UDP 会话时，网关会唤醒该会话的上游接收任务。最后一个持有该会话的任务释放引用后，名额即可重新使用。
- UDP 来源地址可以被伪造：每个伪造来源的一个数据报就会创建一个 session，并占用名额直到 session 超时（60 秒）。因此挂在 UDPRoute 上的 `ConnectionLimit` 能被少量伪造数据报占满，导致新的合法客户端被拒绝。它限制资源占用，不能防御伪造源洪泛。
- `pluginPolicy` 照常生效：`Block` `stream/ConnectionLimit` 会让该挂载 fail-closed，`Bypass` 会去掉上限。
- 拒绝计入 `edgion_downstream_connections_rejected_total{reason="listener"|"route"}`（UDP 按每个被拒绝的新 session 准入尝试计数，也就是每个数据报）；TCP/UDP access log 中显示通用的 `stream plugin policy denied`。

如需跨所有 listener 的进程级上限，请在 gateway 配置中设置 [`server.max_downstream_connections`](../../ops-guide/configuration/gateway-config.md)。

## 故障排除

### 插件未生效

1. 确认插件资源存在：
   ```bash
   kubectl get edgionstreamplugins -A
   ```
2. 检查注解值（必须为 `name` 或 `namespace/name`）：
   ```bash
   kubectl get tcproute <name> -o yaml | grep -A2 annotations
   ```
3. 查看 Gateway 日志：
   ```bash
   kubectl logs <gateway-pod> | grep -i "stream plugin"
   ```

### 连接被意外拒绝

`deny` 优先于 `allow`，`defaultAction: deny` 拒绝所有不在列表中的请求。
请验证客户端 IP 是否在某个 `allow` 分组的 `cidrs` 范围内，且未被 `deny` 分组捕获。
