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
| basic（Redis / Etcd / ES / Webhook） | `kubernetes.io/basic-auth` 或 `Opaque` | `password` | `username` |
| apiKey（Elasticsearch） | `Opaque` | `apiKeyId`、`apiKeySecret` | — |
| bearer（Elasticsearch / Webhook） | `Opaque` | `bearerToken` | — |
| hmac（Webhook） | `Opaque` | `hmacKey` | — |
| apiKey（Webhook） | `Opaque` | `apiKey` | — |
| TLS（所有后端） | `kubernetes.io/tls` 或 `Opaque` | `tls.crt`、`tls.key`、`ca.crt` 中至少一个 | — |

应用层键名使用 camelCase；TLS 键名遵循 Kubernetes `kubernetes.io/tls` 标准。

### TLS

每个后端均接受相同的 `tls` 块用于保护连接。

```yaml
tls:
  enabled: true
  insecureSkipVerify: false
  certs:
    # CA 证书和客户端证书属于公开材料，可以内联。
    caCert: |
      -----BEGIN CERTIFICATE-----
      <base64-encoded-cert>
      -----END CERTIFICATE-----
    clientCert: |
      -----BEGIN CERTIFICATE-----
      <base64-encoded-cert>
      -----END CERTIFICATE-----
    # 客户端私钥（tls.key）必须来自 Secret。
    secretRef:
      name: my-client-tls
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | Boolean | `false` | 为连接启用 TLS。对于已隐含 TLS 的 URL（`https://`、`rediss://`），此字段仅添加自定义 CA / 客户端证书处理。 |
| `insecureSkipVerify` | Boolean | `false` | 跳过证书验证。⚠️ **仅限开发和测试。** Redis 和 Etcd 不支持此选项——设置为 `true` 会直接拒绝启动。 |
| `certs.caCert` | String | none | CA 证书 PEM（公开材料，可内联）。 |
| `certs.clientCert` | String | none | mTLS 客户端证书 PEM（公开材料，可内联）。 |
| `certs.secretRef` | Object | none | 提供客户端私钥 `tls.key` 的 Secret 引用。**私钥必须来自 Secret，不可内联。** |

⚠️ **客户端私钥必须始终通过 `certs.secretRef` 提供。** 只有 CA 和客户端证书（公开材料）可以内联为 PEM。

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
      certs:
        secretRef:
          name: etcd-client-tls
    namespace: "/my-app/"
```

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `endpoints` | Array\<String\> | **必填** | Etcd 端点，至少一个。示例：`"https://etcd-1:2379"`。 |
| `auth` | Object | none | 认证配置。Secret 通常同时提供 `username` 和 `password`。 |
| `tls` | Object | none | TLS 配置；详见 [TLS](#tls)。Etcd **不支持** `insecureSkipVerify`——设置为 `true` 会拒绝启动。 |
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

Webhook 连接器需要指定一个 `target`，即在外部 URI（`type: uri`）和 Kubernetes Service（`type: service`）之间的标签选择。

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
      type: uri
      uri: "https://key-service.internal/resolve"
    requestMethod: POST
    timeoutMs: 3000
    auth:
      type: bearer
      secretRef:
        name: webhook-token
    defaultHeaders:
      X-Source: edgion
    successStatusCodes: [200]
    maxResponseBytes: 2048
```

