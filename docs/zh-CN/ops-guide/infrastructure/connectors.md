# 连接器（LinkSys）

> **🔌 Edgion 扩展**
>
> 连接器由 `LinkSys` CRD（`edgion.io/v1`）定义，用于集成外部系统，不属于标准 Gateway API 规范。

## 概述

**连接器**是指向外部系统（Redis、Etcd、Elasticsearch、HTTP webhook、Kafka 或 OTLP/gRPC Collector）的可复用命名句柄。您只需声明一次 `LinkSys` 资源，通过 Kubernetes Secret 挂载凭据，即可在任何需要该后端的功能中引用它。这样可以将连接详情和凭据集中管理，避免在每个插件和策略中重复配置。

一个 `LinkSys` 对象对应一个连接器。顶层字段 `spec.type` 选择后端类型，`spec.config` 承载该后端的配置。多个 Edgion 功能通过 `namespace/name` 引用连接器：

- 分布式（集群级）限流通过 `namespace/name` 引用 `redis` 连接器——详见 [Redis 限流](../../user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md)。
- 异步日志的输出目标可引用 `elasticsearch`、`kafka` 或 `otlp` 连接器——详见 [Access Log](../observability/access-log.md)。
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
| bearer（Elasticsearch / OTLP） | `Opaque` | `bearerToken` | — |
| TLS（所有后端） | `kubernetes.io/tls` 或 `Opaque` | `tls.crt`、`tls.key`、`ca.crt` 中至少一个 | — |

应用层键名使用 camelCase；TLS 键名遵循 Kubernetes `kubernetes.io/tls` 标准。

