# 连接器（LinkSys）

> **🔌 Edgion 扩展**
>
> 连接器由 `LinkSys` CRD（`edgion.io/v1`）定义，用于集成外部系统，不属于标准 Gateway API 规范。

## 概述

**连接器**是指向外部系统（Redis、Etcd、Elasticsearch、HTTP webhook 或 Kafka）的可复用命名句柄。您只需声明一次 `LinkSys` 资源，通过 Kubernetes Secret 挂载凭据，即可在任何需要该后端的功能中引用它。这样可以将连接详情和凭据集中管理，避免在每个插件和策略中重复配置。

一个 `LinkSys` 对象对应一个连接器。顶层字段 `spec.type` 选择后端类型，`spec.config` 承载该后端的配置。多个 Edgion 功能通过 `namespace/name` 引用连接器：

- 分布式（集群级）限流通过 `namespace/name` 引用 `redis` 连接器——详见 [Redis 限流](../../user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md)。
- 访问日志与审计日志的输出目标可引用 `elasticsearch`、`redis` 或 `webhook` 连接器——详见 [Access Log](../observability/access-log.md)。
- 通过 webhook 进行密钥解析等外部数据查询的插件可引用 `webhook` 连接器。

`LinkSys` 是命名空间作用域的资源，短名称为 `lsys`。`kubectl get linksys` 可一览 `Accepted` 和 `ResolvedRefs` 状态条件。

```text
                 reference by namespace/name
  ┌──────────────┐        ┌──────────────────┐        ┌────────────────┐
  │ EdgionPlugins│───────▶│  LinkSys (lsys)  │───────▶│ External system│
  │ AccessLog    │        │  type + config   │        │ Redis / Etcd / │
  │ ...          │        │  + secretRef     │        │ ES / Webhook   │
  └──────────────┘        └────────┬─────────┘        └────────────────┘
                                   │ secretRef
                                   ▼
                          ┌──────────────────┐
                          │ Secret (creds /  │
                          │ TLS private key) │
                          └──────────────────┘
```

⚠️ **所有凭据（密码、令牌、API 密钥、HMAC 密钥、客户端私钥）必须通过 Secret 引用传入。不允许在 CRD 中内联敏感字段**——认证和 TLS 结构体开启了 `deny_unknown_fields`，直接在 YAML 中写入 `username`、`password`、`bearerToken`、`apiKeyId`、`hmacKey` 或 `clientKey` 会导致校验失败，而非被静默忽略。

## 凭据模型

所有连接器共享相同的凭据约定。

### Secret 引用

凭据通过 `secretRef` 对象引用：

```yaml
secretRef:
  name: redis-auth           # 必填
  namespace: credentials     # 可选，默认为 LinkSys 所在命名空间
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 读取凭据的 Secret 名称。 |
| `namespace` | String | LinkSys 所在命名空间 | Secret 所在命名空间。跨命名空间引用需要 `ReferenceGrant`。 |
| `group` | String | `""`（core） | 被引用对象的 API group，留空即可。 |
| `kind` | String | `Secret` | 被引用对象的类型，留空即可。 |

当 `namespace` 与 LinkSys 资源所在命名空间不同时（跨命名空间引用），需要在目标命名空间创建允许 `Secret` 引用的 Gateway API `ReferenceGrant`。否则状态报告 `ResolvedRefs=False/RefNotPermitted`。详见[跨命名空间引用](./reference-grant.md)。

### Secret 键名约定

Secret 键名遵循固定的已知约定，不支持通过 `usernameKey` / `passwordKey` 字段自定义。

| 认证类型 | 推荐 `Secret.type` | 必需键 | 可选键 |
|---------|---------------------|--------|--------|
| basic（Redis / Etcd / ES） | `kubernetes.io/basic-auth` 或 `Opaque` | `password` | `username` |
| apiKey（Elasticsearch） | `Opaque` | `apiKeyId`、`apiKeySecret` | — |
| bearer（Elasticsearch） | `Opaque` | `bearerToken` | — |
| TLS（所有后端） | `kubernetes.io/tls` 或 `Opaque` | `tls.crt`、`tls.key`、`ca.crt` 中至少一个 | — |

应用层键名使用 camelCase；TLS 键名遵循 Kubernetes `kubernetes.io/tls` 标准。

> **Webhook 凭据有所不同。** Webhook 连接器不再有类型化的 `auth` 块。改为将任意凭据作为自定义请求头（或 arg/cookie）注入，并从 Secret 中读取。Secret 的键名由**运营者自行选择**（即 `key:` 中填写的内容），可选的 `prefix:` 用于在前面拼接 `Bearer ` / `Basic ` 等。详见 [Webhook 连接器 → 自定义字段来源](#自定义字段来源)。

### TLS

每个后端均接受相同的 `tls` 块用于保护连接。

```yaml
tls:
  enabled: true
  verify: true                 # 验证对端证书（默认 true）；false 由运维人员负责，启动时记录 WARN
  caSource: SecretRef          # System | SecretRef | Inline
  caSecretRef:
    name: my-ca-bundle         # 包含 ca.crt 的 Secret（caSource: SecretRef 时使用）
  # caCert: |                  # 内联 PEM CA 包（caSource: Inline 时使用）
  #   -----BEGIN CERTIFICATE-----
  #   <base64-encoded-cert>
  #   -----END CERTIFICATE-----
  clientSecretRef:
    name: my-client-tls        # 包含 tls.crt + tls.key 的 Secret，用于客户端 mTLS
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | Boolean | `false` | 为连接启用 TLS。对于已隐含 TLS 的 URL（`https://`、`rediss://`），此字段仅添加自定义 CA / 客户端证书处理。 |
| `verify` | Boolean | `true` | 验证对端 TLS 证书。`verify: false` 由运维人员负责，启动时记录 WARN。Redis 和 Etcd 始终验证证书（SDK 无跳过验证 API，启动时即失败）。 |
| `caSource` | Enum | — | 信任锚来源：`System`（内置 webpki 根证书）、`SecretRef`（来自 `caSecretRef` 的私有 CA）或 `Inline`（`caCert` 中的 PEM）。自定义 CA **替换**公共根证书。 |
| `caSecretRef` | Object | none | 包含私有 CA 包（`ca.crt` 键）的 Kubernetes Secret。`caSource: SecretRef` 时必填。 |
| `caCert` | String | none | 内联 PEM CA 包。`caSource: Inline` 时使用。 |
| `clientSecretRef` | Object | none | 包含客户端 mTLS 身份（`tls.crt` / `tls.key` 键）的 Kubernetes Secret。 |