### 目标

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `target.type` | Enum | **必填** | 目标类型。可选值：`uri` \| `service`。 |
| `target.uri` | String | **必填**（`uri` 时） | 外部端点，必须以 `http://` 或 `https://` 开头。 |
| `target.name` | String | **必填**（`service` 时） | Kubernetes Service 名称。 |
| `target.namespace` | String | LinkSys 所在命名空间 | Service 所在命名空间。 |
| `target.port` | Integer | none | Service 端口，设置时必须大于 0。 |
| `target.kind` | String | `Service` | Service 类型，例如 `Service` 或 `ServiceClusterIp`。 |

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `requestMethod` | String | `GET` | HTTP 请求方法，必须为有效的 HTTP 方法。 |
| `timeoutMs` | Integer | `5000` | 请求超时（毫秒），必须大于 0。 |
| `auth` | Object | none | 认证配置（标签枚举），见下文。 |
| `tls` | Object | none | TLS 配置；详见 [TLS](#tls)。未设置时使用系统 CA 包。 |
| `defaultHeaders` | Object | none | 每次调用都发送的静态请求头（冲突时按调用时请求头优先）。 |
| `successStatusCodes` | Array\<Integer\> | 任意 2xx | 视为成功的 HTTP 状态码。 |
| `maxResponseBytes` | Integer | `1024` | 读取的最大响应体大小（字节），受 `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes`（默认 16 KiB）限制。 |

### 认证

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `auth.type` | Enum | **必填**（设置 `auth` 时） | 认证方式。可选值：`basic` \| `bearer` \| `hmac` \| `apiKey`。 |
| `auth.secretRef` | Object | **必填**（设置 `auth` 时） | 提供所选 `auth.type` 凭据的 Secret。 |
| `auth.in` | String | **必填**（`apiKey` 时） | 密钥注入位置。可选值：`header` \| `query`。 |
| `auth.headerName` | String | none | `in=header` 时的请求头名称。示例：`X-API-Key`。 |
| `auth.queryParam` | String | none | `in=query` 时的查询参数名称。示例：`api_key`。 |

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
      type: uri
      uri: "https://key-service.internal/resolve"
    requestMethod: POST
    timeoutMs: 3000
    auth:
      type: hmac
      secretRef:
        name: webhook-hmac
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

#### 示例：指向 Kubernetes Service 的 webhook

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
      type: service
      name: key-service
      namespace: production
      port: 8080
    requestMethod: POST
    timeoutMs: 3000
```

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

CA 和客户端证书内联（公开材料），客户端私钥来自 Secret。

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
      certs:
        caCert: |
          -----BEGIN CERTIFICATE-----
          <base64-encoded-cert>
          -----END CERTIFICATE-----
        clientCert: |
          -----BEGIN CERTIFICATE-----
          <base64-encoded-cert>
          -----END CERTIFICATE-----
        secretRef:
          name: etcd-client-tls
```

## 常见问题

### Q1：为什么我的 LinkSys 报告 `Accepted=False/Invalid`？

**A**：结构或后端特定校验失败。常见原因：`endpoints` 列表为空、`topology.mode=sentinel` 但缺少 `sentinel` 块（或 `mode=cluster` 但缺少 `cluster` 块）、webhook 的 `target.uri` 不以 `http://` 或 `https://` 开头、webhook 的 `requestMethod` 不是有效的 HTTP 方法。状态 `message` 字段列出了具体错误。

### Q2：凭据看起来正确，但 `ResolvedRefs` 为 `False`。为什么？

**A**：Secret 无法解析。请检查原因：

- `SecretNotFound`——Secret 尚不存在。Controller 在 Secret 创建后会自动重新入队。
- `RefNotPermitted`——跨命名空间引用但没有匹配的 `ReferenceGrant`。
- `InvalidSecretType`——`Secret.type` 不在该认证类型解析器的允许列表中。
- `MissingKeys`——Secret 缺少必需键（例如 basic auth 缺少 `password`，apiKey 缺少 `apiKeyId` / `apiKeySecret`）。

### Q3：能直接在 LinkSys YAML 中写密码吗？

**A**：不能。认证和 TLS 结构体开启了 `deny_unknown_fields`，内联敏感字段（`username`、`password`、`bearerToken`、`apiKeyId`、`hmacKey`、`clientKey` 等）会导致解析失败而非被忽略。请始终使用 `secretRef`。

### Q4：为什么将 `insecureSkipVerify: true` 会导致 Redis 或 Etcd 连接器崩溃？

**A**：Redis 和 Etcd 客户端不支持跳过验证——设置为 `true` 会直接拒绝启动。这是故意的设计，避免出现字段看似已设置但验证仍然发生的配置陷阱。如需连接使用私有 CA 的后端，请通过 `tls.certs.caCert` 或 `ca.crt` Secret 键提供 CA 证书。

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
