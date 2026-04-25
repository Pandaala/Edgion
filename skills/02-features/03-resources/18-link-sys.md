---
name: link-sys-detail
description: LinkSys CRD 完整 Schema：Redis/Etcd/Elasticsearch/Webhook/Kafka 连接器配置，统一 secretRef/TLS 凭据模型。
---

# LinkSys 资源

> API: `edgion.io/v1` | Scope: Namespaced | ShortName: `lsys`

LinkSys 是统一的外部系统连接器资源。通过顶层 `spec.type` 区分后端，`spec.config` 承载对应后端的详细配置。凭据全部走 `secretRef` 引用 K8s `Secret`，**不允许**在 CRD 里内联用户名、密码、token、`clientKey` 等敏感字段（会被 `deny_unknown_fields` 在反序列化阶段硬拒）。

## 顶层 Schema

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: my-redis
  namespace: default
spec:
  type: redis                 # redis | etcd | elasticsearch | webhook | kafka
  config:                     # 后端类型特定配置（下方按 type 展开）
    ...
```

`type` 与 `config` 是 `#[serde(tag = "type", content = "config")]` 的 tagged enum：YAML 里 `type: redis` 对应 `RedisClientConfig`，`type: webhook` 对应 `WebhookServiceConfig`，以此类推。旧版本把 `type` 放在 `spec.config.type` 里的写法已失效。

## 凭据模型（通用）

### Secret 引用：`SecretReference`

```yaml
secretRef:
  name: my-secret           # 必填
  namespace: credentials    # 可选；缺省=LinkSys 所在 namespace
```

- 跨 namespace（`namespace != 当前资源 namespace`）需要目标 namespace 下存在允许 `Secret` 的 Gateway API `ReferenceGrant`，否则 `ResolvedRefs=False/RefNotPermitted`。
- Secret 的键走 **well-known 约定**（下表），CRD 不支持自定义 `usernameKey` / `passwordKey` 这类字段。

### Secret 键约定（与 resolver 对齐）

| 认证类型 | 推荐 Secret.type | 必需 key | 可选 key |
|---------|------------------|----------|---------|
| basic（Redis/Etcd/ES/Webhook） | `kubernetes.io/basic-auth` 或 `Opaque` | `password` | `username`（缺省=后端默认 ACL） |
| apiKey（Elasticsearch） | `Opaque` | `apiKeyId`, `apiKeySecret` | — |
| bearer（Elasticsearch/Webhook） | `Opaque` | `bearerToken` | — |
| hmac（Webhook） | `Opaque` | `hmacKey` | — |
| TLS（所有 provider） | `kubernetes.io/tls` 或 `Opaque` | `tls.crt` / `tls.key` / `ca.crt` 三者**至少一个** | — |

所有应用层键名使用 **camelCase**；TLS 键沿用 K8s `kubernetes.io/tls` 标准（含点号）。

> **破坏性变更提示**：所有 `*Auth` 结构体已开启 `deny_unknown_fields`，CRD YAML 里写 `username: ...` / `password: ...` / `bearerToken: ...` / `apiKeyId: ...` / `clientKey: ...` 会直接解析失败（不会被静默忽略）。

### TLS 凭据结构

```yaml
tls:
  enabled: true
  insecureSkipVerify: false       # 仅开发/测试；Redis/Etcd 不支持，写 true 会直接拒绝启动
  certs:
    caCert: |                      # 公开材料，可内联
      -----BEGIN CERTIFICATE-----
      ...
    clientCert: |                  # 公开材料，可内联
      -----BEGIN CERTIFICATE-----
      ...
    secretRef:                     # 私钥 tls.key 必须走 secretRef
      name: my-client-tls
      # namespace 省略时同 LinkSys
```

- 客户端私钥 **只能**通过 `certs.secretRef` 提供，指向 `kubernetes.io/tls`（键 `tls.key`）或 `Opaque`（同名键）的 Secret。
- `caCert` / `clientCert` 是公开材料，保留 inline PEM 能力。
- 对于 URL 已经隐含 TLS 的 provider（`https://`、`rediss://`），`enabled: true` 只额外启用自定义 CA / 客户端证书处理；纯走系统 CA 时可不写 `tls`。

## 按 type 展开配置

### `type: redis`

```yaml
spec:
  type: redis
  config:
    endpoints:                    # 必填，至少一个
      - "redis://redis:6379"
    db: 0                         # 0-15，Redis 标准库编号
    auth:                         # 可选；存在则 secretRef 必填
      secretRef:
        name: redis-auth
    pool:
      size: 16
      minIdle: 2
    timeout:
      connect: 3000               # ms
      read: 2000
      write: 2000
    topology:                     # standalone（默认） | sentinel | cluster
      mode: sentinel
      sentinel:
        masterName: mymaster
        sentinels: ["sentinel-1:26379", "sentinel-2:26379"]
        password: "..."           # Sentinel 自身密码，独立于 Redis 节点鉴权
      # cluster:
      #   readFromReplicas: true
      #   maxRedirects: 6
    tls:
      enabled: true
      certs:
        secretRef: { name: redis-client-tls }
    observability:
      logging: { enabled: true, level: info, slowLogThreshold: 200 }
      metrics: { enabled: true, labels: { env: prod } }
```