客户端 mTLS 身份（证书 + 私钥）必须通过 `clientSecretRef` 提供。

### 可观测性

Redis、Etcd 和 Elasticsearch 支持共享的 `observability` 块。

```yaml
observability:
  logging:
    enabled: true
    level: info
    slowLogThreshold: 200
  metrics:
    enabled: true
    labels:
      env: production
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `logging.enabled` | Boolean | `false` | 启用每个连接器的日志记录。 |
| `logging.level` | String | none | 日志级别。示例：`info`、`debug`、`warn`、`error`。 |
| `logging.slowLogThreshold` | Integer | none | 慢操作日志阈值（毫秒）。 |
| `metrics.enabled` | Boolean | `false` | 启用每个连接器的指标。 |
| `metrics.labels` | Object | none | 附加到指标的额外键值标签。 |

## Redis 连接器

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: cache-redis
  namespace: production
spec:
  type: redis
  config:
    endpoints:
      - "redis://redis.production.svc:6379"
    db: 0
    auth:
      secretRef:
        name: redis-auth
    pool:
      size: 16
      minIdle: 2
    timeout:
      connect: 3000
      read: 2000
      write: 2000
```

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `endpoints` | Array\<String\> | **必填** | Redis 端点，至少一个。示例：`"redis://redis:6379"` 或 `"rediss://..."` 表示 TLS。 |
| `db` | Integer | none | 数据库索引（0–15）。 |
| `auth` | Object | none | 认证配置。存在时 `auth.secretRef` 必填。Secret 需提供 `password`；`username` 可选（Redis 6+ ACL）。 |
| `tls` | Object | none | TLS 配置；详见 [TLS](#tls)。 |
| `observability` | Object | none | 日志和指标；详见[可观测性](#可观测性)。 |

### 连接池与超时

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `pool.size` | Integer | none | 连接池最大大小。 |
| `pool.minIdle` | Integer | none | 保持的最小空闲连接数。 |
| `timeout.connect` | Integer | none | 连接超时（毫秒）。 |
| `timeout.read` | Integer | none | 读超时（毫秒）。 |
| `timeout.write` | Integer | none | 写超时（毫秒）。 |
| `retry.maxRetries` | Integer | none | 最大重试次数。 |
| `retry.backoff.type` | String | **必填**（设置 `backoff` 时） | 退避策略。示例：`exponential`、`linear`、`fixed`。 |
| `retry.backoff.initialDelay` | Integer | none | 初始重试延迟（毫秒）。 |
| `retry.backoff.maxDelay` | Integer | none | 最大重试延迟（毫秒）。 |
| `retry.backoff.multiplier` | Number | none | 指数退避乘数。 |

### 拓扑：standalone、sentinel、cluster

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `topology.mode` | Enum | — （设置 `topology` 时必填） | 部署拓扑。可选值：`standalone` \| `sentinel` \| `cluster`。 |
| `topology.sentinel` | Object | none | Sentinel 配置。**`mode=sentinel` 时必填。** |
| `topology.cluster` | Object | none | 集群配置。**`mode=cluster` 时必填。** |
| `topology.sentinel.masterName` | String | **必填** | Sentinel 监控的主节点名称。 |
| `topology.sentinel.sentinels` | Array\<String\> | **必填** | Sentinel 地址（`host:port`）。 |
| `topology.cluster.readFromReplicas` | Boolean | none | 允许从副本节点读取。 |
| `topology.cluster.maxRedirects` | Integer | none | 最大集群重定向跳数。 |

省略 `topology` 时连接器以 standalone 模式运行。`mode=sentinel` 但缺少 `sentinel` 块（或 `mode=cluster` 但缺少 `cluster` 块）时，状态报告 `Accepted=False/Invalid`。

> **说明**：`topology.sentinel.password`（独立于 Redis 节点认证的 Sentinel 自身密码）字段已被移除——该字段在运行时从未被消费（no-op）。如果现有 CR 的 `topology.sentinel` 下包含 `password:` 字段，Kubernetes API server 在重新 apply 时会静默裁剪该字段，不会导致解析错误。

#### 示例：Sentinel 高可用

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: ha-redis
  namespace: production
spec:
  type: redis
  config:
    endpoints:
      - "redis://sentinel-1:26379"
    auth:
      secretRef:
        name: redis-auth
    topology:
      mode: sentinel
      sentinel:
        masterName: mymaster
        sentinels:
          - "sentinel-1:26379"
          - "sentinel-2:26379"
          - "sentinel-3:26379"
```

#### 示例：集群模式

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: cluster-redis
  namespace: production
spec:
  type: redis
  config:
    endpoints:
      - "redis://redis-node-1:6379"
      - "redis://redis-node-2:6379"
      - "redis://redis-node-3:6379"
    auth:
      secretRef:
        name: redis-auth
    topology:
      mode: cluster
      cluster:
        readFromReplicas: true
        maxRedirects: 6
```

## Etcd 连接器

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: config-etcd
  namespace: production
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd-1.production.svc:2379"
      - "https://etcd-2.production.svc:2379"
    auth:
      secretRef:
        name: etcd-creds
        namespace: credentials
    tls:
      enabled: true
      clientSecretRef:
        name: etcd-client-tls
    namespace: "/my-app/"
```

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `endpoints` | Array\<String\> | **必填** | Etcd 端点，至少一个。示例：`"https://etcd-1:2379"`。 |
| `auth` | Object | none | 认证配置。Secret 通常同时提供 `username` 和 `password`。 |
| `tls` | Object | none | TLS 配置；详见 [TLS](#tls)。Etcd（与 Redis 一样）在任何环境下均拒绝 `verify: false`。 |
| `namespace` | String | none | Etcd v3 键前缀。 |
| `userAgent` | String | none | 发送给 etcd 的 HTTP User-Agent。示例：`"edgion-gateway/v1"`。 |
| `rejectOldCluster` | Boolean | none | 拒绝连接到过时的集群。 |
| `observability` | Object | none | 日志和指标；详见[可观测性](#可观测性)。 |

### 超时与 keepalive

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `timeout.dial` | Integer | none | 拨号超时（毫秒）。 |
| `timeout.request` | Integer | none | 请求超时（毫秒）。 |
| `timeout.keepAlive` | Integer | none | keepalive 超时（毫秒）。 |
| `keepAlive.time` | Integer | none | HTTP/2 keepalive 时间（秒）。 |
| `keepAlive.timeout` | Integer | none | HTTP/2 keepalive 超时（秒）。 |
| `keepAlive.permitWithoutStream` | Boolean | none | 无活跃流时允许 keepalive ping。 |
| `autoSyncInterval` | Integer | none | 端点自动同步间隔（秒），`0` 表示禁用。 |
| `maxCallSendSize` | Integer | none | 最大发送消息大小（字节）。 |
| `maxCallRecvSize` | Integer | none | 最大接收消息大小（字节）。 |

## Elasticsearch 连接器

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: logs-es
  namespace: production
spec:
  type: elasticsearch
  config:
    endpoints:
      - "https://es-1.production.svc:9200"
      - "https://es-2.production.svc:9200"
    auth:
      type: apiKey
      secretRef:
        name: es-api-key
    tls:
      enabled: true
    index:
      prefix: "edgion-logs"
      datePattern: "%Y.%m.%d"
    bulk:
      batchSize: 500
      flushInterval: 5000
```

多个端点以轮询方式进行负载均衡。此连接器也兼容 OpenSearch；请求直接通过 HTTP 发出。

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `endpoints` | Array\<String\> | **必填** | Elasticsearch 端点，至少一个。示例：`"https://es-1:9200"`。 |
| `auth` | Object | none | 认证配置（标签枚举）。存在时 `type` 必填。 |
| `auth.type` | Enum | **必填**（设置 `auth` 时） | 认证方式。可选值：`basic` \| `apiKey` \| `bearer`。 |
| `auth.secretRef` | Object | **必填**（设置 `auth` 时） | 提供所选 `auth.type` 凭据的 Secret。 |
| `tls` | Object | none | TLS 配置；详见 [TLS](#tls)。 |
| `compatibility.vendor` | Enum | `elasticsearch` | 目标厂商。可选值：`elasticsearch` \| `opensearch`。 |
| `observability` | Object | none | 日志和指标；详见[可观测性](#可观测性)。 |

### 连接池与超时

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `timeout.connect` | Integer | none | 连接超时（毫秒）。 |
| `timeout.request` | Integer | none | 请求超时（毫秒）。 |
| `pool.maxIdlePerHost` | Integer | none | 每主机最大空闲连接数。 |
| `pool.idleTimeout` | Integer | none | 空闲连接超时（毫秒）。 |

### 索引命名

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `index.prefix` | String | `"edgion-logs"` | 索引名称前缀。 |
| `index.datePattern` | String | `"%Y.%m.%d"` | 附加到索引名称的 strftime 日期格式。生成的索引如 `edgion-logs-2026.05.29`。 |

### 批量写入

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `bulk.batchSize` | Integer | `500` | 每批次文档数。 |
| `bulk.flushInterval` | Integer | `5000` | 即使批次未满也强制刷新的间隔（毫秒）。 |
| `bulk.maxRetries` | Integer | `3` | 批量写入失败的最大重试次数。 |
| `bulk.backoffMs` | Integer | `1000` | 重试间隔的初始退避（毫秒）。 |
| `bulk.maxBodyBytes` | Integer | `10485760`（10 MB） | 批量请求体最大大小（字节），上限 50 MB。 |
| `bulk.channelSize` | Integer | `10000` | 生产者到批量循环的通道容量，上限 100000。 |

### 失败文档缓存

> **状态：规划中 —— 尚未实现。** 下文的 `failedCache` 配置与示例仅描述设计意图。
> 网关当前**不会**处理 `failedCache` 配置块（未知 CRD 字段会被裁剪，设置后不生效）。
> 在其落地前，请额外注册一个 `localFile` sink 作为兜底，以免 Elasticsearch 故障期间丢日志。
> 跟踪：`edgion-gateway/src/link_sys/readme`。

索引失败时，可以将文档缓存以便后续重试。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `failedCache.type` | Enum | **必填**（设置 `failedCache` 时） | 缓存后端。可选值：`localFile` \| `redis`。 |
| `failedCache.localFile.path` | String | **必填**（`localFile` 时） | 失败文档存储的目录路径。 |
| `failedCache.redis.linkSysRef` | String | **必填**（`redis` 时） | Redis 连接器的 `namespace/name`。 |
| `failedCache.redis.listKey` | String | none | 失败文档的 Redis 列表键名。 |
| `failedCache.redis.maxEntries` | Integer | none | 列表中保留的最大条目数。 |

#### 示例：带 Redis 失败缓存的 Elasticsearch

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: logs-es
  namespace: production
spec:
  type: elasticsearch
  config:
    endpoints:
      - "https://es-1.production.svc:9200"
    auth:
      type: apiKey
      secretRef:
        name: es-api-key
    tls:
      enabled: true
    index:
      prefix: "edgion-logs"
      datePattern: "%Y.%m.%d"
    failedCache:
      type: redis
      redis:
        linkSysRef: "production/cache-redis"
        listKey: "es-failed"
        maxEntries: 100000
```

## Webhook 连接器

Webhook 连接器让插件（ctx-set、key-auth、hmac-auth 等）调用外部 HTTP 服务，用于用户解析、权限校验、上下文增强、审计日志等查询。该连接器是出站请求的**唯一事实来源**：它声明后端（`target.backendRef`），并通过 `request` 块精确定义发送的路径、方法、查询参数、请求头和 Cookie。消费方插件不能再静默地改变请求形状——它们只能向 webhook 作者声明的命名槽位填值，可选地覆盖路径（在被允许时），以及解析响应。出站字段遵循**白名单**模型：除非 webhook 明确声明，否则不转发任何内容，因此上游某个行为异常的插件无法泄露运营者从未打算暴露的请求头。

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver
  namespace: production
spec:
  type: webhook
  config:
    target:
      backendRef:
        kind: Service
        name: key-service
        namespace: production
        port: 8080
    request:
      method:
        template: POST
      headers:
        origin:
          - name: X-Tenant-Id
        custom:
          - name: X-Source
            value: edgion
          - name: Authorization
            secret:
              secretRef:
                name: webhook-token
              key: token
              prefix: "Bearer "
    timeoutMs: 3000
    successStatusCodes: [200]
    maxResponseBytes: 2048
```

### 目标

每个 webhook 通过单一的 `backendRef` 引用其后端，与路由已采用的 `BackendObjectReference` 形状相同。不再有 `type: uri` / `type: service` 的选择——`backendRef` 是唯一的目标形式。

```yaml
target:
  backendRef:
    group: ""              # 可选；默认为 core API 组
    kind: Service          # Service | ServiceClusterIp | ServiceEndpointSlice（留空默认为 Service）
    name: webhook-backend
    namespace: production  # 可选；默认为 LinkSys 所在命名空间
    port: 8080             # 必填
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `target.backendRef.group` | String | core（`""`） | 被引用对象的 API 组。 |
| `target.backendRef.kind` | String | `Service` | 目标类型。可选：`Service`、`ServiceClusterIp`、`ServiceEndpointSlice` 或留空（默认 `Service`）。 |
| `target.backendRef.name` | String | **必填** | Kubernetes Service 名称。 |
| `target.backendRef.namespace` | String | LinkSys 所在命名空间 | Service 所在命名空间。 |
| `target.backendRef.port` | Integer | **必填** | Service 端口，必须大于 0。缺少端口会在准入时以 `Accepted=False` 拒绝。 |

按 kind 的解析方式：

- `Service`（默认，留空/未设置亦同）：网关在调用时从 Kubernetes EndpointSlice 存储解析就绪端点。
- `ServiceEndpointSlice`：同样的 EndpointSlice 解析的显式形式。
- `ServiceClusterIp`：通过 Service ClusterIP 的 DNS 名称解析（kube-proxy）。

`kind: ExternalName` 和 `ServiceImport` **不**支持作为 webhook 目标，会在准入时以带说明的 `Accepted=False` 消息拒绝。要访问外部端点，请创建一个代理 Service（例如 `LoadBalancer` 类型的 Service，或由 ingress controller 前置的 `ClusterIP` Service）指向外部主机，然后按名称引用该 Service。详见[从旧 schema 迁移](#从旧-schema-迁移)。

### 出站请求

`request` 块声明出站请求的形状。所有五个子字段均为可选：

- **`path`** / **`method`**——单值。省略任一项则复用来自传入客户端请求的**冻结**值。设置时，其值为一个 `template`（字面字符串或 `${type:name}` 表达式）。
- **`args`** / **`headers`** / **`cookies`**——白名单字段组，每组拆分为 `origin` 列表和 `custom` 列表。省略某组则不转发该类的任何字段。Cookie 作为单个 `Cookie` 请求头转发。

```yaml
request:
  path:
    template: "/resolve/${ctx:action}"
    allowOverride: true        # 消费方可在调用时传入 pathOverride
  method:
    template: POST

  args:
    origin:
      - name: tenant_id                      # 缺失 → 省略（默认）
      - name: trace_id
        presence: required                   # 缺失 → webhook 调用失败
      - name: org_id
        presence: warnIfMissing              # 缺失 → PluginLog 告警，继续
    custom:
      - name: source
        value: edgion                        # 字面值
      - name: action
        template: "${ctx:action}"            # 调用时求值

  headers:
    origin:
      - name: X-Tenant-Id
      - name: X-User-Id
        presence: required
    custom:
      - name: X-Caller
        value: edgion-gateway
      - name: X-User
        template: "${ctx:user_id}"
      - name: Authorization
        secret:
          secretRef:
            name: webhook-token
          key: token
          prefix: "Bearer "

  cookies:
    origin:
      - name: session_id
    custom:
      - name: webhook_call_id
        template: "${ctx:trace_id}"
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `request.path.template` | String | 冻结的传入路径 | 出站路径；字面或 `${type:name}` 模板。 |
| `request.path.allowOverride` | Boolean | `false` | 为 `true` 时，消费方的 `pathOverride` 在调用时替换整个路径；为 `false` 时，任何 `pathOverride` 被忽略。 |
| `request.method.template` | String | 冻结的传入方法 | 出站 HTTP 方法；字面（`POST`、`GET` 等）或模板。 |
| `request.{args,headers,cookies}.origin` | Array | `[]` | 按名称从**冻结**的传入请求复制的字段。 |
| `request.{args,headers,cookies}.custom` | Array | `[]` | 运营者构造的字段（字面 / 模板 / secret）。 |

**Origin 字段**从传入请求的冻结入口快照中复制同名值——不受链中较早插件所做任何改写的影响。每个 origin 字段只有 `{ name, presence }`。共有三类 origin 组，对应 HTTP 的三个位置：

- `headers.origin`——复制请求头。**请求头名称大小写不敏感**（HTTP 标准）。
- `args.origin`——复制查询字符串参数。**arg 名称大小写敏感。**
- `cookies.origin`——复制单个 Cookie。**Cookie 名称大小写敏感。**

如果你想要请求头的**当前**（可能已被改写）视图，请改用 custom 模板字段：`custom: [{ name: X-Foo, template: "${header:X-Foo}" }]`。

#### 存在性策略（presence）

`presence` 只存在于 `origin` 字段上（custom 字段总有值）。它控制当命名字段在传入请求中缺失时的行为：

| 策略 | 字段缺失时的行为 | 何时使用 |
|------|------------------|----------|
| `skipIfMissing`（默认） | 从出站请求中静默省略该字段，不记录日志。 | 缺失属正常的可选增强。 |
| `warnIfMissing` | 省略该字段，写入一条 PluginLog 告警，请求继续。 | *应当*存在的字段——你想要可见性但不想让调用失败。 |
| `required` | webhook 调用被视为失败。随后 `statusOnError` / `allowDegradation` 与其他任何 webhook 失败完全一样地生效。 | 后端缺之不可的字段。 |

`presence` 与 `allowDegradation` 正交且可组合：`presence: required` 决定缺失字段是否*算作失败*；`allowDegradation` 随后决定该失败是中止插件链（`false`，默认）还是静默跳过 webhook 并继续（`true`）。

#### 自定义字段来源

每个 `custom` 条目为 `{ name, <source> }`，其中 source 是以下之一（**且仅一个**）：

| 来源 | 形式 | 解析时机 | 说明 |
|------|------|----------|------|
| 字面 | `value: "edgion"` | 无 | 逐字的静态字符串。 |
| 模板 | `template: "${ctx:user_id}"` | 调用时 | 由 ctx-set 模板引擎插值（见[模板变量](#模板变量)）。 |
| Secret | `secret: { secretRef: {...}, key: ..., prefix: ... }` | 配置时 | 由控制器从 Kubernetes Secret 读取并经 conf_sync 下发到网关。解析后的值在 `/configclient` 输出中被脱敏。 |

Secret 形式是注入凭据的方式（不再有类型化的 `auth` 块）：

```yaml
custom:
  - name: Authorization
    secret:
      secretRef:
        name: webhook-token       # 命名空间默认为 LinkSys 所在命名空间
      key: token                  # Secret data map 中由运营者选择的键
      prefix: "Bearer "           # 可选；拼接到解析值之前
```

`prefix` 用于把原始令牌变成 `Bearer <token>`、`Basic <encoded>` 等。Secret 轮换是自动的：当被引用的 Secret 变化时，控制器重新入队该 LinkSys 资源、重新解析值并重新下发到网关——无需运营者操作。

#### 模板变量

`path.template`、`method.template` 和 `custom[].template` 中的模板复用 ctx-set 模板引擎。它们在资源摄入时预编译，因此热路径只做替换。语法为 `${type:name}`：

| Token | 含义 |
|-------|------|
| `${header:Name}` | 传入请求头（当前视图）。 |
| `${query:name}` | 传入请求查询参数。 |
| `${cookie:name}` | 传入请求 Cookie。 |
| `${ctx:name}` | 由 ctx-set 或其他插件设置的网关上下文变量。 |
| `${path}` / `${uri}` | 当前（可能已改写的）路径 / URI。 |
| `${requestPath}` / `${requestUri}` | 冻结的入口路径 / URI。 |
| `${method}` | 当前 HTTP 方法。 |
| `${clientIp}` | 真实客户端 IP。 |

引用未定义变量的 `${ctx:X}` 解析为空字符串。要强制存在性，请在 origin 字段上使用 `presence: required`，或在调用 webhook 之前用 ctx-set 步骤设置该变量。

#### 路径覆盖

当 `request.path.allowOverride: true` 时，消费方插件可传入 `pathOverride` 在调用时替换整个路径。当 `allowOverride` 为 `false`（默认）或省略 `path` 块时，消费方发送的任何 `pathOverride` 都被**忽略**。这让 webhook 资源掌控自身的面貌，同时在作者主动允许时支持按调用的路径。

#### 字段冲突校验

以下组合在准入时以 `Accepted=False` 拒绝：

- 同一名称同时出现在某组的 `origin` 和 `custom` 中（例如 `headers.origin[name: X-Foo]` 和 `headers.custom[name: X-Foo]`）。
- 同一 `origin` 列表或同一 `custom` 列表内的重复名称。
- 同时声明 `headers.origin[name: Cookie]` 和任何非空的 `cookies.*` 列表（会重复注入 `Cookie` 请求头）——二者只能取其一。

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `timeoutMs` | Integer | `5000` | 请求超时（毫秒），必须大于 0。 |
| `tls` | Object | none | TLS 配置；详见 [TLS](#tls)。未设置时使用系统 CA 包。 |
| `successStatusCodes` | Array\<Integer\> | 任意 2xx | 视为成功的 HTTP 状态码。 |
| `maxResponseBytes` | Integer | `1024` | 读取的最大响应体大小（字节），受 `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes`（默认 16 KiB）限制。 |

> **已移除的字段。** `requestMethod`、`auth` 和 `defaultHeaders` 已弃用。在滚动升级期间它们仍被接受但运行时忽略，控制器在遇到时记录 WARN。它们将在下一个版本中移除——请在此之前迁移。详见[从旧 schema 迁移](#从旧-schema-迁移)。

### 降级与错误处理

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `allowDegradation` | Boolean | `false` | 为 `true` 时，webhook 失败会被静默跳过，继续消费方的 fallback 链。为 `false` 时，失败导致查询失败。 |
| `statusOnError` | Integer | `503` | `allowDegradation=false` 且 webhook 为唯一来源时返回的状态码，取值范围 `[200, 599]`。 |

### 重试

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `retry.maxRetries` | Integer | `1` | 最大重试次数（不含首次调用）。 |
| `retry.retryDelayMs` | Integer | `100` | 重试间隔（毫秒）。 |
| `retry.retryOnTimeout` | Boolean | `true` | 请求超时时重试。 |
| `retry.retryOnConnectError` | Boolean | `true` | 连接错误时重试。 |
| `retry.retryOnStatus` | Array\<Integer\> | `[]` | 触发重试的 HTTP 状态码。 |

省略 `retry` 时，webhook 在首次出错后立即失败。

### 限流

防止外部 webhook 服务被网关压垮。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `rateLimit.rate` | Integer | **必填**（设置 `rateLimit` 时） | 每时间窗口允许的最大调用次数。 |
| `rateLimit.windowSec` | Integer | `1` | 时间窗口大小（秒）。 |

### 健康检查

支持主动探测、被动监控或两者结合（推荐）。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `healthCheck.active.path` | String | none | 主动健康检查路径，附加到基础 URI。示例：`/healthz`。 |
| `healthCheck.active.intervalSec` | Integer | `10` | 探测间隔（秒）。 |
| `healthCheck.active.timeoutMs` | Integer | `2000` | 探测超时（毫秒）。 |
| `healthCheck.active.healthyThreshold` | Integer | `1` | 标记为健康所需的连续成功次数。 |
| `healthCheck.active.unhealthyThreshold` | Integer | `3` | 标记为不健康所需的连续失败次数（仅在被动检查禁用时使用）。 |
| `healthCheck.passive.unhealthyThreshold` | Integer | `3` | 标记为不健康所需的连续失败次数。 |
| `healthCheck.passive.failureStatusCodes` | Array\<Integer\> | `[500, 502, 503, 504]` | 视为失败的 HTTP 状态码。 |
| `healthCheck.passive.countTimeout` | Boolean | `true` | 将超时计为失败。 |
| `healthCheck.passive.backoff.initialSec` | Integer | `5` | 初始退避时间（秒，仅在主动检查禁用时生效）。 |
| `healthCheck.passive.backoff.multiplier` | Number | `2.0` | 退避乘数。 |
| `healthCheck.passive.backoff.maxSec` | Integer | `60` | 最大退避时间（秒）。 |

### 示例

以下示例各聚焦于一个特性。它们是可运行的 LinkSys 清单；按需裁剪不需要的字段。

#### 1. Service 与 EndpointSlice 解析

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver-svc
  namespace: production
spec:
  type: webhook
  config:
    target:
      backendRef:
        kind: Service              # 从 EndpointSlice 解析就绪端点
        name: key-service
        namespace: production
        port: 8080
    request:
      method:
        template: POST
    timeoutMs: 3000
---
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver-clusterip
  namespace: production
spec:
  type: webhook
  config:
    target:
      backendRef:
        kind: ServiceClusterIp     # 通过 ClusterIP 的 DNS 名称解析（kube-proxy）
        name: key-service
        port: 8080
    request:
      method:
        template: POST
```

#### 2. Origin 字段（白名单转发）

从冻结的传入请求中转发选定的值，保持不变。省略 `path` / `method` 时复用传入的路径和方法——这是 forward-auth 风格 webhook 的常见形状。

```yaml
spec:
  type: webhook
  config:
    target:
      backendRef: { kind: Service, name: authz, port: 8080 }
    request:
      headers:
        origin:
          - name: X-Tenant-Id
          - name: X-User-Id
      args:
        origin:
          - name: tenant_id
      cookies:
        origin:
          - name: session_id
```

#### 3. Custom 字面字段

注入运营者掌控的静态值。

```yaml
spec:
  type: webhook
  config:
    target:
      backendRef: { kind: Service, name: audit-sink, port: 8080 }
    request:
      method:
        template: POST
      headers:
        custom:
          - name: X-Source
            value: edgion-gateway
          - name: X-Env
            value: production
```

#### 4. 来自 K8s Secret 的 Custom 字段

从 Secret 注入凭据。`prefix` 构造完整的请求头值（`Bearer <token>`）。

```yaml
spec:
  type: webhook
  config:
    target:
      backendRef: { kind: Service, name: key-service, port: 8080 }
    request:
      method:
        template: POST
      headers:
        custom:
          - name: Authorization
            secret:
              secretRef:
                name: webhook-token   # 在 LinkSys 所在命名空间
              key: token
              prefix: "Bearer "
          - name: X-API-Key
            secret:
              secretRef:
                name: webhook-apikey
              key: apiKey             # 无 prefix → 原始值
```

#### 5. 模板变量替换

在调用时从传入请求数据和网关上下文计算出站值。

```yaml
spec:
  type: webhook
  config:
    target:
      backendRef: { kind: Service, name: enrich, port: 8080 }
    request:
      path:
        template: "/resolve/${ctx:action}"
      method:
        template: POST
      headers:
        custom:
          - name: X-User
            template: "${ctx:user_id}"
          - name: X-Forwarded-Path
            template: "${requestPath}"     # 冻结的入口路径
          - name: X-Client-Ip
            template: "${clientIp}"
```

#### 6. 存在性策略（required / skipIfMissing / warnIfMissing）

```yaml
spec:
  type: webhook
  config:
    target:
      backendRef: { kind: Service, name: authz, port: 8080 }
    allowDegradation: false        # required 字段失败时暴露 statusOnError
    statusOnError: 503
    request:
      headers:
        origin:
          - name: X-Trace-Id                       # 默认：skipIfMissing
          - name: X-User-Id
            presence: required                     # 缺失 → 调用失败 → 503
          - name: X-Org-Id
            presence: warnIfMissing                # 缺失 → PluginLog 告警，继续
```

#### 7. 路径覆盖（allowOverride）

```yaml
spec:
  type: webhook
  config:
    target:
      backendRef: { kind: Service, name: user-resolver, port: 8080 }
    request:
      path:
        template: "/v1/users/lookup"
        allowOverride: true        # 消费方可通过 pathOverride 替换路径
      method:
        template: POST
```

当 `allowOverride: false`（或没有 `path` 块）时，消费方的 `pathOverride` 被忽略，使用此模板（或冻结的传入路径）。

#### 8. ctx-set 集成（`${ctx:*}` 变量）

消费方插件先设置上下文变量，再从 webhook 的模板中引用它们。按调用的动态值完全经由 `${ctx:*}` 流入——没有单独的"调用方参数"概念。

```yaml
# Webhook 资源
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: user-resolver
  namespace: production
spec:
  type: webhook
  config:
    target:
      backendRef: { kind: Service, name: user-service, port: 8080 }
    request:
      path:
        template: "/resolve/${ctx:action}"
        allowOverride: true
      method:
        template: POST
      headers:
        custom:
          - name: X-User-Id
            template: "${ctx:user_id}"
---
# 消费方插件（ctx-set），位于 HTTPRoute filter 上
type: ctx-set
vars:
  - { name: user_id, template: "${header:X-User-Id}" }
  - { name: action, value: "get-user" }
  - name: resolved_user
    from:
      type: webhook
      webhookRef: production/user-resolver
      pathOverride: "/special/path"   # 仅因 allowOverride: true 才被采纳
```

#### 示例：带重试和健康检查的弹性 webhook

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver
  namespace: production
spec:
  type: webhook
  config:
    target:
      backendRef:
        kind: Service
        name: key-service
        namespace: production
        port: 8080
    request:
      method:
        template: POST
      headers:
        custom:
          - name: Authorization
            secret:
              secretRef:
                name: webhook-token
              key: token
              prefix: "Bearer "
    timeoutMs: 3000
    allowDegradation: false
    statusOnError: 503
    retry:
      maxRetries: 1
      retryDelayMs: 100
      retryOnTimeout: true
      retryOnConnectError: true
      retryOnStatus: [502, 503, 504]
    rateLimit:
      rate: 200
      windowSec: 1
    healthCheck:
      active:
        path: "/healthz"
        intervalSec: 10
        timeoutMs: 2000
        healthyThreshold: 1
        unhealthyThreshold: 3
```

### 从旧 schema 迁移

Webhook schema 在本版本中发生变化。旧字段在滚动升级期间被接受但运行时忽略（控制器记录 WARN），并将在下一版本中移除。请在此之前迁移每个 webhook LinkSys。

| 旧 | 新 |
|----|----|
| `target.type: uri` + `target.uri` | 指向（代理）Service 的 `target.backendRef`——不再支持原始 URL |
| `target.type: service` + `target.name/port` | `target.backendRef.{kind,name,namespace,port}`（去掉 `type` 标签） |
| `auth.bearer` / `auth.basic` / `auth.hmac` / `auth.apiKey` | 带 `secret:` 的 `request.headers.custom`（用 `prefix:` 拼 `Bearer `/`Basic `） |
| `defaultHeaders`（静态 map） | 每条用 `value:` 的 `request.headers.custom` |
| `requestMethod: POST` | `request.method.template: POST`（旧默认是 `GET`） |
| 消费方 `pathSuffix` | webhook 上的 `request.path`，或当 `allowOverride: true` 时消费方 `pathOverride` |
| 消费方 `forwardHeaders` | webhook 上的 `request.headers.origin`（集中式白名单） |

旧：

```yaml
spec:
  type: webhook
  config:
    target:
      type: uri
      uri: "https://key-service.internal/resolve"
    requestMethod: POST
    auth:
      type: bearer
      secretRef:
        name: webhook-token
    defaultHeaders:
      X-Source: edgion
```

新：

```yaml
spec:
  type: webhook
  config:
    target:
      backendRef:
        kind: Service
        name: key-service        # 对于外部端点，将此指向一个代理 Service
        namespace: production
        port: 8080
    request:
      method:
        template: POST
      headers:
        custom:
          - name: X-Source
            value: edgion
          - name: Authorization
            secret:
              secretRef:
                name: webhook-token
              key: token
              prefix: "Bearer "
```

消费方侧，`pathSuffix` 和 `forwardHeaders` 已移除：

```yaml
# 旧
- type: key-auth
  config:
    keys:
      - type: webhook
        webhookRef: production/user-resolver
        pathSuffix: "/special/path"
        forwardHeaders: ["X-Tenant-Id", "X-User-Id"]

# 新——在 webhook 资源上声明 path/headers；消费方只覆盖路径
- type: key-auth
  config:
    keys:
      - type: webhook
        webhookRef: production/user-resolver
        pathOverride: "/special/path"   # 仅当 webhook 上 allowOverride: true 时被采纳
```

> **外部端点。** `target.type: uri` 和 `kind: ExternalName` 不支持。用代理 Service（LoadBalancer，或由 ingress controller 前置的 ClusterIP）前置外部主机，并按名称引用该 Service。

### Webhook 故障排查

**Webhook 无法解析 / "webhook not found"。**
检查 `target.backendRef`：命名的 Service 必须存在于解析出的命名空间中，且 `port` 必须已设置且有效。`Accepted=False` 的 LinkSys 不会加载进网关运行时，因此任何调用它的消费方都会命中相同的"webhook 不可用"路径。运行 `kubectl get linksys <name> -o yaml` 并阅读 `Accepted` 条件消息。

**关于 `kind` 值的 `Accepted=False`。**
只有 `Service`、`ServiceClusterIp`、`ServiceEndpointSlice` 和留空有效。`ExternalName` 和 `ServiceImport` 被拒绝——请改用代理 Service。

**Required 字段错误 / 意外的 `statusOnError`。**
某个 `presence: required` 的 `origin` 字段在传入请求中缺失，这算作 webhook 失败。确认客户端确实发送了该 header/arg/cookie（注意大小写：请求头大小写不敏感，arg 和 cookie 大小写敏感）。要降级而非失败，设置 `allowDegradation: true`；要仅记录，用 `presence: warnIfMissing`。

**模板变量解析为空。**
`${ctx:NAME}` 在变量从未设置时返回空。确保在 webhook 被调用之前有一个 ctx-set 步骤设置它，并检查 `${type:name}` 拼写。要强制存在性，优先在 `origin` 字段上用 `presence: required` 而非模板。

**路径覆盖不生效。**
`pathOverride` 仅当 webhook 的 `request.path.allowOverride` 为 `true` 时被采纳。如果 `path` 被省略或 `allowOverride` 为 `false`，覆盖被静默忽略，使用模板（或冻结的传入路径）。

**关于重复或冲突字段的准入错误。**
你在 `origin` 和 `custom` 中命名了同一字段、在一个列表内列了重复名称，或同时声明了 `headers.origin[name: Cookie]` 和一个 `cookies.*` 列表。移除冲突。

**健康状态重置 vs. 端点变动。**
编辑任何 `request.*` 字段（或目标）会重建 webhook 运行时并**重置**其健康状态——这是有意的，运营者的配置变更不等于 Pod 变动。EndpointSlice 变化（Pod 扩缩/重启）**不会**重建运行时，也**不会**重置健康状态：端点在调用时重新解析，而聚合健康模型被保留。

## Kafka 连接器

⚠️ **Kafka 当前为占位实现。** schema 有效且资源报告 `Accepted=True`，但 Gateway 尚未集成 Kafka provider。实际可用性取决于后续版本。

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: events-kafka
  namespace: production
spec:
  type: kafka
  config:
    brokers:
      - "kafka-1.production.svc:9092"
      - "kafka-2.production.svc:9092"
      - "kafka-3.production.svc:9092"
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `brokers` | Array\<String\> | **必填** | Kafka broker 地址（`host:port`），集群时列出多个。 |

## 常见配置场景

### 1. 开发用最小 Redis

无认证、无 TLS 的单节点 Redis，适合快速验证。

⚠️ **仅限开发！请勿用于生产！**

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: dev-redis
  namespace: edgion-test
spec:
  type: redis
  config:
    endpoints:
      - "redis://redis:6379"
```

### 2. 凭据集中存放在专用命名空间

将所有连接器的 Secret 存放在 `credentials` 命名空间，并通过 `ReferenceGrant` 授权访问。

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-linksys-secrets
  namespace: credentials
spec:
  from:
    - group: edgion.io
      kind: LinkSys
      namespace: production
  to:
    - group: ""
      kind: Secret
---
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: cache-redis
  namespace: production
spec:
  type: redis
  config:
    endpoints:
      - "rediss://redis.production.svc:6379"
    auth:
      secretRef:
        name: redis-auth
        namespace: credentials
    tls:
      enabled: true
```

### 3. 带 mTLS 客户端证书的 TLS 加密 Etcd

CA 包内联（`caSource: Inline`）。客户端证书和私钥来自 Secret（`clientSecretRef`）。

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: config-etcd
  namespace: production
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd-1.production.svc:2379"
      - "https://etcd-2.production.svc:2379"
    auth:
      secretRef:
        name: etcd-creds
    tls:
      enabled: true
      caSource: Inline
      caCert: |
        -----BEGIN CERTIFICATE-----
        <base64-encoded-cert>
        -----END CERTIFICATE-----
      clientSecretRef:
        name: etcd-client-tls
```

## 常见问题

### Q1：为什么我的 LinkSys 报告 `Accepted=False/Invalid`？

**A**：结构或后端特定校验失败。常见原因：`endpoints` 列表为空、`topology.mode=sentinel` 但缺少 `sentinel` 块（或 `mode=cluster` 但缺少 `cluster` 块）、webhook 的 `target.backendRef` 缺少/为零的 `port` 或不支持的 `kind`（仅允许 `Service`、`ServiceClusterIp`、`ServiceEndpointSlice`、留空）、webhook 的 `request` 存在冲突字段（同一名称同时出现在 `origin` 和 `custom`、列表内重复名称、或同时声明 `headers.origin[name: Cookie]` 和 `cookies.*` 列表）。状态 `message` 字段列出了具体错误。

### Q2：凭据看起来正确，但 `ResolvedRefs` 为 `False`。为什么？

**A**：Secret 无法解析。请检查原因：

- `SecretNotFound`——Secret 尚不存在。Controller 在 Secret 创建后会自动重新入队。
- `RefNotPermitted`——跨命名空间引用但没有匹配的 `ReferenceGrant`。
- `InvalidSecretType`——`Secret.type` 不在该认证类型解析器的允许列表中。
- `MissingKeys`——Secret 缺少必需键（例如 basic auth 缺少 `password`，apiKey 缺少 `apiKeyId` / `apiKeySecret`）。

### Q3：能直接在 LinkSys YAML 中写密码吗？

**A**：不能。认证和 TLS 结构体开启了 `deny_unknown_fields`，内联敏感字段（`username`、`password`、`bearerToken`、`apiKeyId`、`hmacKey`、`clientKey` 等）会导致解析失败而非被忽略。请始终使用 `secretRef`。

### Q4：能否为 Redis 或 Etcd 禁用 TLS 验证（`verify: false`）？

**A**：不能。Redis 和 Etcd 客户端不支持禁用证书验证——在任何环境下，`verify: false` 对这两种连接器均会在启动时直接失败。这是故意的设计。如需连接使用私有 CA 的后端，请通过 `tls.caSource: Inline` + `tls.caCert`（内联 PEM）或 `tls.caSource: SecretRef` + `tls.caSecretRef` 提供 CA 证书。

### Q5：轮换 Secret 后，现有连接会怎样？

**A**：连接器会透明地重建。当引用的 Secret 发生变化时，Controller 重新计算凭据，将新规格推送给 Gateway，Gateway 在旧连接优雅排空的同时重建客户端。

### Q6：凭据解析失败后，Edgion 会降级为匿名连接吗？

**A**：不会。如果必需凭据待处理或无效，Gateway 会快速失败而不构建客户端，绝不降级为未认证连接。

## 获取帮助

如遇问题：

1. 检查本文档的常见问题部分。
2. 检查资源状态：`kubectl get linksys <name> -o yaml`，查看 `Accepted` / `ResolvedRefs` 状态条件。
3. 检查网关日志。
4. 在 GitHub 提交 issue。

## 相关文档

- [Secret 管理](./secret-management.md)
- [跨命名空间引用](./reference-grant.md)
- [mTLS 配置](./mtls.md)
- [Redis 限流](../../user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md)
- [Access Log](../observability/access-log.md)