> **Webhook 凭据有所不同。** Webhook 连接器不再有类型化的 `auth` 块。请在自定义请求字段模板中引用 Secret，例如 `template: "Bearer ${secretRef:production/webhook-token:token}"`；命名空间和 data key 都必须显式给出。详见 [Webhook 连接器 → 自定义字段来源](#自定义字段来源)。

### TLS

每个后端均接受相同的 `tls` 块用于保护连接。

```yaml
tls:
  enabled: true
  verify: true                 # 验证对端证书（默认 true）；false 由运维人员负责，启动时记录 WARN
  validation:                  # 可选；省略则使用内置 webpki 根证书
    caCertificateRefs:         # CA 证书引用列表；LinkSys 仅支持 kind: Secret
      - kind: Secret
        name: my-ca-bundle     # Secret 须包含 ca.crt 键（类型 kubernetes.io/tls 或 Opaque）
    wellKnownCACertificates: System  # 或显式使用内置 webpki 根证书
    # Phase 1：hostname 和 subjectAltNames 字段已暴露，但暂不强制执行，配置后 Accepted=False。
  clientCertificateRef:
    name: my-client-tls        # 包含 tls.crt + tls.key 的 Secret，用于客户端 mTLS
```

> **Phase 1 CA 引用限制**：LinkSys（redis / etcd / elasticsearch / kafka / otlp）的 `validation.caCertificateRefs` 在本版本中仅支持 `kind: Secret`，不支持 ConfigMap CA。如需 ConfigMap CA，请使用插件（OIDC / LDAP / ForwardAuth / EGC）的 TLS 配置路径。内联 CA（`caCert` 字段）已从本版本中移除，请改用包含 `ca.crt` 键的 Secret。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | Boolean | `false` | 为连接启用 TLS。对于已隐含 TLS 的 URL（`https://`、`rediss://`），此字段仅添加自定义 CA / 客户端证书处理。 |
| `verify` | Boolean | `true` | TLS 块启用时验证对端证书。`verify: false` 由运维人员负责，启动时记录 WARN。Redis、Etcd 和 OTLP 拒绝启用块中的 `verify: false`，其 SDK 无跳过验证 API。 |
| `validation` | Object | none | 可选信任锚配置。省略则使用内置 webpki 根证书。 |
| `validation.caCertificateRefs` | Array | none | CA 证书 Secret 引用列表（每项：`{ kind: Secret, name, namespace }`）。Secret 须包含 `ca.crt` 键。LinkSys 仅支持 `kind: Secret`。自定义 CA **替换**内置根证书。 |
| `validation.wellKnownCACertificates` | String | none | 显式使用内置 webpki 根证书。允许值：`System`。 |
| `clientCertificateRef` | Object | none | 包含客户端 mTLS 身份（`tls.crt` / `tls.key` 键）的 Kubernetes Secret。 |

客户端 mTLS 身份（证书 + 私钥）必须通过 `clientCertificateRef` 提供。

当 `tls.enabled` 为 `false`（包括默认值）时，该块中的验证设置和证书引用不生效。
引用的 Secret 缺失或无效不会阻止连接器就绪。停用时也会清空此前解析的 TLS
证书及其依赖；重新启用后，需要重新解析这些引用。认证 Secret 仍然独立要求有效。
`https://` 或 `rediss://` 等安全端点 URL 仍选择 TLS，并使用提供方的正常默认值，
不会使用已停用块中的自定义设置。`credentialSource` 提供方的 TLS 块也遵循此规则。

### 可观测性

Etcd 和 Elasticsearch 支持共享的 `observability` 块。Redis 使用 Gateway
二进制统一管理的日志以及指标清单约束的指标，不提供连接器级观测配置块。

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
    timeout:
      connect: 3s
      command: 2s
```

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `endpoints` | Array\<String\> | 取决于拓扑 | Redis 端点：standalone 必须恰好一个，cluster 至少一个，sentinel 必须省略或为空。 |
| `db` | Integer | none | 数据库索引（0–255）；Redis 服务端必须提供所选数据库。 |
| `auth` | Object | none | 认证配置。存在时 `auth.secretRef` 必填。Secret 需提供 `password`；`username` 可选（Redis 6+ ACL）。 |
| `tls` | Object | none | TLS 配置；详见 [TLS](#tls)。 |

### 连接池与超时

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `pool.size` | Integer | `8` | 固定连接数，范围 1–64；所有连接在初始化时建立。 |
| `timeout.connect` | Duration | `5s` | 连接超时，必须大于零且不超过 `10s`。 |
| `timeout.command` | Duration | `5s` | 单条 Redis 命令超时，必须大于零且不超过 `30s`。 |

Redis duration 使用 Gateway API 格式，例如 `500ms`、`3s` 或 `10s`。连接器以固定的
有界指数退避持续恢复连接，但不会在连接失败且执行结果未知时透明重放普通命令；调用方
必须自行判断重复操作是否安全。

### 拓扑：standalone、sentinel、cluster

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `topology.mode` | Enum | — （设置 `topology` 时必填） | 部署拓扑。可选值：`standalone` \| `sentinel` \| `cluster`。 |
| `topology.sentinel` | Object | none | Sentinel 配置。**`mode=sentinel` 时必填。** |
| `topology.cluster` | Object | none | 可选的 Cluster 路由配置。 |
| `topology.sentinel.masterName` | String | **必填** | Sentinel 监控的主节点名称。 |
| `topology.sentinel.sentinels` | Array\<String\> | **必填** | Sentinel 地址（`host:port`）。 |
| `topology.cluster.maxRedirects` | Integer | `5` | 最大集群重定向跳数，范围 0–64。 |

省略 `topology` 时连接器以 standalone 模式运行。`mode=sentinel` 但缺少 `sentinel`
块时，状态报告 `Accepted=False/Invalid`。Cluster 模式仅在覆盖 `maxRedirects` 时需要
`cluster` 块。

每个活动端点必须采用 `host`、`host:port`、`[IPv6]`、`[IPv6]:port`、
`redis://host[:port]` 或 `rediss://host[:port]` 之一，默认端口为 6379。
用户信息、非根路径、查询、fragment、未知 scheme、空 host 和端口 0 均会被拒绝。
解析采用全有或全无语义：任意 seed 无效都会拒绝整个资源，不会静默缩减拓扑。
验证错误只报告字段和列表索引，不回显输入值。

未设置 `tls.enabled: true` 时，所有活动端点必须选择同一种传输：`rediss://`
表示 TLS，`redis://` 和无 scheme 形式表示明文。显式 TLS 作用于整个 Fred 客户端，
因此允许 endpoint 采用混合拼写。

### DNS 与出站流量策略

Redis 端点由 Gateway 在 Fred 建立连接时解析，而不是由 Controller 预先解析，也不会在每条
Redis 命令执行时解析。配置的 seed、自动重连、Sentinel 发现的主节点，以及 Cluster 发现或
重定向到的节点都经过同一拨号时解析器。解析使用 Edgion 共享解析器的系统 DNS 模式，结果按
`outbound_egress_dns_cache_ttl_ms` 缓存（默认 5 秒）。

Fred 建立连接前会立即对每个解析地址分类。生产模式始终拒绝回环、链路本地和未指定地址；
私有地址与唯一本地地址遵循 Gateway 全局 `outbound_egress_block_private` 配置，Redis 不提供
连接器级覆盖。此防护属于纵深防御，不能替代 Gateway Pod 的 `NetworkPolicy`。共享配置和地址
分类详见[出站流量策略（SSRF 防护）](#出站流量策略ssrf-防护)。

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
      clientCertificateRef:
        name: etcd-client-tls
    namespace: "/my-app/"
```

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `endpoints` | Array\<String\> | **必填** | Etcd 端点，至少一个。示例：`"https://etcd-1:2379"`。 |
| `auth` | Object | none | 认证配置。Secret 通常同时提供 `username` 和 `password`。 |
| `tls` | Object | none | TLS 配置；详见 [TLS](#tls)。Etcd（与 Redis 一样）在任何环境下均拒绝启用 TLS 块中的 `verify: false`。 |
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
| `bulk.maxRetries` | Integer | `3` | 传输失败、HTTP 5xx 响应和 item 级 429 拒绝共用的重试预算。 |
| `bulk.backoffMs` | Integer | `1000` | 重试间隔的初始退避（毫秒）。 |
| `bulk.maxBodyBytes` | Integer | `10485760`（10 MB） | 批量请求体最大大小（字节），上限 50 MB。 |
| `bulk.channelSize` | Integer | `10000` | 生产者到批量循环的通道容量，上限 100000。 |

当 Bulk HTTP 请求成功但部分 item 失败时，连接器只重试状态为 `429` 的 item，并保持其请求
顺序。重试以 `bulk.backoffMs` 为初始延迟，继续使用现有指数退避；传输失败、HTTP 5xx 和
item 级重试共用同一个 `bulk.maxRetries` 预算。串行重试期间，后续日志可以保留在有界的
`bulk.channelSize` 队列中，最多达到配置的容量。队列已满后，新日志会被丢弃并计入
`edgion_access_log_dropped_total`；本次失败提交不会继续尝试 fallback。其他 item
失败视为永久失败。如果响应无法安全关联（例如 item 数量与请求不一致），则不会重试，因为
重发结果不明确的操作可能产生重复文档。429 重试预算耗尽后，连接器会转为不健康，已配置的
access-log fallback 可以接收后续日志。

### Access Log fallback 与手动回填

Elasticsearch LinkSys schema 中没有 `failedCache` 字段。对于 Access Log，
应在 Gateway 进程配置中为 Elasticsearch 主 sink 配置有界 LocalFile
`fallback`：

```yaml
access_log:
  enabled: true
  output:
    es:
      linkSysRef:
        namespace: production
        name: logs-es
  fallback:
    localFile:
      path: logs/es_access_fallback.log
      diskUsageThresholdPercent: 90
      rotation:
        strategy: { size: 104857600 }
        maxFiles: 10
```

first-healthy 路由只在主 sink 报告不健康后才将新日志发往 fallback；
Elasticsearch 仍健康时的 channel-full 提交会被丢弃，不会切换到 fallback。
Elasticsearch 恢复后，经过身份验证的运维人员可调用
`POST /api/v1/access-log/backfill` 重放专用 fallback 文件。回填仅支持
Elasticsearch，语义为至少一次。运维限制和响应说明详见
[Access Log 降级与回填](../observability/access-log.md#降级fallback-sink-与手动回填)。

## Webhook 连接器

> ⚠️ **破坏性变更：`successStatusCodes` 已被 `success.statusCodes` 取代。** `successStatusCodes` 已不再是运行时可识别字段。仍设置它的 webhook YAML 会继续被接纳，但 serde 会丢弃该字段，成功判定退化为“任意 2xx”。Controller 现在会通过提示性的未知字段 warning、`UnknownConfigFields` 条件、diagnostics/metric，以及 Admin API 写入和 validate 响应报告精确路径；该发现不会改变 `Accepted`，也不会阻断同步。请立即审查每个 webhook LinkSys（以及共用该连接类型的每个 ForwardAuth 插件）并迁移到嵌套的 `success:` 块；详见下文[Success（成功判定）](#成功判定-success)。

Webhook 连接器让插件（ctx-set、key-auth、hmac-auth 等）调用外部 HTTP 服务，用于用户解析、权限校验、上下文增强、审计日志等查询。该连接器是出站请求的**唯一事实来源**：它声明后端（直接写在 `target` 下的 `name`/`namespace`/`port`/`kind`/`group` 字段），并通过 `request` 块精确定义发送的路径、方法、查询参数、请求头和 Cookie。消费方插件不能再静默地改变请求形状——它们只能向 webhook 作者声明的命名槽位填值，可选地覆盖路径（在被允许时），以及解析响应。出站字段遵循**白名单**模型：除非 webhook 明确声明，否则不转发任何内容，因此上游某个行为异常的插件无法泄露运营者从未打算暴露的请求头。

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
            template: "Bearer ${secretRef:production/webhook-token:token}"
    timeoutMs: 3000
    success:
      statusCodes: [200]
    maxResponseBytes: 2048
```

### 目标

每个 webhook 通过直接写在 `target` 下的 Service 字段引用其后端——字段集合（`group`/`kind`/`name`/`namespace`/`port`）与 Gateway API 自身的 `BackendObjectReference` 相同，但被展平、没有嵌套的包装键。不再有 `type: uri` / `type: service` 的选择，也没有 `backendRef` 这个键——展平后的 Service 字段（或下文的 `url` 形式）是唯一的目标形态。

```yaml
target:
  group: ""              # 可选；默认为 core API 组
  kind: Service          # Service | ServiceClusterIp | ServiceEndpointSlice（留空默认为 Service）
  name: webhook-backend
  namespace: production  # 可选；默认为 LinkSys 所在命名空间
  port: 8080             # 必填
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `target.group` | String | core（`""`） | 被引用对象的 API 组。 |
| `target.kind` | String | `Service` | 目标类型。可选：`Service`、`ServiceClusterIp`、`ServiceEndpointSlice` 或留空（默认 `Service`）。 |
| `target.name` | String | **必填** | Kubernetes Service 名称。 |
| `target.namespace` | String | LinkSys 所在命名空间 | Service 所在命名空间。 |
| `target.port` | Integer | **必填** | Service 端口，必须大于 0。缺少端口会在准入时以 `Accepted=False` 拒绝。 |

按 kind 的解析方式：

- `Service`（默认，留空/未设置亦同）：网关在调用时从 Kubernetes EndpointSlice 存储解析就绪端点。
- `ServiceEndpointSlice`：同样的 EndpointSlice 解析的显式形式。
- `ServiceClusterIp`：通过 Service ClusterIP 的 DNS 名称解析（kube-proxy）。

`kind: ExternalName` 和 `ServiceImport` **不**支持作为 webhook 目标，会在准入时以带说明的 `Accepted=False` 消息拒绝。要访问外部端点，请创建一个代理 Service（例如 `LoadBalancer` 类型的 Service，或由 ingress controller 前置的 `ClusterIP` Service）指向外部主机，然后按名称引用该 Service——或者改用下文的 URL 目标。

### URL 目标（外部服务）

除了 Service 字段，webhook 也可以直接指向任意外部 URL：

```yaml
target:
  url: https://api.example.com
  blockPrivate: true       # 可选；仅 URL 目标可用，见下文"出站流量策略"
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `target.url` | String | none | 外部 URL 目标。**仅限 origin**：`scheme://host[:port]`——不允许路径、查询字符串、片段或内嵌凭据。与 `target.name` / `target.namespace` / `target.port` / `target.kind` / `target.group` 互斥。 |
| `target.blockPrivate` | Boolean | `false`（继承网关本地默认值） | 仅 URL 目标可用。详见[出站流量策略（SSRF 防护）](#出站流量策略ssrf-防护)。 |

**为什么仅限 origin。** `target.url` 不得携带路径、查询字符串、片段或内嵌凭据——设置任一项都会在准入时以 `Accepted=False` 拒绝。出站路径完全由 `request.path.template`（见下文[出站请求](#出站请求)）决定，该字段已经能与 `healthCheck.active.path` 及消费方的 `pathOverride` 正确组合。如果 `target.url` 同时也能携带路径，上述每一条组合规则都需要再定义一次"基础路径"，且相互冲突——仅限 origin 消除的是这种歧义本身，而不是靠文档去解释它。`https://api.example.com` 搭配 `request.path.template: /v1/check` 是典型写法。

**与 Service 字段互斥。** 同时设置 `target.url` 与 `target.name` / `target.namespace` / `target.port` / `target.kind` / `target.group` 中的任意一个都会在准入时被拒绝——webhook 要么指向外部 URL，要么指向 Kubernetes Service，不能两者兼有。

**`http://` 搭配 `tls.enabled: true` 会被拒绝。** 如果 `target.url` 以 `http://` 开头（scheme 比较大小写不敏感）且连接的 `tls.enabled` 为 `true`，准入会失败并报 `Accepted=False`。这是有意为之：URL 目标的 scheme 本身已经决定了明文还是 TLS，若再叠加显式的 `tls.enabled: true`，要么被静默忽略，要么静默地降级运营者的 TLS 意图——两者都不可接受，因此选择直接拒绝而非猜测意图。

**独享连接池。** URL 目标总是构建自己的 `reqwest::Client`——即便完全没有设置 `tls` 块——绝不回退到其他消费方共用的进程级共享 HTTP 客户端。这使得每个 URL 目标能拥有自己的 DNS 解析与地址缓存（见[出站流量策略（SSRF 防护）](#出站流量策略ssrf-防护)）；也意味着 URL 目标不会与其他无关 webhook 共享连接池耗尽或健康状态。

### 出站流量策略（SSRF 防护）

URL 目标按主机名寻址，因此——与通过 Kubernetes EndpointSlice 解析的 Service 目标不同——网关需要自行完成 DNS 解析，并在建立连接前对结果进行分类。两个执行点实现同一套策略，因此无论 `target.url` 指向的是主机名还是 IP 字面量，行为都一致：

- **始终拒绝**：回环地址（`127.0.0.0/8`、`::1`）和链路本地地址，其中包括云元数据地址 `169.254.169.254` 及其所在的整个 `169.254.0.0/16` 网段。
- **`target.blockPrivate: true` 时额外拒绝**：RFC1918 私有网段（`10/8`、`172.16/12`、`192.168/16`）及唯一本地地址（`fd00::/8`），在始终拒绝的类别之外追加。
- **默认（`blockPrivate` 未设置或为 `false`）**：私有网段被放行，与 Service 目标一贯的行为保持一致——多数 webhook 合法地指向内部制品库，或只能通过私有网络访问的 SaaS。

被拒绝的地址会导致该次调用连接失败（WebhookKeyGet 不写入值，与其他任何 webhook 失败一样）。拒绝会使 `edgion_outbound_egress_blocked_total{reason="private"|"loopback"|"link_local"}` 计数器递增，但这仅在**主机名**路径上成立（`EgressResolver`，会记录全部三种 reason）。在 **IP 字面量**路径上，只有 `reason="private"` 会被记录：`dial_url` 的 `blockPrivate` 检查会记录该指标，但那条路径上的回环/链路本地拒绝要经过早于该指标就已存在的共享后端守卫（`validate_endpoint_backend_addr`），并不会写入该指标。因此，基于该计数器搭建的看板或告警，对主机名目标能获得完整覆盖，但当 `target.url` 是 IP 字面量时，对回环/链路本地拒绝是"看不见"的。

**网关本地出站设置。** 当 webhook 未显式设置 `target.blockPrivate` 时，生效值来自网关自身的配置。同一配置块还管理 Gateway 主动发起的 reqwest 调用所共用的 HTTP/2 响应头上限：

| 配置项 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `outbound_egress_block_private`（网关配置文件）/ `EDGION_OUTBOUND_BLOCK_PRIVATE`（环境变量） | Boolean | `false` | 受保护出站目标的网关级默认值。Webhook 可用 `target.blockPrivate` 覆盖；Redis 不可覆盖。真值：`1`、`true`、`yes`、`on`（大小写不敏感）。 |
| `outbound_egress_dns_cache_ttl_ms`（网关配置文件）/ `EDGION_OUTBOUND_DNS_CACHE_TTL_MS`（环境变量） | Integer（毫秒） | `5000` | 受保护出站 DNS 结果（包括 Redis）在分类后、重新解析前的缓存时长。环境变量值无法解析时会被忽略（记录 WARN）并保留已配置的值。 |
| `outbound_http2_max_header_list_size`（网关配置文件） | Integer（字节） | `16384` | Gateway 主动发起的 reqwest 调用共用的 HTTP/2 解码响应头列表上限。有效范围为 `16384..=65536`，修改后需要重启 Gateway；不影响客户端到 Gateway 的流量或 Pingora 路由 upstream。 |

**优先级：资源字段优先于网关默认值。** webhook 上显式设置的 `target.blockPrivate` 总是覆盖网关本地默认值——希望全局采用更严格策略的集群管理员可以打开 `outbound_egress_block_private`；需要访问内部 `10.x` 制品库的个别 webhook 仍可用 `target.blockPrivate: false` 单独放行，反之亦然。

**没有命令行开关，是有意的。** 除非上表列出了环境变量，否则这些配置项仅能通过配置文件设置；没有对应的命令行开关。网关的配置合并逻辑只会把一小组固定字段从命令行带入基于文件加载的配置中，而 `--config-file` 在常见部署路径下默认指向一个真实文件，因此该路径下的命令行开关会被静默忽略。请改用网关配置文件或上表列出的 `EDGION_*` 环境变量。

**这是纵深防御，而非主要控制手段。** `target.blockPrivate` 与 `outbound_egress_block_private` 收窄的是一次已经具备特权（LinkSys/EdgionPlugins 写权限）的写操作所能触达的范围，而不能替代网络层隔离。主要的出站流量控制手段仍然是作用于 Gateway Pod 的 Kubernetes `NetworkPolicy`，限制其可触达的外部 IP/端口范围——之所以还需要代码层防护，是因为 URL 目标允许运营者按主机名指定任意外部主机，而这是 `NetworkPolicy` 在配置编写时无法评估的。

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
        template: "Bearer ${secretRef:production/webhook-token:token}"

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
| `request.{args,headers,cookies}.custom` | Array | `[]` | 使用字面值或模板来源构造的字段。 |

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
| `required` | webhook 调用被视为失败：WebhookKeyGet 不写入值，与其他任何 webhook 失败一样。 | 后端缺之不可的字段。 |

每次出站调用中缺失的 `warnIfMissing` 字段合并为一条 `warn outbound field missing` PluginLog 记录，受插件日志的正常容量上限约束。记录不包含字段名或值，重试不会重复记录。WebhookKeyGet 的每个变量分别发起调用。若后续构造步骤或外部交换失败，已经发现的缺失告警仍会保留；在字段检查前停止的调用不会产生缺失告警。

`statusOnError` 存在于该连接器的 schema 中（它与 ForwardAuth 插件共用 `ExternalHttpConnection`），但**webhook 连接器本身并不读取它**：任何失败——包括某个 `presence: required` 字段缺失——WebhookKeyGet 都不写入值（或写入其 `default`），请求继续往下走。在 webhook `LinkSys` 上设置此字段没有任何效果；它只在同一个结构体被直接嵌入 ForwardAuth 插件配置时才有意义。

#### 自定义字段来源

每个 `custom` 条目为 `{ name, <source> }`，其中 source 是以下之一（**且仅一个**）：

| 来源 | 形式 | 解析时机 | 说明 |
|------|------|----------|------|
| 字面 | `value: "edgion"` | 无 | 逐字的静态字符串。 |
| 模板 | `template: "${ctx:user_id}"` | 调用时 | 由 ctx-set 模板引擎插值（见[模板变量](#模板变量)）。 |
| Secret 模板 | `template: "Bearer ${secretRef:namespace/name:key}"` | 配置时 + 调用时 | 控制器解析 Secret 并通过 conf_sync 下发真实值；管理接口与配置检查输出会将其脱敏。网关在每次调用时将其代入预编译模板。 |

在模板中引用 Secret 材料（不再有类型化的 `auth` 块或 `secret:` 自定义来源）：

```yaml
custom:
  - name: Authorization
    template: "Bearer ${secretRef:production/webhook-token:token}"
```

模板中的字面文本用于添加 `Bearer `、`Basic ` 等前缀。Secret 轮换是自动的：当被引用的 Secret 变化时，控制器重新入队该 LinkSys 资源、重新解析值并重新下发到网关——无需运营者操作。

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
| `${remoteIp}` | 解析后的客户端 IP。 |

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
| `timeoutMsTemplate` | String | none | 渲染单次请求超时的模板，如 `"${ctx:desired_timeout_ms}"`。仅限一组受信任的变量白名单——详见下文 [Templates（模板）](#模板-templates)。 |
| `tls` | Object | none | TLS 配置；详见 [TLS](#tls)。未设置时使用系统 CA 包。 |
| `success` | Object | 任意 2xx | 成功判定条件：状态码加上可选的 JSON body 断言。**取代已移除的 `successStatusCodes`**——详见下文 [Success（成功判定）](#成功判定-success)。 |
| `maxResponseBytes` | Integer | `1024` | 读取的最大响应体大小（字节），受 `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes`（默认 16 KiB）限制。当设置了 `success.body` 断言时必须 `>= 4096`——详见 [Success（成功判定）](#成功判定-success)。 |

> **已移除的字段。** `requestMethod`、`auth` 和 `defaultHeaders` 已弃用。在滚动升级期间它们仍被接受但运行时忽略，控制器在遇到时记录 WARN。它们将在下一个版本中移除——请在此之前迁移。详见[从旧 schema 迁移](#从旧-schema-迁移)。
>
> `successStatusCodes` 不属于这条弃用路径，也没有运行时兼容 shim。它会被忽略，但 Controller 会通过上述提示性未知字段界面报告。详见 [Success（成功判定）](#成功判定-success)。

### Success（成功判定）

取代旧的 `successStatusCodes` 字段（已移除；预发布阶段，不设别名——详见本节开头的警告）。只有当状态码检查**和**可选的 body 断言**同时**通过时，响应才被判定为成功：

```yaml
success:
  statusCodes: [200]          # 省略 -> 任意 2xx（200-299），与之前行为一致
  body:                       # 可选；与 statusCodes 按与运算组合
    - pointer: "/code"        # 指向已解析响应 body 的 RFC 6901 JSON Pointer
      equals: 0
    - pointer: "/data/status"
      in: ["ok", "accepted"]
```

动机：一些后端总是返回 `200 OK`，把真实结果放在 body 里，例如 `{"code": -101, "message": "invalid user"}`。没有 body 断言时，网关会把这种响应当作成功，重试和降级都不会触发。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `success.statusCodes` | Array\<Integer\> | 任意 2xx | 视为成功的状态码。省略 → 任意 2xx（200-299）。显式的空列表（`statusCodes: []`）会在**准入时被拒绝**，而不是静默地等同于"任意 2xx"——因为二者效果相同却少了可见性，例如 Helm 模板渲染出空列表会在无人察觉的情况下放宽鉴权白名单。 |
| `success.body` | Array\<Object\> | `[]` | JSON-Pointer body 断言，与状态码检查、以及断言之间彼此按与运算组合。 |
| `success.body[].pointer` | String | **必填** | 指向已解析响应 body 的 RFC 6901 JSON Pointer，如 `/code` 或 `/data/status`。 |
| `success.body[].equals` / `.notEquals` / `.exists` / `.in` | — | — | 每项恰好一个操作符（在反序列化时强制校验——多于一个或一个都没有都会解析失败）。`equals` / `notEquals` / `in` 按严格的 `serde_json::Value` 比较——**不做类型转换**：`equals: 0` 匹配 JSON 的 `0`，但不匹配字符串 `"0"`。`exists: true/false` 仅检查是否存在。`in` 接受一个非空的取值列表。 |

**非 JSON body → 判定为失败，而非报错。** 当配置了任意 `success.body` 断言，且响应 body 不是合法 JSON（`Content-Type` 不对或无法解析）时，响应被判定为**失败**——不 panic，不报错。

**指针缺失 → 判定为失败，`exists: false` 除外。** 当 body 确实是合法 JSON，但某条断言的 `pointer` 指向不存在的位置时，`equals`/`notEquals`/`in` 都会判定为失败——不能对一个根本不存在的字段做相等/成员关系断言。唯一合法的例外是 `exists: false`，它的定义就是"该指针不存在"，因此在这种情况下会成功。例如，`{pointer: /error, notEquals: true}` 应用于 `200 {"authenticated": false}`（完全没有 `/error` 键）的响应体上，结果是**失败**，而不是通过——一个隐藏在不同字段名下的业务错误，不能借由一个指向别处字段的 `notEquals` 断言蒙混过关。

**使用 body 断言时必须调高 `maxResponseBytes`。** 默认上限是 1024 字节（1 KiB）。若 `success.body` 非空，准入时要求 `maxResponseBytes >= 4096`，否则资源会被拒绝并报 `Accepted=False`。原因是：超过上限的 body 会在分类**之前**就被截断——截断的 body 会呈现为独立的 `ResponseTooLarge` 失败，**而不是**"断言未通过"。排查时请区分二者：`ResponseTooLarge` 失败意味着要调高 `maxResponseBytes`；断言失败意味着后端的响应确实不匹配。

**不影响被动健康检查。** 被动健康检查（`healthCheck.passive.failureStatusCodes`）仍然只看 HTTP 状态码——200 响应体里的业务错误不会触发连接器的健康状态变化或熔断。

### 重试

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `retry.maxRetries` | Integer | `1` | 最大重试次数（不含首次调用）。范围 `0..=10`——更大的值在准入时被拒绝（`Accepted=False`）。这条限制与下方 `maxDelayMs` 的上限共同存在：即便每次退避都有上限，不受限的 `maxRetries`（例如 200）仍会让一个存活的数据面请求内联阻塞长达数十分钟的 `tokio::time::sleep`。 |
| `retry.retryDelayMs` | Integer | `100` | **退避曲线的基础延迟**（毫秒）——不是固定的"重试间隔"。在 `backoffPolicy: fixed` 下它就是恒定延迟；在 `exponential`（默认）下它是曲线的第一项。必须大于 0。 |
| `retry.backoffPolicy` | Enum | `exponential` | `exponential` \| `fixed`。`exponential`：`delay = min(maxDelayMs, retryDelayMs << (attempt-1))`。`fixed`：`delay = retryDelayMs`（每次恒定）。 |
| `retry.maxDelayMs` | Integer | `1000` | 单次退避的上限（毫秒），在抖动**之前**应用。范围 `1..=10000`——更大的值在准入时被拒绝（`Accepted=False`）。重试的休眠会内联阻塞数据面上一个活跃的客户端请求，因此这是硬性安全上限，而不是用于等待故障恢复的调节旋钮（那是健康检查/熔断器的职责）。 |
| `retry.jitter` | Boolean | `true` | 为每次退避延迟增加 50%–100% 的抖动（`d = uniform(d/2 ..= d)`，在 `maxDelayMs` 上限**之后**应用，因此结果永远不会超过该上限）。防止重试风暴。 |
| `retry.honorRetryAfter` | Boolean | `false` | 可选启用。当响应状态码为 `429` 或 `503`，**且**该状态码已列在 `retryOnStatus` 中时，采用响应的 `Retry-After` 头（同时支持 `delta-seconds` 和 HTTP-date 两种形式）代替计算出的退避值。解析值 `<= maxDelayMs` 时用作本次尝试的延迟；解析值 `> maxDelayMs` 时网关会**停止重试**并将后端的响应原样返回——既不会立即重试，也不会报错。缺失或无法解析的 `Retry-After` 会回退到计算出的退避值。`honorRetryAfter` 只改变重试发生的**时机**，不改变是否重试的**决定**。 |
| `retry.retryOnTimeout` | Boolean | **按消费方决定——见下文** | 请求超时时重试。 |
| `retry.retryOnConnectError` | Boolean | `true` | 连接错误（拒绝、重置）时重试。连接失败意味着请求从未到达后端，因此重试总是幂等安全的——这个默认值不受下文消费方拆分的影响。 |
| `retry.retryOnStatus` | Array\<Integer\> | `[]` | 触发重试的 HTTP 状态码。状态码重试保持可选启用。 |
| `retry.retryOnBodyFailure` | Boolean | `false` | 当 `success.body` 断言（见 [Success（成功判定）](#成功判定-success)）将响应判定为失败时重试。默认关闭：像 `{"code": -101}` 这样的业务错误，第十次尝试仍然是 `-101`——重试纯属浪费。只有确认 body 失败是瞬时性的才应启用。 |

省略 `retry` 时，webhook 在首次出错后立即失败。

> **`retryOnTimeout` 的默认值已改变，现在按消费方决定：webhook 为 `false`，ForwardAuth 为 `true`。** 超时意味着结果*未知*——后端可能已经完整处理了请求。webhook 调用绝大多数是非幂等的 POST（发送通知、扣款、写审计记录），因此对超时的 webhook 重试有导致重复执行的风险——所以默认 `false`。ForwardAuth 调用是幂等的鉴权检查，对瞬时超时重试是真实的可用性收益——所以默认 `true`。**这是一次真实的行为变更**：任何设置了 `retry:` 块、且此前依赖旧的无条件 `true` 默认值的 webhook LinkSys 都会受影响；运营者始终可以显式覆盖该字段。`retryOnConnectError` 不受影响（两者都保持 `true` 不变），因为连接失败总是可以安全重试的。

### Templates（模板）

有一个连接层字段可以由 `request.*` 所用的同一套单次请求模板引擎驱动（见[模板变量](#模板变量)）——但它的变量白名单比 `request.*` **窄得多**。它会在准入时编译并校验；格式错误的模板，或引用了不允许的变量类型的模板，都会被拒绝并报 `Accepted=False`。

> ⚠️ **`timeoutMsTemplate` 只接受 `${ctx:...}`、`${remoteIp}`、`${matchedHttpRoute:...}` 和 `${matchedGrpcRoute:...}`。** `${header:...}`、`${query:...}`、`${cookie:...}`、`${path}`、`${uri}`、`${requestPath}`、`${requestUri}`、`${method}` 以及 `${secretRef:...}` 在这个字段上都会被专门拒绝（它们在 `request.*` 中仍然完全可用）。该字段是安全敏感的：`timeoutMsTemplate` 设置鉴权服务调用的截止时间。一个客户端可控的来源会让攻击者把超时压到小到必然超时的值（可靠的拒绝服务）。

```yaml
timeoutMsTemplate: "${ctx:desired_timeout_ms}"     # 回退到 timeoutMs
```

**规则：模板渲染为可解析的值时优先采用；否则回退到静态字段。**

| 参数 | 渲染为 | 回退目标 | 说明 |
|------|--------|----------|------|
| `timeoutMsTemplate` | `u64` 毫秒数 | `timeoutMs` | 渲染结果非数字、为空或低于下限时回退。下限是 `MIN_OUTBOUND_TIMEOUT_MS` = 50 毫秒（低于该值的渲染结果与 0/空一视同仁）。无论如何，渲染值在上限侧都会被**限幅到 60000 毫秒（60 秒）**。 |

每次静默回退都会使 `edgion_outbound_template_fallback_total{field="timeout_ms"}` 计数器递增——详见下文 [Observability metrics（可观测性指标）](#可观测性指标)，因为数据面无法通过 `tracing` 记录该回退。

**总阻塞时间。** 重试休眠和出站调用都内联在请求路径上，因此最坏情况的阻塞时间是 `(有效超时 × 最大尝试次数) + Σ 退避延迟`，每一项都独立设有上限：`timeoutMsTemplate` 受 60 秒限幅（以及下限的 50 毫秒下限）约束，退避延迟受 `maxDelayMs` 及其 10 秒硬上限约束，`maxRetries` 本身也受其 10 次的上限约束（见上方 [重试](#重试)）。

该字段不能重定向出站连接——本版本没有模板化的 `target.url`——而且有了上面的变量白名单，它也不能被这次调用所处理的客户端请求中的任何内容驱动。

### Observability metrics（可观测性指标）

Webhook 连接器（以及共用同一个 `ExternalHttpConnection` 的 ForwardAuth 插件）会输出两个 Prometheus 计数器，用于覆盖那些在数据面上原本不可见的结果（数据面无法通过 `tracing` 记录日志）：

| 指标 | 标签 | 触发条件 |
|------|------|----------|
| `edgion_outbound_template_fallback_total` | `field` ∈ `timeout_ms` | `timeoutMsTemplate` 渲染结果无法解析（或渲染为 0 或空），连接回退到静态字段。 |
| `edgion_outbound_success_predicate_fail_total` | 无 | 响应状态码已被 `success.statusCodes` 接受，但 `success.body` 断言将其判定为失败——即"200 OK 但 body 里有错误"的情形。 |

关于 Prometheus 的通用参考（抓取配置、基数、Gateway 指标监听端口），见 [Metrics](../observability/metrics.md)。

### 限流

防止外部 webhook 服务被网关压垮。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `rateLimit.rate` | Integer | **必填**（设置 `rateLimit` 时） | 每时间窗口允许的最大调用次数。 |
| `rateLimit.windowSec` | Integer | `1` | 时间窗口大小（秒）。 |

限流按每个 Gateway 客户端实例独立计算，使用同一客户端的调用共享额度，
在外部调用重试之前检查一次。采用双桶滑动窗口近似：当前桶计数与逐渐衰减的
上一桶计数之和必须小于 `rate`，才允许调用。这不是精确记录每次请求的滚动
窗口，也不是集群级配额。窗口使用秒级精度的单调运行时间，最小为一秒，
系统时钟调整不会改变窗口。连续两个或更多窗口间隔没有获准调用后，过期计数不再参与计算。

并发调用会同时更新窗口并消耗额度。延迟调用若发现其采样窗口已被替换，
会按限流拒绝。每桶最多支持 1,099,511,627,775 次获准调用，即使 `rate` 更大；
达到此上限后拒绝该桶的后续调用，直到窗口推进。窗口编号耗尽时同样拒绝调用，
不会重置计数。这些语义也适用于其他插件使用的共享 outbound 客户端 `rateLimit` 配置。

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
    target: { kind: Service, name: authz, port: 8080 }
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
    target: { kind: Service, name: audit-sink, port: 8080 }
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

通过在模板中嵌入 `${secretRef:namespace/name:key}` 注入凭据。

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: key-service, port: 8080 }
    request:
      method:
        template: POST
      headers:
        custom:
          - name: Authorization
            template: "Bearer ${secretRef:default/webhook-token:token}"
          - name: X-API-Key
            template: "${secretRef:default/webhook-apikey:apiKey}"
```

#### 5. 模板变量替换

在调用时从传入请求数据和网关上下文计算出站值。

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: enrich, port: 8080 }
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
            template: "${remoteIp}"
```

#### 6. 存在性策略（required / skipIfMissing / warnIfMissing）

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: authz, port: 8080 }
    request:
      headers:
        origin:
          - name: X-Trace-Id                       # 默认：skipIfMissing
          - name: X-User-Id
            presence: required                     # 缺失 → WebhookKeyGet 不写入值
          - name: X-Org-Id
            presence: warnIfMissing                # 缺失 → PluginLog 告警，继续
```

#### 7. 路径覆盖（allowOverride）

```yaml
spec:
  type: webhook
  config:
    target: { kind: Service, name: user-resolver, port: 8080 }
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
    target: { kind: Service, name: user-service, port: 8080 }
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
# 消费方插件，位于 HTTPRoute filter 上。
# CtxSet 必须先运行：webhook 模板读取 ${ctx:user_id} / ${ctx:action}。
- type: CtxSet
  config:
    vars:
      - { name: user_id, template: "${header:X-User-Id}" }
      - { name: action, value: "get-user" }
- type: WebhookKeyGet
  config:
    vars:
      - name: resolved_user
        webhookRef: production/user-resolver
        pathOverride: "/special/path"   # 仅因 allowOverride: true 才被采纳
        extract:
          source: body
          path: "data.user"
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
            template: "Bearer ${secretRef:production/webhook-token:token}"
    timeoutMs: 3000
    retry:
      maxRetries: 1
      retryDelayMs: 100          # 退避曲线的基础延迟，不是固定间隔
      backoffPolicy: exponential # exponential（默认）| fixed
      maxDelayMs: 1000           # 单次退避上限，范围 1..=10000
      jitter: true               # 50%-100% 抖动，在 maxDelayMs 上限之后应用
      retryOnTimeout: false      # webhook 的默认值；forward_auth 默认改为 true
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
| `target.type: uri` + `target.uri` | 仅包含 origin 的 `target.url`；将旧 URI 的路径移到 `request.path.template` |
| `target.type: service` + `target.name/port` | 直接写在 `target` 下的 `target.{kind,name,namespace,port}`（去掉 `type` 标签） |
| `auth.bearer` / `auth.basic` / `auth.hmac` / `auth.apiKey` | 在 `request.headers.custom[].template` 中使用 `${secretRef:namespace/name:key}` 并添加所需字面前缀 |
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
      url: "https://key-service.internal"
    request:
      path:
        template: /resolve
      method:
        template: POST
      headers:
        custom:
          - name: X-Source
            value: edgion
          - name: Authorization
            template: "Bearer ${secretRef:production/webhook-token:token}"
```

消费方侧，`pathSuffix` 和 `forwardHeaders` 已移除。面向运维的消费方是 `WebhookKeyGet` 插件（不是 KeyGet 的 `type: webhook` 来源）：

```yaml
# 旧
- type: key-auth
  config:
    keys:
      - type: webhook
        webhookRef: production/user-resolver
        pathSuffix: "/special/path"
        forwardHeaders: ["X-Tenant-Id", "X-User-Id"]

# 新——在 webhook 资源上声明 path/headers；WebhookKeyGet 写入 ctx；
# KeyAuth（或任何后续插件）读取 { type: ctx, name: … }。
# WebhookKeyGet 必须排在 KeyAuth 之前；把列表写反是静默认证绕过。
- type: WebhookKeyGet
  config:
    vars:
      - name: resolved_key
        webhookRef: production/user-resolver
        pathOverride: "/special/path"   # 仅当 webhook 上 allowOverride: true 时被采纳
        extract:
          source: body
          path: "data.key"
- type: KeyAuth
  config:
    keySources:
      - type: ctx
        name: resolved_key
```

> **外部端点。** 使用仅含 origin 的 `target.url`，路径写入
> `request.path.template`。不支持 `kind: ExternalName`；URL 目标受连接器的
> SSRF 策略和可选 `blockPrivate` 限制保护。

### Webhook 故障排查

**Webhook 无法解析 / "webhook not found"。**
检查 `target` 下的 Service 字段（`name`/`namespace`/`port`/`kind`）：命名的 Service 必须存在于解析出的命名空间中，且 `port` 必须已设置且有效。`Accepted=False` 的 LinkSys 不会加载进网关运行时，因此任何调用它的消费方都会命中相同的"webhook 不可用"路径。运行 `kubectl get linksys <name> -o yaml` 并阅读 `Accepted` 条件消息。

**关于 `kind` 值的 `Accepted=False`。**
只有 `Service`、`ServiceClusterIp`、`ServiceEndpointSlice` 和留空有效。`ExternalName` 和 `ServiceImport` 被拒绝——请改用代理 Service。

**Required 字段错误 / key 解析结果为空。**
某个 `presence: required` 的 `origin` 字段在传入请求中缺失，这算作 webhook 失败：WebhookKeyGet 不写入值（或写入其 `default`）。请确认客户端确实发送了该 header/arg/cookie（注意大小写：请求头大小写不敏感，arg 和 cookie 大小写敏感），或用 `presence: warnIfMissing` 仅记录而不失败。

**模板变量解析为空。**
`${ctx:NAME}` 在变量从未设置时返回空。确保在 webhook 被调用之前有一个 ctx-set 步骤设置它，并检查 `${type:name}` 拼写。要强制存在性，优先在 `origin` 字段上用 `presence: required` 而非模板。

**路径覆盖不生效。**
`pathOverride` 仅当 webhook 的 `request.path.allowOverride` 为 `true` 时被采纳。如果 `path` 被省略或 `allowOverride` 为 `false`，覆盖被静默忽略，使用模板（或冻结的传入路径）。

**关于重复或冲突字段的准入错误。**
你在 `origin` 和 `custom` 中命名了同一字段、在一个列表内列了重复名称，或同时声明了 `headers.origin[name: Cookie]` 和一个 `cookies.*` 列表。移除冲突。

**健康状态重置 vs. 端点变动。**
编辑任何 `request.*` 字段（或目标）会重建 webhook 运行时并**重置**其健康状态——这是有意的，运营者的配置变更不等于 Pod 变动。EndpointSlice 变化（Pod 扩缩/重启）**不会**重建运行时，也**不会**重置健康状态：端点在调用时重新解析，而聚合健康模型被保留。

## Kafka 连接器

Kafka 是已上线的 Gateway provider，具有强类型 runtime store、full/partial
热更新和后台健康恢复。每个异步日志输出通过 `KafkaLogSender` 选择一个 topic，
被引用的 LinkSys 负责共享客户端和待发送数据的总预算。

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
    channelSize: 4096
    maxTopics: 64
    maxPendingRecords: 4096
    maxPendingBytes: 16777216
    lingerMs: 5
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `brokers` | Array\<String\> | **必填** | Kafka broker 地址（`host:port`），集群时列出多个。 |
| `sasl` | Object | none | 可选 SASL/PLAIN 用户名与 Secret 支持的密码槽。 |
| `tls` | Object | none | 可选的共享 LinkSys TLS 策略；启用 Kafka TLS 时始终验证对端。 |
| `channelSize` | Integer | `4096` | 每个 topic 的非阻塞 producer 队列容量。 |
| `maxTopics` | Integer | `64` | 单个 Kafka LinkSys 客户端最多持有的 topic producer 队列数。 |
| `maxPendingRecords` | Integer | `4096` | 所有 topic 中排队或正在发送的记录总数上限。 |
| `maxPendingBytes` | Integer | `16777216` | 所有 topic 中排队或正在发送的 UTF-8 载荷总字节数上限。它是逻辑载荷限制，不包含分配器、channel 节点、task 和 Kafka SDK 的额外开销。 |
| `lingerMs` | Integer | `5` | Kafka 批处理等待时间，必须小于 30 秒 producer 超时时间。 |

所有队列限制都必须大于零。达到任一总限额时，新日志会通过现有非阻塞发送路径被丢弃，
Kafka 客户端健康状态保持不变。增大 `channelSize` 不会增大两个待发送数据总限额。

在 Gateway 日志配置中使用
`kafka: { linkSysRef: { namespace: production, name: events-kafka }, topic: <topic> }`
选择 Kafka sink。独立集成测试入口为
`edgion-tests/integration/scripts/integration/link_sys/run_kafka_test.sh`。

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

CA 包通过 Secret 引用提供（`validation.caCertificateRefs`），客户端证书和私钥来自另一个 Secret（`clientCertificateRef`）。注意：本版本已移除内联 CA（`caCert` 字段），请改用包含 `ca.crt` 键的 Secret。

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
      validation:
        caCertificateRefs:
          - kind: Secret
            name: etcd-ca-bundle   # 包含 ca.crt 键的 Secret，例如 kubectl create secret generic etcd-ca-bundle --from-file=ca.crt=ca.pem
      clientCertificateRef:
        name: etcd-client-tls
```

## 常见问题

### Q1：为什么我的 LinkSys 报告 `Accepted=False/Invalid`？

**A**：结构或后端特定校验失败。常见原因包括 Redis endpoint 语法、基数或传输模式无效，`topology.mode=sentinel` 但缺少有效的 `sentinel` 块，Etcd/Elasticsearch endpoint 为空，webhook 的 `target` Service 引用缺少/为零的 `port` 或使用不支持的 `kind`，以及 webhook 请求字段互相冲突。状态 `message` 会列出具体错误，但不会回显 Redis endpoint 输入。

### Q2：凭据看起来正确，但 `ResolvedRefs` 为 `False`。为什么？

**A**：Secret 无法解析。请检查原因：

- `SecretNotFound`——Secret 尚不存在。Controller 在 Secret 创建后会自动重新入队。
- `RefNotPermitted`——跨命名空间引用但没有匹配的 `ReferenceGrant`。
- `InvalidSecretType`——`Secret.type` 不在该认证类型解析器的允许列表中。
- `MissingKeys`——Secret 缺少必需键（例如 basic auth 缺少 `password`，apiKey 缺少 `apiKeyId` / `apiKeySecret`）。

### Q3：能直接在 LinkSys YAML 中写密码吗？

**A**：不能。认证和 TLS 结构体开启了 `deny_unknown_fields`，内联敏感字段（`username`、`password`、`bearerToken`、`apiKeyId`、`hmacKey`、`clientKey` 等）会导致解析失败而非被忽略。请始终使用 `secretRef`。

### Q4：能否为 Redis 或 Etcd 禁用 TLS 验证（`verify: false`）？

**A**：不能。Redis 和 Etcd 客户端不支持禁用证书验证——在任何环境下，启用 TLS 块中的 `verify: false` 均会在启动时被拒绝。停用块中的 `verify` 设置会被忽略，无法关闭 URL 隐含 TLS 连接的证书验证。如需连接使用私有 CA 的后端，请启用 TLS，并通过 `tls.validation.caCertificateRefs` 引用包含 `ca.crt` 键的 Secret。

### Q5：轮换 Secret 后，现有连接会怎样？

**A**：Controller 会重新处理 LinkSys 资源，并将解析后的配置发送给 Gateway。如果连接器实际使用的凭据及其他运行时材料没有变化，现有客户端会继续复用；如果发生变化，Gateway 会启动替换流程。对于 Redis、Etcd、Elasticsearch、Kafka 和 OTLP，旧客户端仍有存活连接时，会保留旧客户端，直到新客户端成功初始化并发布，然后再将旧客户端退出；没有存活连接时，会先退出旧客户端，再尝试构建新客户端。Webhook 使用单独的替换流程。旧客户端退出时，不保证队列中的 Kafka 或 Elasticsearch 记录全部送达。

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
- [Metrics](../observability/metrics.md)

## OTLP/gRPC 连接器

使用 `type: otlp` 将日志记录发送到 OpenTelemetry Collector。一个 LinkSys
资源在访问日志、TCP、TLS 和 UDP 日志输出之间共享连接、exporter、批处理器及
TCP 健康探针。SSL 日志使用同步文件写入，不支持 OTLP。

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  namespace: observability
  name: collector
spec:
  type: otlp
  config:
    endpoint: https://collector.observability.svc:4317
    timeoutMs: 10000
    auth:
      secretRef:
        name: collector-auth
    tls:
      enabled: true
      validation:
        caCertificateRefs:
          - kind: Secret
            name: collector-ca
      clientCertificateRef:
        name: collector-client
```

在相同命名空间创建引用的 Secret：`collector-auth` 使用 `bearerToken` 键，
`collector-ca` 使用 `ca.crt` 键，`collector-client` 是包含 `tls.crt` 和 `tls.key`
的 `kubernetes.io/tls` Secret。不需要认证时省略 `auth`；使用 HTTPS 系统根证书时
省略整个 TLS 块；不需要客户端认证时省略 `clientCertificateRef`。自定义 CA 会替换
公共根证书。跨命名空间引用 Secret 需要 ReferenceGrant。认证仅支持来自 Secret 的
Bearer token，最多 8192 字节；任意 metadata 和内联凭据均不支持。

另行配置 Gateway 进程中的日志输出：

```yaml
access_log:
  enabled: true
  output:
    otlp:
      linkSysRef:
        namespace: observability
        name: collector
  fallback:
    localFile:
      path: logs/access-fallback.log
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `endpoint` | String | 必填 | `http://host[:port]` 或 `https://host[:port]`，可带末尾 `/`。省略端口时使用 80/443；常见 Collector 应显式填写 `:4317`。 |
| `timeoutMs` | Integer | 10000 | 完整导出操作的超时，单位毫秒，范围 1–300000。 |
| `auth.secretRef` | Object | 无 | 包含 `bearerToken` 的 Secret。 |
| `tls` | Object | 无 | 共享出站 TLS 策略及证书引用。 |

Endpoint 不允许 userinfo、非根路径、query、fragment、转义及空白；IPv6 地址必须
使用方括号。HTTP 不能启用本地 TLS 块。HTTPS 始终校验原始 endpoint 主机名，
不支持关闭证书校验、覆盖主机名或 SAN 策略。禁用本地 TLS 块只会忽略该块，
HTTPS 仍使用 TLS 和全局校验策略。

每次连接和探针均对实际解析得到的地址执行 Gateway egress 策略。生产模式拒绝
loopback、link-local 和 unspecified 地址，也包括 IPv4-mapped IPv6。未启用全局
私网阻断时，允许私网或 unique-local Collector。代理环境变量和重定向不能改变目的地。

引用缺失、类型不匹配、初始化中或不健康时，会选择配置中的下一个健康日志输出；
全部不健康时丢弃日志并增加已有丢弃计数器。引用的连接故障不会阻止 Gateway 启动。
非法 namespace/name 会使进程配置失败；所有输出和 fallback（包括已禁用的输出）
都必须满足 Kubernetes 名称语法及长度限制。

热更新期间，替换配置无效、引用未解析或首次空 OTLP Export 失败时保留旧的可用连接。
首次检查在三秒总时限内确认 Collector 认证和 TLS 接受结果。合法的待连接配置会自动重试。删除资源会停止 exporter 和探针；进程退出时，每条管线最多用五秒排空记录，
随后取消未完成的导出并等待线程退出。已有退役管线先完成清理，再清理当前管线。这是尽力交付，不提供持久重放，也不会在一次发送失败后改投
第二个输出。

`edgion_link_sys_health{link="observability/collector"}` 及其状态切换计数器使用
LinkSys key。Gauge 在每次指标采集时读取已发布的客户端。健康仅表示 TCP 可达，不表示认证成功、每次重连的 TLS 握手成功或
日志已交付；端到端验证应检查 Collector 收到的记录。可能暴露 endpoint 或证书细节
的 SDK 传输日志即使在 `RUST_LOG=trace` 下也会被抑制。

此连接器仅支持 OTLP/gRPC 日志导出，没有 `protocol` 字段，也不保留旧内联 OTLP
输出格式。不提供 OTLP/HTTP、trace 或应用指标导出。