- `topology.mode=sentinel` 时 `sentinel` 必填，`mode=cluster` 时 `cluster` 必填，否则 `Accepted=False/Invalid`。
- `auth` 使用 `BasicAuthResolver`：Secret 里至少提供 `password`；`username` 可选（Redis 6+ ACL）。

### `type: etcd`

```yaml
spec:
  type: etcd
  config:
    endpoints:
      - "https://etcd-1:2379"
      - "https://etcd-2:2379"
    auth:                         # 可选
      secretRef:
        name: etcd-creds
        namespace: credentials
    tls:
      enabled: true
      certs:
        secretRef: { name: etcd-client-tls }
    timeout:                      # ms
      dial: 5000
      request: 10000
      keepAlive: 30000
    keepAlive:                    # HTTP/2 keepalive，单位秒
      time: 30
      timeout: 10
      permitWithoutStream: true
    namespace: "/my-app/"         # etcd v3 key 前缀
    autoSyncInterval: 300         # 秒；0 关闭
    maxCallSendSize: 2097152
    maxCallRecvSize: 4194304
    userAgent: "edgion-gateway/v1"
    rejectOldCluster: true
    observability:
      logging: { enabled: true, level: info }
      metrics: { enabled: true, labels: { env: prod } }
```

- `auth` 同样使用 `BasicAuthResolver`（username + password，etcd 通常二者都必填）。
- etcd-client 不支持 `insecureSkipVerify`：显式设为 `true` 会直接 `bail!` 拒绝启动（与 Redis 对齐，避免"已忽略却继续严格验证"导致的配置错觉）。要跳过验证只能通过其他机制。

### `type: elasticsearch`

```yaml
spec:
  type: elasticsearch
  config:
    endpoints:
      - "https://es-1:9200"
      - "https://es-2:9200"
    auth:                         # tagged enum，必须带 type
      type: apiKey                # basic | apiKey | bearer
      secretRef:
        name: es-api-key
    tls:
      enabled: true
    timeout:
      connect: 5000
      request: 30000
    pool:
      maxIdlePerHost: 10
      idleTimeout: 90000          # ms
    bulk:
      batchSize: 500
      flushInterval: 5000
      maxRetries: 3
      backoffMs: 1000
      maxBodyBytes: 10485760      # 10 MiB
    index:
      prefix: "edgion-logs"
      datePattern: "%Y.%m.%d"
    failedCache:
      type: localFile             # localFile | redis
      localFile:
        path: "/var/log/edgion/es-failed"
      # redis:
      #   linkSysRef: "default/redis-cache"
      #   listKey: "es-failed"
      #   maxEntries: 100000
    compatibility:
      vendor: elasticsearch       # elasticsearch | opensearch
```

- Auth tagged-enum：`type: basic` → `BasicAuthResolver`、`type: apiKey` → `EsApiKeyResolver`、`type: bearer` → `BearerResolver`。
- 运行期使用 reqwest 直接打 HTTP，不依赖官方 `elasticsearch` crate；与 OpenSearch 兼容。

### `type: webhook`

```yaml
spec:
  type: webhook
  config:
    uri: "https://key-service.internal/resolve"   # 必填，http/https
    requestMethod: POST                            # 默认 GET
    timeoutMs: 3000
    auth:                                          # 可选
      type: bearer                                 # basic | bearer | hmac
      secretRef:
        name: webhook-token
    tls:
      enabled: true
      certs:
        secretRef: { name: webhook-client-tls }
    defaultHeaders:                                # 静态出站 headers，每次调用都带上
      X-Source: edgion
    successStatusCodes: [200]
    maxResponseBytes: 2048                         # 默认硬上限 16 KiB（可经 EdgionGatewayConfig.linkSys.webhookMaxResponseBytes 调整）
    allowDegradation: true                         # 故障时静默降级返回 None
    statusOnError: 503                             # allowDegradation=false 且 webhook 是唯一 key 来源时的返回码
    retry:
      maxRetries: 1
      retryDelayMs: 100
      retryOnTimeout: true
      retryOnConnectError: true
      retryOnStatus: [502, 503, 504]
    rateLimit:                                     # 对外部 webhook 的保护
      rate: 200
      windowSec: 1
    healthCheck:
      active:
        path: "/healthz"
        intervalSec: 10
        timeoutMs: 2000
        healthyThreshold: 1
        unhealthyThreshold: 3
      passive:
        unhealthyThreshold: 3
        failureStatusCodes: [500, 502, 503, 504]
        countTimeout: true
        # backoff:                                 # 仅 active 关闭时生效
        #   initialSec: 5
        #   multiplier: 2.0
        #   maxSec: 60
```

- Auth tagged-enum：`type: basic` → `BasicAuthResolver`、`type: bearer` → `BearerResolver`、`type: hmac` → `HmacResolver`（Secret 键 `hmacKey`）、`type: apiKey` → `WebhookApiKeyResolver`（Secret 键 `apiKey`，注入 header 或 query）。
- `uri` 必须 `http://` 或 `https://`；`requestMethod` 必须是合法 HTTP 方法；`statusOnError` ∈ [200, 599]；`maxResponseBytes` 在调用时 clamp 至 `EdgionGatewayConfig.linkSys.webhookMaxResponseBytes`（默认 16 KiB）。
- 调用语义（method/path/headers/body）由消费方决定：`KeyGet::Webhook` 通过 `forwardHeaders` 选择从入站请求转发哪些 header、`pathSuffix` 拼接到 `uri` 末尾；其他消费方通过 `get_webhook_call_handle()` + `WebhookCallRequest` 自定义。

### `type: kafka`

```yaml
spec:
  type: kafka
  config:
    brokers:
      - "kafka:9092"
```

- Kafka 目前是占位（schema 合法，Gateway 端尚未接入 provider）。`Accepted=True`，运行期是否可用视后续版本。

## Status Schema

```yaml
status:
  conditions:
    - type: Accepted              # spec 是否结构合法 + per-provider validate
      status: "True"
      reason: Accepted
    - type: ResolvedRefs          # 全部 secretRef / tls.secretRef 是否都已成功解析
      status: "True"
      reason: ResolvedRefs
```

**`Accepted`**：
- `True/Accepted` — 结构 + `validate_config()` 全过。
- `False/Invalid` — 有 validation error（endpoint 为空、Sentinel/Cluster 缺子配置、Webhook URI 非法等），`message` 是错误拼接。

**`ResolvedRefs`**：
- `True/ResolvedRefs` — 所有引用的 `Secret` 都已找到、`Secret.type` 合法、必需 key 全部存在且可解码。
- `False/SecretNotFound` — Secret 尚未出现在 apiserver（Controller 会随 Secret 创建事件自动 requeue）。
- `False/RefNotPermitted` — 跨 namespace 且没有匹配的 `ReferenceGrant`。
- `False/InvalidSecretType` — Secret.type 不在该 resolver 的白名单（例如 basic auth 拿到了 `kubernetes.io/dockerconfigjson`）。
- `False/MissingKeys` — Secret 里没有必需的 key（如 basic 缺 `password`、apiKey 缺 `apiKeyId` + `apiKeySecret`）。
- `False/DecodeError` — 某个 key 的 UTF-8 解码或 JSON 解析失败（仅 ACME 私钥走 JSON 校验，LinkSys 基本不会命中）。

`kubectl get linksys` 会直接显示 `Accepted` / `ResolvedRefs` 两列。

## 运行时行为

- **Controller 侧**：解析 `secretRef` → 在 `SecretRefManager` 登记 owner→secret 依赖（Secret 更新/删除会级联 requeue 本资源）→ 把整份 `Secret` 内嵌到 `spec.config.<auth>.secret` / `spec.config.tls.certs.secret` → 通过 conf_sync gRPC 同步到 Gateway。`Secret` 本身是 `no_sync_kind`，不会被单独同步。
- **Gateway 侧**：`LinkSysStore` 收到 spec 后，用 `src/types/credentials` 里的 resolver 再次从内嵌 `Secret.data` 算出 `Material`，交给 Redis / Etcd / ES / Webhook client 构建连接。凭据四态（`NotRequired` / `Pending` / `Invalid` / `Resolved`）中只有前者或 `Resolved` 允许构建 client，其余 fail-fast，**不会**退化为匿名连接。
- **热更新**：secretRef 指向的 Secret 变更 → Controller 重算 → 通过 conf_sync 推新版 spec → Gateway 重建对应 provider client，老连接优雅退出。

## 跨引用与依赖

| 引用方向 | 目标资源 | 机制 | 典型场景 |
|---------|---------|------|---------|
| LinkSys → Secret | `Secret` | `secretRef` / `tls.certs.secretRef` + `SecretRefManager` | 所有 auth / mTLS 凭据 |
| LinkSys → ReferenceGrant | `ReferenceGrant` | 跨 namespace `secretRef` 授权 | 把凭据集中到 `credentials` namespace |
| AccessLog → LinkSys | `LinkSys`（type=elasticsearch/redis/webhook） | 配置里按 `namespace/name` 引用 | 日志输出到 ES / Redis |
| EdgionPlugins → LinkSys | `LinkSys`（type=redis/webhook） | 插件配置按 `namespace/name` 引用 | 分布式限流、Webhook Key 解析 |

内部实现与 Handler 流程见 [01-architecture/05-resources/18-link-sys.md](../../01-architecture/05-resources/18-link-sys.md)。
